//! `Backend::spawn_stdio` and the `ProcessHandle` contract (PLAN §18.3): output, stderr lines
//! and the exit in order, stdin round trips, the working directory and environment, refusals,
//! a kill or a drop ending the process and everything it started, a process's exit taking its
//! children with it; then what only a remote backend does: a running process holds its server
//! against the idle close, and a lost connection ends its processes with an exit (what only the
//! server does is `server.rs`'s). The process is the helper `dv_test_child` (`src/bin`), never
//! a shell.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dv_backend::client::ConnectionOptions;
use dv_backend::{Connection, ConnectionState, Launcher};
use dv_core::backend::{Backend, BackendError, ProcessEvent, ProcessHandle, ProcessSpec};
use pretty_assertions::assert_eq;

use crate::{Side, Subject, committed, helper, same_dir};

crate::conformance!(
    child: prints_on_both_streams_and_exits_with_its_code,
    stdin_round_trips_in_order,
    runs_in_its_directory_with_its_environment,
    a_spec_that_cant_run_is_refused,
    a_kill_ends_it_and_what_it_started,
    dropping_the_handle_kills_it_and_stops_delivery,
    dropping_the_handle_from_inside_its_sink_kills_it,
    its_exit_takes_what_it_started_with_it,
    a_write_after_its_exit_fails,
);

pub(crate) const WITHIN: Duration = Duration::from_secs(20);

/// The helper running `steps`.
pub(crate) fn steps(steps: &[&str]) -> ProcessSpec {
    ProcessSpec::new(helper()).args(steps.iter().copied())
}

/// Sets its flag when dropped: a sink owning one tells when it is gone.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// A started process, its events on a channel, and a flag set once its sink is dropped.
fn start(
    backend: &dyn Backend,
    spec: &ProcessSpec,
) -> (ProcessHandle, Receiver<ProcessEvent>, Arc<AtomicBool>) {
    let (tx, rx) = mpsc::channel();
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = DropFlag(Arc::clone(&dropped));
    let handle = backend
        .spawn_stdio(
            spec,
            Box::new(move |event| {
                let _flag = &flag;
                let _ = tx.send(event);
            }),
        )
        .unwrap_or_else(|error| panic!("{spec:?}: {error:?}"));
    (handle, rx, dropped)
}

/// What a process printed, as its events said.
#[derive(Debug, Default)]
struct Seen {
    stdout: Vec<u8>,
    stderr: Vec<String>,
    exit: Option<Option<i32>>,
}

impl Seen {
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Takes the next event; an event after the exit fails the test.
    fn take(&mut self, event: ProcessEvent) {
        assert_eq!(self.exit, None, "{event:?} after the exit");
        match event {
            ProcessEvent::Stdout(bytes) => self.stdout.extend(bytes),
            ProcessEvent::Stderr(line) => self.stderr.push(line),
            ProcessEvent::Exit(code) => self.exit = Some(code),
        }
    }

    /// Takes events until `done` holds.
    fn until(&mut self, rx: &Receiver<ProcessEvent>, done: impl Fn(&Seen) -> bool) {
        let deadline = Instant::now() + WITHIN;
        while !done(self) {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(event) => self.take(event),
                Err(error) => panic!("not within {WITHIN:?} ({error:?}); saw {self:?}"),
            }
        }
    }

    fn until_exit(&mut self, rx: &Receiver<ProcessEvent>) -> Option<i32> {
        self.until(rx, |seen| seen.exit.is_some());
        self.exit.flatten()
    }

    /// The number after `<word> ` on a line of stdout, waiting for it.
    fn number(&mut self, rx: &Receiver<ProcessEvent>, word: &str) -> u32 {
        let find = |seen: &Seen| {
            seen.stdout().lines().find_map(|line| {
                line.strip_prefix(word)?
                    .strip_prefix(' ')?
                    .trim()
                    .parse::<u32>()
                    .ok()
            })
        };
        self.until(rx, |seen| find(seen).is_some());
        find(self).unwrap()
    }
}

/// Whether the process `pid` runs (a zombie nobody reaped yet doesn't).
pub(crate) fn running(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .expect("tasklist runs");
        String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
    }
    #[cfg(unix)]
    {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            // The state follows the command's closing parenthesis.
            Ok(stat) => stat
                .rsplit_once(')')
                .is_some_and(|(_, rest)| !rest.trim_start().starts_with('Z')),
            Err(_) => false,
        }
    }
}

/// Waits until the process `pid` is gone.
pub(crate) fn assert_gone(pid: u32, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while running(pid) {
        assert!(Instant::now() < deadline, "{what} (pid {pid}) still runs");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The exit a kill gives: a signal on Unix, the job's exit code on Windows.
fn killed() -> Option<i32> {
    if cfg!(windows) { Some(1) } else { None }
}

fn prints_on_both_streams_and_exits_with_its_code(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let (handle, rx, _) = start(
        &*backend,
        &steps(&[
            "out", "hello", "err", "oops", "err", "second", "out", "bye", "exit", "3",
        ]),
    );
    assert!(handle.pid() > 0);
    let mut seen = Seen::default();
    assert_eq!(seen.until_exit(&rx), Some(3));
    assert_eq!(seen.stdout(), "hello\nbye\n");
    assert_eq!(seen.stderr, ["oops", "second"]);
    // Nothing follows the exit.
    assert!(matches!(
        rx.recv_timeout(Duration::from_millis(300)),
        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected)
    ));
}

fn stdin_round_trips_in_order(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let (handle, rx, _) = start(&*backend, &steps(&["echo"]));
    let mut seen = Seen::default();
    handle.write(b"ping\n").unwrap();
    seen.until(&rx, |seen| seen.stdout == b"ping\n");
    // A megabyte in pieces, every byte value, back in order.
    let payload: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
    for piece in payload.chunks(100_000) {
        handle.write(piece).unwrap();
    }
    let total = 5 + payload.len();
    seen.until(&rx, |seen| seen.stdout.len() >= total);
    assert_eq!(seen.stdout.len(), total);
    assert!(
        seen.stdout[5..] == payload[..],
        "the bytes came back changed"
    );
    handle.kill();
    assert_eq!(seen.until_exit(&rx), killed());
}

fn runs_in_its_directory_with_its_environment(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let spec = steps(&["cwd", "env", "DV_TEST_CHILD_SET", "env", "PATH"])
        .cwd("sub/deep")
        .env("DV_TEST_CHILD_SET", Some("yes"))
        .env("PATH", None);
    let (_handle, rx, _) = start(&*backend, &spec);
    let mut seen = Seen::default();
    assert_eq!(seen.until_exit(&rx), Some(0));
    let stdout = seen.stdout();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{stdout}");
    assert!(
        same_dir(lines[0], repo.path().join("sub").join("deep")),
        "{}",
        lines[0]
    );
    assert_eq!(lines[1..], ["yes", "<unset>"]);
}

fn a_spec_that_cant_run_is_refused(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let refused = |spec: ProcessSpec| {
        backend
            .spawn_stdio(&spec, Box::new(|_| {}))
            .expect_err("refused")
    };
    let error = refused(ProcessSpec::new("dv_test_child"));
    assert!(
        matches!(&error, BackendError::InvalidProcess { program, .. } if program == "dv_test_child"),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "can't run dv_test_child: the program must be an absolute path"
    );
    let error = refused(steps(&[]).cwd("../outside"));
    assert!(
        matches!(error, BackendError::InvalidPath { .. }),
        "{error:?}"
    );
    let error = refused(steps(&[]).cwd("a.txt"));
    assert!(matches!(error, BackendError::Io { .. }), "{error:?}");
    let missing = repo.path().join("no-such-program").display().to_string();
    let error = refused(ProcessSpec::new(missing.clone()));
    assert!(
        matches!(&error, BackendError::StartProcess { program, .. } if *program == missing),
        "{error:?}"
    );
    assert!(
        error
            .to_string()
            .starts_with(&format!("couldn't start {missing}: "))
    );
    if cfg!(windows) {
        let error = refused(ProcessSpec::new(r"C:\tools\tsc.cmd"));
        assert!(
            matches!(error, BackendError::InvalidProcess { .. }),
            "{error:?}"
        );
    }
    // The backend serves on.
    let (_handle, rx, _) = start(&*backend, &steps(&["exit", "0"]));
    assert_eq!(Seen::default().until_exit(&rx), Some(0));
}

fn a_kill_ends_it_and_what_it_started(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let (handle, rx, dropped) = start(&*backend, &steps(&["child", "pid", "sleep", "600000"]));
    let mut seen = Seen::default();
    let child = seen.number(&rx, "child");
    let pid = seen.number(&rx, "pid");
    assert_eq!(pid, handle.pid());
    handle.kill();
    assert_eq!(seen.until_exit(&rx), killed());
    assert_gone(pid, "the process");
    assert_gone(child, "the process it started");
    // Killing it again, or writing to it, changes nothing.
    handle.kill();
    assert!(handle.write(b"x").is_err());
    // Its delivery ended with the exit, which drops the sink, the handle still held.
    let deadline = Instant::now() + WITHIN;
    while !dropped.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "the sink outlived the exit");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(handle);
}

fn dropping_the_handle_kills_it_and_stops_delivery(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let (handle, rx, dropped) = start(&*backend, &steps(&["child", "pid", "sleep", "600000"]));
    let mut seen = Seen::default();
    let child = seen.number(&rx, "child");
    let pid = seen.number(&rx, "pid");
    drop(handle);
    assert!(
        dropped.load(Ordering::SeqCst),
        "the sink is gone once drop returns"
    );
    assert_gone(pid, "the process");
    assert_gone(child, "the process it started");
    // No exit reaches a sink that is gone: the channel just ends.
    for event in rx.try_iter() {
        assert!(!matches!(event, ProcessEvent::Exit(_)), "{event:?}");
    }
}

fn dropping_the_handle_from_inside_its_sink_kills_it(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let slot: Arc<Mutex<Option<ProcessHandle>>> = Arc::default();
    let (tx, rx) = mpsc::channel();
    let handle = backend
        .spawn_stdio(
            &steps(&["pid", "sleep", "600000"]),
            Box::new({
                let slot = Arc::clone(&slot);
                move |event| {
                    if let ProcessEvent::Stdout(bytes) = &event {
                        let _ = tx.send(String::from_utf8_lossy(bytes).into_owned());
                        // The first output drops the handle, from inside the sink.
                        drop(slot.lock().unwrap().take());
                    }
                }
            }),
        )
        .unwrap();
    let pid = handle.pid();
    *slot.lock().unwrap() = Some(handle);
    let printed = rx.recv_timeout(WITHIN).expect("its pid");
    assert!(printed.starts_with("pid "), "{printed}");
    assert_gone(pid, "the process");
    assert!(slot.lock().unwrap().is_none());
}

fn its_exit_takes_what_it_started_with_it(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let (_handle, rx, _) = start(&*backend, &steps(&["child", "exit", "0"]));
    let mut seen = Seen::default();
    let child = seen.number(&rx, "child");
    assert_eq!(seen.until_exit(&rx), Some(0));
    assert_gone(child, "the process it started");
}

fn a_write_after_its_exit_fails(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let (handle, rx, _) = start(&*backend, &steps(&["exit", "0"]));
    assert_eq!(Seen::default().until_exit(&rx), Some(0));
    let error = handle.write(b"anyone?\n").expect_err("it has ended");
    assert!(matches!(error, BackendError::Io { .. }), "{error:?}");
    // Alike through the wire, named by its program.
    assert_eq!(
        error.to_string(),
        format!("writing to {} failed: the process has ended", helper())
    );
}

#[test]
fn refusals_read_the_same_through_the_wire() {
    let repo = committed();
    let (local, remote) = (Subject::new(Side::Local), Subject::new(Side::Remote));
    let (local, remote) = (local.open(&repo), remote.open(&repo));
    let missing = repo.path().join("no-such-program").display().to_string();
    for spec in [
        ProcessSpec::new("tsc"),
        steps(&["a\0b"]),
        steps(&[]).cwd("../outside"),
        steps(&[]).cwd("missing"),
        ProcessSpec::new(missing),
    ] {
        let local = local.spawn_stdio(&spec, Box::new(|_| {})).unwrap_err();
        let remote = remote.spawn_stdio(&spec, Box::new(|_| {})).unwrap_err();
        assert_eq!(
            std::mem::discriminant(&remote),
            std::mem::discriminant(&local),
            "{spec:?}: {local:?} vs {remote:?}"
        );
        assert_eq!(remote.to_string(), local.to_string(), "{spec:?}");
    }
}

#[test]
fn a_running_process_holds_its_server_against_the_idle_close() {
    let repo = committed();
    let connection = Connection::with_options(
        "in-process",
        Launcher::InProcess,
        ConnectionOptions {
            backoff: Vec::new(),
            idle: Some(Duration::from_millis(300)),
        },
    );
    let backend = connection
        .open(repo.path().to_str().unwrap(), repo.env_overlay())
        .unwrap();
    let (handle, rx, _) = start(&backend, &steps(&["pid", "sleep", "600000"]));
    let pid = Seen::default().number(&rx, "pid");
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(connection.state(), ConnectionState::Connected);
    assert_eq!(connection.generation(), 1);
    drop(handle);
    assert_gone(pid, "the process");
    let deadline = Instant::now() + WITHIN;
    while connection.state() != ConnectionState::Idle {
        assert!(Instant::now() < deadline, "{:?}", connection.state());
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_lost_connection_ends_its_processes_with_an_exit() {
    let repo = committed();
    // One respawn, a minute away: the death's own effects stay in view.
    let connection = Connection::with_backoff(
        "in-process",
        Launcher::InProcess,
        vec![Duration::from_secs(60)],
    );
    let backend = connection
        .open(repo.path().to_str().unwrap(), repo.env_overlay())
        .unwrap();
    let (handle, rx, _) = start(&backend, &steps(&["pid", "sleep", "600000"]));
    let mut seen = Seen::default();
    let pid = seen.number(&rx, "pid");
    connection.kill();
    assert_eq!(seen.until_exit(&rx), None);
    let error = handle.write(b"x").expect_err("it has ended");
    assert!(matches!(error, BackendError::Io { .. }), "{error:?}");
    // The server's input ended with the link, and it killed what it ran.
    assert_gone(pid, "the process");
}

#[test]
fn a_child_process_server_spawns_through_its_own_pipes() {
    // The child side's server is a process of its own: the pid its process reports differs
    // from both this test's and the server's.
    let repo = committed();
    let subject = Subject::new(Side::Child);
    let backend = subject.open(&repo);
    let server = subject.connection().server().expect("connected").pid;
    assert_ne!(server, std::process::id());
    let (handle, rx, _) = start(&*backend, &steps(&["pid", "exit", "0"]));
    let mut seen = Seen::default();
    let pid = seen.number(&rx, "pid");
    assert_eq!(seen.until_exit(&rx), Some(0));
    assert_eq!(pid, handle.pid());
    assert_ne!(pid, server);
}
