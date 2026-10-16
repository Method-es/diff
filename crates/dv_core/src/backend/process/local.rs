//! The processes a `LocalBackend` spawns on this machine (`super`'s module docs, PLAN §18.3).
//!
//! Four threads serve a process: two read its stdout and stderr into one bounded channel (a
//! sink that falls behind makes the process wait on its pipes, rather than dv buffer without
//! end), one waits for its exit, and one delivers the channel to the sink in order, the exit
//! last: once the process has exited and both pipes have ended, or [`DRAIN`] after the exit
//! when something the process started still holds a pipe. Such a process escaped the tree
//! (below: it made itself a session or a job of its own); the reader of the pipe it holds
//! stays blocked until it closes the pipe or exits, a thread dv can't reclaim earlier.
//!
//! **The process's tree.** On Unix the process leads a process group of its own
//! (`process_group(0)`); on Windows it is the one process in a job of its own
//! (`crate::job::Job`), which also dies with dv. Killing it kills the group or the job. Its
//! exit is seen before it is reaped (`waitid` with `WNOWAIT` on Unix), and what is left of its
//! group or job is killed then, before the reap: an unreaped process keeps its pid, so the
//! group's id can't have been reused by then. A wait that fails for another reason (the
//! process was reaped elsewhere) kills nothing: its group's id may already be another's.
//!
//! On Windows a process joins its job right after it starts, so one it starts in that instant
//! stays outside the job. Closing that window needs the process created suspended and its
//! first thread resumed once it is in the job, but `std::process::Command` keeps the thread's
//! handle to itself, and finding the thread again takes a snapshot of every thread on the
//! machine; a language server starts its children only once it has read a request, long after.

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{
    ProcessControl, ProcessEvent, ProcessHandle, ProcessSink, ProcessSpec, STDERR_LINE_MAX,
    bounded_line, ended,
};
use crate::backend::{BackendError, paths};

/// How many chunks and lines wait for the sink before the readers wait for it.
const QUEUE: usize = 64;

/// How long after the exit its pipes may stay open before the exit is delivered anyway.
const DRAIN: Duration = Duration::from_secs(1);

/// How much of stdout one read takes.
const CHUNK: usize = 64 * 1024;

/// What the delivery thread is sent.
enum Msg {
    Stdout(Vec<u8>),
    Stderr(String),
    StdoutEnd,
    StderrEnd,
    Exited(Option<i32>),
    /// The handle was dropped: deliver nothing more.
    Stop,
}

/// Starts `spec` in the worktree at `toplevel` (`super`'s module docs).
pub(in crate::backend) fn spawn(
    toplevel: &Path,
    spec: &ProcessSpec,
    sink: ProcessSink,
) -> Result<ProcessHandle, BackendError> {
    let (program, cwd) = check(toplevel, spec)?;
    let mut command = Command::new(&program);
    command
        .args(&spec.args)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in &spec.env {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    own_tree(&mut command);
    let start_error = |source: io::Error| BackendError::StartProcess {
        program: spec.program.clone(),
        source,
    };
    let mut child = command.spawn().map_err(start_error)?;
    let pid = child.id();
    let tree = Tree::of(&child);
    let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let running = Arc::new(Running::new(child, tree));
    let (Some(stdin), Some(stdout), Some(stderr)) = pipes else {
        running.kill();
        return Err(start_error(io::Error::other("its pipes weren't created")));
    };

    let (tx, rx) = mpsc::sync_channel(QUEUE);
    let stop = Arc::new(AtomicBool::new(false));
    let started = (|| {
        start_thread("dv-proc-wait", {
            let (running, tx) = (Arc::clone(&running), tx.clone());
            move || {
                let code = running.wait();
                let _ = tx.send(Msg::Exited(code));
            }
        })?;
        start_thread("dv-proc-out", {
            let tx = tx.clone();
            move || read_stdout(stdout, &tx)
        })?;
        start_thread("dv-proc-err", {
            let tx = tx.clone();
            move || read_stderr(stderr, &tx)
        })?;
        start_thread("dv-proc", {
            let stop = Arc::clone(&stop);
            move || deliver(&rx, &stop, sink)
        })
    })();
    let delivery = match started {
        Ok(delivery) => delivery,
        Err(error) => {
            // The threads that did start end with the process.
            running.kill();
            return Err(start_error(error));
        }
    };
    Ok(ProcessHandle::new(
        pid,
        LocalProcess {
            program: spec.program.clone(),
            stdin: Mutex::new(Some(stdin)),
            running,
            stop,
            wake: tx,
            delivery: Some(delivery),
        },
    ))
}

/// The executable and the working directory `spec` names, checked (`ProcessSpec`'s docs).
fn check(toplevel: &Path, spec: &ProcessSpec) -> Result<(PathBuf, PathBuf), BackendError> {
    let refuse = |reason: &'static str| BackendError::InvalidProcess {
        program: spec.program.clone(),
        reason: reason.into(),
    };
    if spec.program.contains('\0') {
        return Err(refuse("the program's path contains NUL"));
    }
    let program = PathBuf::from(&spec.program);
    if !program.is_absolute() {
        return Err(refuse("the program must be an absolute path"));
    }
    if cfg!(windows) && is_batch_file(&program) {
        return Err(refuse(
            "a batch file runs only through cmd.exe, and dv runs no shell",
        ));
    }
    if spec.args.iter().any(|arg| arg.contains('\0')) {
        return Err(refuse("an argument contains NUL"));
    }
    for (name, value) in &spec.env {
        if name.is_empty() || name.contains(['=', '\0']) {
            return Err(refuse(
                "an environment variable's name is empty or has '=' or NUL",
            ));
        }
        if value.as_ref().is_some_and(|value| value.contains('\0')) {
            return Err(refuse("an environment variable's value contains NUL"));
        }
    }
    let cwd = if spec.cwd.is_empty() {
        toplevel.to_path_buf()
    } else {
        paths::worktree_path(toplevel, &spec.cwd)?
    };
    match fs::metadata(&cwd) {
        Ok(metadata) if metadata.is_dir() => Ok((program, cwd)),
        Ok(_) => Err(BackendError::io(
            "starting a process in",
            &cwd,
            io::Error::new(io::ErrorKind::NotADirectory, "not a directory"),
        )),
        Err(error) => Err(BackendError::io("starting a process in", &cwd, error)),
    }
}

/// Whether Windows would run `program` through `cmd.exe`: a `.bat` or `.cmd` file, also spelled
/// with trailing dots or spaces, which Windows drops from a file name.
fn is_batch_file(program: &Path) -> bool {
    let Some(name) = program.file_name() else {
        return false;
    };
    let name = name.to_string_lossy();
    let name = name.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    name.ends_with(".bat") || name.ends_with(".cmd")
}

fn start_thread(name: &str, f: impl FnOnce() + Send + 'static) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new().name(name.into()).spawn(f)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A local process's [`ProcessControl`].
struct LocalProcess {
    /// As the spec named it, for errors.
    program: String,
    /// `None` once the handle is dropped. Its own lock, so a write blocked on a full pipe never
    /// holds up a kill.
    stdin: Mutex<Option<ChildStdin>>,
    running: Arc<Running>,
    stop: Arc<AtomicBool>,
    /// Wakes the delivery thread when the handle is dropped.
    wake: SyncSender<Msg>,
    delivery: Option<JoinHandle<()>>,
}

impl ProcessControl for LocalProcess {
    fn write(&self, bytes: &[u8]) -> Result<(), BackendError> {
        let mut stdin = lock(&self.stdin);
        let Some(pipe) = stdin.as_mut() else {
            return Err(ended(&self.program));
        };
        pipe.write_all(bytes)
            .and_then(|()| pipe.flush())
            .map_err(|source| {
                // Its stdin closed: it has ended, or is ending, and says so as the server does.
                if source.kind() == io::ErrorKind::BrokenPipe {
                    ended(&self.program)
                } else {
                    BackendError::Io {
                        op: "writing to".into(),
                        path: self.program.clone(),
                        source,
                    }
                }
            })
    }

    fn kill(&self) {
        self.running.kill();
    }
}

impl Drop for LocalProcess {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.running.kill();
        drop(lock(&self.stdin).take());
        // A full queue needs no wake-up: the delivery thread sees `stop` at its next message.
        let _ = self.wake.try_send(Msg::Stop);
        if let Some(thread) = self.delivery.take() {
            // Dropped from inside the sink, the thread is this one: it stops when the sink
            // returns and sees `stop`, and joining it here would deadlock.
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}

/// The process, its tree, and whether it has exited: under one lock, so a kill never reaches
/// a group whose leader was reaped (module docs).
struct Running {
    state: Mutex<State>,
    /// The process's handle, which the waiter waits on without the lock; valid while `state`
    /// holds the `Child`.
    #[cfg(windows)]
    process: isize,
    #[cfg(unix)]
    pid: u32,
}

struct State {
    child: Child,
    tree: Tree,
    exited: bool,
}

impl Running {
    fn new(child: Child, tree: Tree) -> Self {
        #[cfg(windows)]
        let process = {
            use std::os::windows::io::AsRawHandle as _;
            child.as_raw_handle() as isize
        };
        #[cfg(unix)]
        let pid = child.id();
        Self {
            #[cfg(windows)]
            process,
            #[cfg(unix)]
            pid,
            state: Mutex::new(State {
                child,
                tree,
                exited: false,
            }),
        }
    }

    /// Kills the process and its tree, unless it has exited (then its tree is gone already).
    fn kill(&self) {
        let mut state = lock(&self.state);
        if !state.exited {
            let State { child, tree, .. } = &mut *state;
            tree.kill(child);
        }
    }

    /// Waits for the process to exit, ends what is left of its tree, reaps it, and returns its
    /// exit code (`None` for a signal). When the wait itself fails (module docs) it is taken as
    /// ended, killing nothing, with no code.
    fn wait(&self) -> Option<i32> {
        let exited = self.block_until_exited();
        let mut state = lock(&self.state);
        state.exited = true;
        if let Err(error) = exited {
            log::warn!("couldn't wait for process {}: {error}", state.child.id());
            return None;
        }
        state.tree.end_rest();
        match state.child.wait() {
            Ok(status) => status.code(),
            Err(error) => {
                log::debug!("couldn't reap process {}: {error}", state.child.id());
                None
            }
        }
    }

    /// Blocks until the process has exited, leaving it unreaped.
    #[cfg(unix)]
    fn block_until_exited(&self) -> io::Result<()> {
        // A pid is positive and fits every platform's `id_t`.
        let pid = self.pid as libc::id_t;
        loop {
            // SAFETY: an all-zero `siginfo_t` is a valid value of this plain C struct, which
            // `waitid` overwrites.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: waits for this process's own child `pid` to exit; `WNOWAIT` leaves it
            // unreaped, so its pid stays taken until `Child::wait` reaps it.
            let waited =
                unsafe { libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT) };
            if waited == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    /// Blocks until the process has exited.
    #[cfg(windows)]
    fn block_until_exited(&self) -> io::Result<()> {
        use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};
        // SAFETY: the handle is the `Child`'s, which `state` keeps open while this `Running`
        // lives, and the waiter holds it.
        let waited = unsafe { WaitForSingleObject(self.process as HANDLE, INFINITE) };
        if waited == WAIT_OBJECT_0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

/// A process and everything it starts (module docs).
struct Tree {
    /// The process group's id: the process's own pid.
    #[cfg(unix)]
    group: libc::pid_t,
    /// `None` when the OS wouldn't make one; the process is then tied to dv's job alone.
    #[cfg(windows)]
    job: Option<crate::job::Job>,
}

impl Tree {
    fn of(child: &Child) -> Self {
        #[cfg(unix)]
        {
            Self {
                group: libc::pid_t::try_from(child.id()).unwrap_or(0),
            }
        }
        #[cfg(windows)]
        {
            let job = crate::job::Job::of(child);
            if job.is_none() {
                crate::job::tie_to_this_process(child);
            }
            Self { job }
        }
    }

    /// Kills the process and everything in its tree.
    fn kill(&self, child: &mut Child) {
        #[cfg(unix)]
        {
            let _ = child;
            self.end_rest();
        }
        #[cfg(windows)]
        match &self.job {
            Some(job) => job.terminate(1),
            None => {
                let _ = child.kill();
            }
        }
    }

    /// Kills whatever is in the tree; once the process has exited, what it left behind.
    fn end_rest(&self) {
        #[cfg(unix)]
        if self.group > 0 {
            // SAFETY: a plain syscall; the group's leader is unreaped (module docs), so its id
            // is still this group's.
            unsafe { libc::kill(-self.group, libc::SIGKILL) };
        }
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.terminate(1);
        }
    }
}

/// The process runs in a tree of its own, and on Windows without a console window.
fn own_tree(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        /// `CREATE_NO_WINDOW` from the Win32 process creation flags.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
}

/// The stdout reader: chunks as the pipe gives them, then the end.
fn read_stdout(mut stdout: impl Read, tx: &SyncSender<Msg>) {
    let mut buffer = vec![0u8; CHUNK];
    loop {
        match stdout.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if tx.send(Msg::Stdout(buffer[..read].to_vec())).is_err() {
                    return;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let _ = tx.send(Msg::StdoutEnd);
}

/// The stderr reader: line by line, then the end.
fn read_stderr(stderr: impl Read, tx: &SyncSender<Msg>) {
    let mut reader = BufReader::new(stderr);
    while let Ok(Some(line)) = read_line(&mut reader) {
        if tx.send(Msg::Stderr(line)).is_err() {
            return;
        }
    }
    let _ = tx.send(Msg::StderrEnd);
}

/// The next line without its `\n` (or `\r\n`), bounded ([`bounded_line`]): whatever a line
/// holds past [`STDERR_LINE_MAX`] is read and dropped. `None` at the end.
fn read_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    let mut any = false;
    loop {
        let buffer = match reader.fill_buf() {
            Ok(buffer) => buffer,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if buffer.is_empty() {
            break;
        }
        any = true;
        let (taken, done) = match buffer.iter().position(|&b| b == b'\n') {
            Some(at) => (at, true),
            None => (buffer.len(), false),
        };
        // One byte past the bound tells `bounded_line` the line was cut.
        let room = (STDERR_LINE_MAX + 1).saturating_sub(line.len());
        line.extend_from_slice(&buffer[..taken.min(room)]);
        reader.consume(if done { taken + 1 } else { taken });
        if done {
            break;
        }
    }
    if !any {
        return Ok(None);
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(Some(bounded_line(
        String::from_utf8_lossy(&line).into_owned(),
    )))
}

/// The delivery thread (module docs): the sink hears the channel in order, the exit last,
/// until the handle is dropped.
fn deliver(rx: &Receiver<Msg>, stop: &AtomicBool, mut sink: ProcessSink) {
    let mut open_pipes = 2;
    let mut exit = None;
    let mut deadline: Option<Instant> = None;
    while exit.is_none() || open_pipes > 0 {
        let msg = match deadline {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
        };
        let msg = match msg {
            Ok(msg) => msg,
            // A pipe still open after the drain: the exit goes out anyway.
            Err(RecvTimeoutError::Timeout) => break,
            // Only once the handle is gone.
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if stop.load(Ordering::Acquire) {
            return;
        }
        match msg {
            Msg::Stdout(bytes) => sink(ProcessEvent::Stdout(bytes)),
            Msg::Stderr(line) => sink(ProcessEvent::Stderr(line)),
            Msg::StdoutEnd | Msg::StderrEnd => open_pipes -= 1,
            Msg::Exited(code) => {
                exit = Some(code);
                deadline = Some(Instant::now() + DRAIN);
            }
            Msg::Stop => return,
        }
    }
    if let Some(code) = exit
        && !stop.load(Ordering::Acquire)
    {
        sink(ProcessEvent::Exit(code));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(input: &[u8]) -> Vec<String> {
        let mut reader = BufReader::with_capacity(16, input);
        let mut lines = Vec::new();
        while let Some(line) = read_line(&mut reader).unwrap() {
            lines.push(line);
        }
        lines
    }

    #[test]
    fn stderr_reads_as_bounded_lines() {
        assert_eq!(lines(b""), Vec::<String>::new());
        assert_eq!(lines(b"a\nb\r\n\nlast"), ["a", "b", "", "last"]);
        assert_eq!(lines(b"caf\xc3\xa9 \xff\n"), ["café \u{fffd}"]);
        let long = format!("{}\nnext\n", "é".repeat(STDERR_LINE_MAX));
        let read = lines(long.as_bytes());
        assert_eq!(read.len(), 2);
        assert!(read[0].len() <= STDERR_LINE_MAX, "{}", read[0].len());
        assert!(read[0].ends_with("é…"), "cut at a character boundary");
        assert_eq!(read[1], "next");
    }

    #[test]
    fn a_spec_is_checked_before_anything_starts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("file"), "").unwrap();
        let absolute = if cfg!(windows) {
            r"C:\Windows\System32\where.exe"
        } else {
            "/bin/true"
        };
        let reason = |spec: ProcessSpec| match check(dir.path(), &spec) {
            Err(BackendError::InvalidProcess { reason, .. }) => reason.into_owned(),
            other => panic!("{spec:?}: {other:?}"),
        };
        assert_eq!(
            reason(ProcessSpec::new("tsc")),
            "the program must be an absolute path"
        );
        assert_eq!(
            reason(ProcessSpec::new(absolute).args(["a\0b"])),
            "an argument contains NUL"
        );
        assert_eq!(
            reason(ProcessSpec::new(absolute).env("A=B", Some("x"))),
            "an environment variable's name is empty or has '=' or NUL"
        );
        if cfg!(windows) {
            for program in [
                r"C:\tools\tsc.CMD",
                r"C:\tools\tsc.bat",
                r"C:\tools\tsc.cmd.",
                r"C:\tools\tsc.cmd ",
                r"C:\tools\tsc.bat. .",
            ] {
                assert_eq!(
                    reason(ProcessSpec::new(program)),
                    "a batch file runs only through cmd.exe, and dv runs no shell",
                    "{program:?}"
                );
            }
        }
        assert!(is_batch_file(Path::new("/x/tsc.cmd.")));
        assert!(!is_batch_file(Path::new("/x/tsc.exe")));
        assert!(!is_batch_file(Path::new("/x/cmd")));
        assert!(matches!(
            check(dir.path(), &ProcessSpec::new(absolute).cwd("../x")),
            Err(BackendError::InvalidPath { .. })
        ));
        assert!(matches!(
            check(dir.path(), &ProcessSpec::new(absolute).cwd("missing")),
            Err(BackendError::Io { .. })
        ));
        assert!(matches!(
            check(dir.path(), &ProcessSpec::new(absolute).cwd("file")),
            Err(BackendError::Io { .. })
        ));
        let (program, cwd) = check(dir.path(), &ProcessSpec::new(absolute).cwd("sub")).unwrap();
        assert_eq!(program, PathBuf::from(absolute));
        assert_eq!(cwd, dir.path().join("sub"));
        assert_eq!(
            check(dir.path(), &ProcessSpec::new(absolute)).unwrap().1,
            dir.path()
        );
    }
}
