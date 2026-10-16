//! The server behind `dv backend --stdio` (docs/PLAN.md D6, §5.1): [`serve`] answers the
//! protocol ([`crate::proto`]) on a reader and a writer, with a `LocalBackend` per open
//! repository.
//!
//! - One loop reads request lines. Each request runs on a thread of its own: requests are
//!   coarse, and a long `git` must not hold up a `store_read` behind it. Responses go out in
//!   whatever order the calls finish.
//! - One writer thread owns the output and writes whole lines from a channel; the requests and
//!   the watches' sinks send their lines there. Nothing else writes the output (logging goes to
//!   stderr), so lines never interleave.
//! - A line that isn't a request gets a `protocol` error (id 0 when it names none) and the loop
//!   carries on.
//! - At the end of the input the watches stop, the writer writes out what it has and [`serve`]
//!   returns, whatever requests are still running: the `dv` process exits right after
//!   (`dv_cli`'s `backend` command), which is how a server dies with its client.
//!
//! A watch's batches never overtake its `watch` answer: its sink holds them back until the
//! answer is queued ([`Gate`]), so the client knows the watch id before its first batch. A
//! process's output waits for its `spawn` answer the same way.
//!
//! **Processes** (PLAN §18.3, [`process`]). A `spawn` starts one through the repository's
//! `LocalBackend`, in a process group of its own; its output goes out as `proc` lines, its exit
//! last, after which the server forgets it, and what waits for the writer is bounded per
//! process. `proc_kill` kills it (its exit line follows). At the end of the input every process
//! still running is killed with its group, a `spawn` still starting one included, so none
//! outlives the client.

mod process;

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, BufRead as _, BufReader, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use dv_core::backend::{
    Backend, BackendError, LocalBackend, LocalBackendOptions, WatchEvent, WatchHandle,
};
use parking_lot::Mutex;
use serde::Serialize;

use self::process::{Credit, Procs};
use crate::proto::{
    self, Bytes, HelloReply, Op, OpenReply, PROTO, Request, WatchReply, WireError, WireGitOutput,
    WirePathProbe,
};

/// Serves the protocol: requests from `input`, answers and watch batches to `output`, until
/// `input` ends (module docs). Returns once the writer has written everything queued before
/// the end; requests still running then are abandoned. A read error on `input` ends it too and
/// is returned.
pub fn serve(input: impl Read, output: impl Write + Send + 'static) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();
    let writer = std::thread::Builder::new()
        .name("dv-backend-writer".into())
        .spawn(move || write_lines(output, rx))?;
    let state = Arc::new(State::new(tx.clone()));
    let result = read_requests(input, &state);
    state.shutdown();
    let (ack_tx, ack_rx) = mpsc::channel();
    if tx.send(Out::Stop(ack_tx)).is_ok() {
        // An error means the writer already stopped (its output broke).
        let _ = ack_rx.recv();
    }
    let _ = writer.join();
    result
}

/// What the writer thread is sent.
enum Out {
    /// A whole `\n`-terminated line, and for a process's line the bytes it counts against the
    /// process's budget until written ([`process`]).
    Line(Vec<u8>, Option<Credit>),
    /// Flush, acknowledge and stop.
    Stop(Sender<()>),
}

/// The writer thread: writes each line whole, flushing whenever nothing else is queued, until
/// told to stop or the output breaks (the client is gone).
fn write_lines(mut output: impl Write, rx: Receiver<Out>) {
    let mut next = rx.recv().ok();
    while let Some(out) = next.take() {
        match out {
            Out::Line(line, credit) => {
                if let Err(error) = output.write_all(&line) {
                    log::debug!("the backend's output broke: {error}");
                    return;
                }
                // Written: its bytes no longer wait.
                drop(credit);
            }
            Out::Stop(ack) => {
                let _ = output.flush();
                let _ = ack.send(());
                return;
            }
        }
        next = match rx.try_recv() {
            Ok(out) => Some(out),
            Err(TryRecvError::Empty) => {
                if let Err(error) = output.flush() {
                    log::debug!("the backend's output broke: {error}");
                    return;
                }
                rx.recv().ok()
            }
            Err(TryRecvError::Disconnected) => None,
        };
    }
    let _ = output.flush();
}

/// Reads request lines until `input` ends, starting each request on its own thread.
fn read_requests(input: impl Read, state: &Arc<State>) -> io::Result<()> {
    let mut reader = BufReader::with_capacity(64 * 1024, input);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        let text = trim_line(&line);
        if text.is_empty() {
            continue;
        }
        match serde_json::from_slice::<Request>(text) {
            Ok(request) => state.start(request),
            Err(error) => {
                let id = request_id(text);
                log::warn!("a request the backend can't read (id {id}): {error}");
                state.send(proto::err_line(
                    id,
                    &WireError::protocol(format!("not a request: {error}")),
                ));
            }
        }
    }
}

/// A line without its `\n` and a `\r` before it.
pub(crate) fn trim_line(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if line.iter().all(u8::is_ascii_whitespace) {
        &[]
    } else {
        line
    }
}

/// The `id` of a line that isn't a valid request, when it is JSON with a numeric `id`; else 0.
fn request_id(text: &[u8]) -> u64 {
    serde_json::from_slice::<serde_json::Value>(text)
        .ok()
        .and_then(|value| value.get("id").and_then(serde_json::Value::as_u64))
        .unwrap_or(0)
}

/// The open repositories, watches and processes, shared by every request thread.
struct State {
    out: Sender<Out>,
    repos: Mutex<HashMap<u32, Arc<LocalBackend>>>,
    watches: Mutex<HashMap<u32, WatchHandle>>,
    procs: Procs,
    next_repo: AtomicU32,
    next_watch: AtomicU32,
    next_proc: AtomicU32,
}

impl State {
    fn new(out: Sender<Out>) -> Self {
        Self {
            out,
            repos: Mutex::default(),
            watches: Mutex::default(),
            procs: Procs::default(),
            next_repo: AtomicU32::new(1),
            next_watch: AtomicU32::new(1),
            next_proc: AtomicU32::new(1),
        }
    }

    /// Queues a line for the writer; dropped when the writer has stopped.
    fn send(&self, line: Vec<u8>) {
        let _ = self.out.send(Out::Line(line, None));
    }

    /// Runs `request` on a thread of its own, which sends its answer.
    fn start(self: &Arc<Self>, request: Request) {
        let Request { id, op } = request;
        let name = op.name();
        let state = Arc::clone(self);
        let started = std::thread::Builder::new()
            .name(format!("dv-backend-{name}"))
            .spawn(move || state.respond(id, op));
        if let Err(error) = started {
            log::error!("couldn't start a thread for {name}: {error}");
            self.send(proto::err_line(
                id,
                &WireError::protocol(format!("the backend couldn't start a thread: {error}")),
            ));
        }
    }

    /// Runs one request and sends its answer.
    fn respond(self: &Arc<Self>, id: u64, op: Op) {
        let line = match op {
            // The answer and the gate are the watch's and the process's own business ([`Gate`]).
            Op::Watch { repo, spec } => match self.watch(id, repo, spec) {
                Ok(()) => return,
                Err(error) => Err(error),
            },
            Op::Spawn { repo, spec } => match self.spawn(id, repo, &spec) {
                Ok(()) => return,
                Err(error) => Err(error),
            },
            op => self.run(id, op),
        };
        self.send(line.unwrap_or_else(|error| proto::err_line(id, &error)));
    }

    /// The answer line to every op but `watch`.
    fn run(&self, id: u64, op: Op) -> Result<Vec<u8>, WireError> {
        fn ok<T: Serialize + ?Sized>(id: u64, value: &T) -> Result<Vec<u8>, WireError> {
            Ok(proto::ok_line(id, value))
        }
        let wire = |error: BackendError| WireError::from(&error);
        match op {
            Op::Hello { build, .. } => ok(id, &hello(build)),
            Op::Open { path, env } => {
                let env = env
                    .into_iter()
                    .map(|(name, value)| (OsString::from(name), value.map(OsString::from)))
                    .collect();
                let backend =
                    LocalBackend::open_with(&path, LocalBackendOptions { env }).map_err(wire)?;
                let info = backend.repo().clone();
                let repo = self.next_repo.fetch_add(1, Ordering::Relaxed);
                self.repos.lock().insert(repo, Arc::new(backend));
                ok(id, &OpenReply { repo, info })
            }
            Op::Close { repo } => {
                self.repos.lock().remove(&repo);
                ok(id, &())
            }
            Op::Git { repo, args, opts } => {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let output = self.repo(repo)?.git(&args, &opts.into()).map_err(wire)?;
                ok(id, &WireGitOutput::from(output))
            }
            Op::WorktreeRead { repo, rel } => {
                let bytes = self.repo(repo)?.worktree_read(&rel).map_err(wire)?;
                ok(id, &bytes.map(Bytes))
            }
            Op::WorktreeLen { repo, rel } => {
                ok(id, &self.repo(repo)?.worktree_len(&rel).map_err(wire)?)
            }
            Op::WorktreeTextStats { repo, rel } => ok(
                id,
                &self.repo(repo)?.worktree_text_stats(&rel).map_err(wire)?,
            ),
            Op::ProbePath { repo, path } => {
                let probe = self.repo(repo)?.probe_path(&path).map_err(wire)?;
                ok(id, &WirePathProbe::from(probe))
            }
            Op::StoreAppend { repo, rel, line } => {
                self.repo(repo)?.store_append(&rel, &line).map_err(wire)?;
                ok(id, &())
            }
            Op::StoreRead { repo, rel } => {
                let bytes = self.repo(repo)?.store_read(&rel).map_err(wire)?;
                ok(id, &bytes.map(Bytes))
            }
            Op::StoreList { repo, dir } => {
                ok(id, &self.repo(repo)?.store_list(&dir).map_err(wire)?)
            }
            Op::StoreWriteAtomic { repo, rel, bytes } => {
                self.repo(repo)?
                    .store_write_atomic(&rel, &bytes)
                    .map_err(wire)?;
                ok(id, &())
            }
            Op::StoreRemove { repo, rel } => {
                ok(id, &self.repo(repo)?.store_remove(&rel).map_err(wire)?)
            }
            Op::CopyIndexToStore { repo, rel } => ok(
                id,
                &self.repo(repo)?.copy_index_to_store(&rel).map_err(wire)?,
            ),
            Op::Unwatch { watch } => {
                // Dropping the handle stops the watch and joins its thread; its sink only
                // queues lines, so that never waits on the writer.
                let handle = self.watches.lock().remove(&watch);
                drop(handle);
                ok(id, &())
            }
            Op::ProcWrite { process, bytes } => {
                self.write(process, &bytes)?;
                ok(id, &())
            }
            Op::ProcKill { process } => {
                self.kill(process);
                ok(id, &())
            }
            Op::Watch { .. } | Op::Spawn { .. } => Err(WireError::protocol(format!(
                "{} is answered by its own path",
                op.name()
            ))),
        }
    }

    /// Starts a watch and answers it: the answer is queued before the gate opens, so no batch
    /// of the watch precedes it.
    fn watch(
        &self,
        id: u64,
        repo: u32,
        spec: dv_core::backend::WatchSpec,
    ) -> Result<(), WireError> {
        let backend = self.repo(repo)?;
        let watch = self.next_watch.fetch_add(1, Ordering::Relaxed);
        let gate = Arc::new(Gate::default());
        let sink = {
            let (gate, out) = (Arc::clone(&gate), self.out.clone());
            Box::new(move |event: WatchEvent| {
                gate.deliver(proto::watch_line(watch, &event), None, &out);
            })
        };
        let handle = backend
            .watch(spec, sink)
            .map_err(|error| WireError::from(&error))?;
        let reply = WatchReply {
            watch,
            worktree_error: handle.worktree_error().map(str::to_string),
        };
        self.watches.lock().insert(watch, handle);
        self.send(proto::ok_line(id, &reply));
        gate.open(&self.out);
        Ok(())
    }

    fn repo(&self, repo: u32) -> Result<Arc<LocalBackend>, WireError> {
        self.repos
            .lock()
            .get(&repo)
            .cloned()
            .ok_or_else(|| WireError::protocol(format!("no repository is open as handle {repo}")))
    }

    /// Stops every watch, kills every process and forgets every repository: the input ended.
    fn shutdown(&self) {
        let watches: Vec<WatchHandle> = self.watches.lock().drain().map(|(_, h)| h).collect();
        drop(watches);
        self.procs.close();
        self.repos.lock().clear();
    }
}

/// `hello`'s answer, whatever protocol the client speaks: it decides. The executable is hashed
/// only when the client sent a build to compare.
fn hello(build: Option<String>) -> HelloReply {
    let build_ok = build.map(|expected| match crate::build::own_hash() {
        Ok(own) => own.eq_ignore_ascii_case(expected.trim()),
        Err(error) => {
            log::warn!("couldn't hash the backend's own executable: {error}");
            false
        }
    });
    HelloReply {
        proto: PROTO,
        build_ok,
        pid: std::process::id(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Holds a new watch's batches, or a new process's output, back until its answer is queued
/// (module docs): closed, it buffers the lines; [`Gate::open`] sends the buffer and lets later
/// lines straight through.
struct Gate {
    /// `None` once open.
    held: Mutex<Option<Vec<Held>>>,
}

/// A line the gate holds back, and its credit (`Out::Line`).
type Held = (Vec<u8>, Option<Credit>);

impl Default for Gate {
    fn default() -> Self {
        Self {
            held: Mutex::new(Some(Vec::new())),
        }
    }
}

impl Gate {
    fn deliver(&self, line: Vec<u8>, credit: Option<Credit>, out: &Sender<Out>) {
        let mut held = self.held.lock();
        match held.as_mut() {
            Some(buffer) => buffer.push((line, credit)),
            None => {
                let _ = out.send(Out::Line(line, credit));
            }
        }
    }

    fn open(&self, out: &Sender<Out>) {
        let mut held = self.held.lock();
        for (line, credit) in held.take().unwrap_or_default() {
            let _ = out.send(Out::Line(line, credit));
        }
    }
}

#[cfg(test)]
mod tests;
