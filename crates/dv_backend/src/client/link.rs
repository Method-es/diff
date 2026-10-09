//! One live session with one server process: the requests in flight, the watches routed to
//! their channels, the reader thread and the stderr drain (docs/PLAN.md §5.1, §5.2).
//!
//! The reader thread routes each reply by id to the call waiting for it (a channel per call)
//! and each watch batch to its watch's channel. Before the server's first reply arrives, lines
//! that aren't part of the protocol are kept in the diagnostics and otherwise ignored
//! (`wsl.exe` can print warnings on stdout before dv starts); after it, such a line is a
//! protocol failure, handled as the link's death.
//!
//! **Death** (the output ends, a read or a write fails, a protocol failure, the server's
//! process exits, [`Link::die`]): the server's process is killed and reaped, the connection
//! hears of it (and reconnects), every call in flight fails at once with the reason, later
//! calls fail before they are sent, the watches' routes go and the server's stdin closes. A
//! call registers under the same lock the death takes, so none can slip in after the death and
//! wait forever.
//!
//! The process's exit counts on its own, without waiting for its output to end: on Windows a
//! child inherits its parent's handles, so a git the server started can hold the server's
//! stdout open after the server is gone.

use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead as _, BufReader, Read, Write};
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use dv_core::backend::WatchEvent;
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

use super::Connection;
use crate::launch::Transport;
use crate::proto::{Incoming, Op, Request, WatchReply, WireError};
use crate::server::trim_line;

/// How many lines of the server's stderr (and of stray output before the handshake) the
/// diagnostics keep.
pub(crate) const DIAGNOSTIC_LINES: usize = 200;

/// How many lines a server that died before it answered still has kept in the diagnostics.
const STRAY_AFTER_DEATH: u32 = 20;

/// How often the server's process is checked for an exit (module docs).
const CHILD_POLL: Duration = Duration::from_millis(250);

/// Why a call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CallError {
    /// The server answered with an error.
    Remote(WireError),
    /// The link died before the answer came (the reason).
    Dead(String),
}

/// A reply as the reader thread hands it to the waiting call.
enum Reply {
    Ok(Option<Box<RawValue>>),
    Err(WireError),
    Dead(String),
}

/// Who waits for a request's reply.
enum Waiter {
    Call(Sender<Reply>),
    /// A `watch`: the reader routes the watch's batches to `events` from the reply on, before
    /// it reads the next line.
    Watch {
        reply: Sender<Reply>,
        events: Sender<WatchEvent>,
    },
    /// Nobody (`close`, `unwatch`): the reply is read and dropped.
    Discard,
}

/// The calls in flight, and whether the link has died (the reason).
#[derive(Default)]
struct Pending {
    dead: Option<String>,
    calls: HashMap<u64, Waiter>,
}

/// What the reader thread shares with the link's owners.
pub(crate) struct LinkShared {
    generation: u64,
    pending: Mutex<Pending>,
    routes: Mutex<HashMap<u32, Sender<WatchEvent>>>,
    /// The server's stdin; `None` once closed.
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    child: Mutex<Option<Child>>,
    diagnostics: Arc<Mutex<VecDeque<String>>>,
    connection: Weak<Connection>,
}

/// A live link, held by the connection and by the calls using it. Dropping the last one closes
/// the server's stdin, which ends a server, and reaps its process.
pub(crate) struct Link {
    shared: Arc<LinkShared>,
    next_id: AtomicU64,
}

impl Link {
    /// Starts the reader thread (and the stderr drain and the process's watcher) on
    /// `transport`, the server of `generation`.
    pub(crate) fn start(
        transport: Transport,
        generation: u64,
        diagnostics: Arc<Mutex<VecDeque<String>>>,
        connection: Weak<Connection>,
    ) -> io::Result<Arc<Self>> {
        let Transport {
            reader,
            writer,
            stderr,
            child,
            notes,
            ..
        } = transport;
        for note in notes {
            push_diagnostic(&diagnostics, note);
        }
        let has_child = child.is_some();
        let shared = Arc::new(LinkShared {
            generation,
            pending: Mutex::default(),
            routes: Mutex::default(),
            writer: Mutex::new(Some(writer)),
            child: Mutex::new(child),
            diagnostics,
            connection,
        });
        if let Some(stderr) = stderr {
            let diagnostics = Arc::clone(&shared.diagnostics);
            let started = std::thread::Builder::new()
                .name("dv-backend-stderr".into())
                .spawn(move || drain_stderr(stderr, &diagnostics));
            if let Err(error) = started {
                shared.die(format!("couldn't start the stderr thread: {error}"));
                return Err(error);
            }
        }
        if has_child {
            let child_shared = Arc::clone(&shared);
            let started = std::thread::Builder::new()
                .name("dv-backend-child".into())
                .spawn(move || watch_child(&child_shared));
            if let Err(error) = started {
                shared.die(format!("couldn't start the process watcher: {error}"));
                return Err(error);
            }
        }
        let reader_shared = Arc::clone(&shared);
        let started = std::thread::Builder::new()
            .name("dv-backend-reader".into())
            .spawn(move || read_lines(reader, &reader_shared));
        if let Err(error) = started {
            shared.die(format!("couldn't start the reader thread: {error}"));
            return Err(error);
        }
        Ok(Arc::new(Self {
            shared,
            next_id: AtomicU64::new(1),
        }))
    }

    pub(crate) fn generation(&self) -> u64 {
        self.shared.generation
    }

    /// Why the link died, once it has.
    pub(crate) fn death(&self) -> Option<String> {
        self.shared.pending.lock().dead.clone()
    }

    /// Sends `op` and waits for its answer, parsed as `T`; `timeout` bounds the wait (the
    /// handshake), after which the link dies. An answer that doesn't parse as `T` is a
    /// protocol failure too.
    pub(crate) fn call<T: DeserializeOwned>(
        &self,
        op: Op,
        timeout: Option<Duration>,
    ) -> Result<T, CallError> {
        let (tx, rx) = mpsc::channel();
        let name = op.name();
        self.send(op, Waiter::Call(tx))?;
        self.wait(name, &rx, timeout)
    }

    /// Starts a watch: `events` receives its batches from the answer on.
    pub(crate) fn call_watch(
        &self,
        op: Op,
        events: Sender<WatchEvent>,
    ) -> Result<WatchReply, CallError> {
        let (tx, rx) = mpsc::channel();
        let name = op.name();
        self.send(op, Waiter::Watch { reply: tx, events })?;
        self.wait(name, &rx, None)
    }

    /// Sends `op` without waiting for its answer (best effort: nothing is sent on a dead link).
    pub(crate) fn notify(&self, op: Op) {
        let _ = self.send(op, Waiter::Discard);
    }

    /// Stops routing the batches of server watch `watch`.
    pub(crate) fn unroute(&self, watch: u32) {
        self.shared.routes.lock().remove(&watch);
    }

    /// Kills the link as a crash would (see the module docs).
    pub(crate) fn die(&self, reason: String) {
        self.shared.die(reason);
    }

    fn send(&self, op: Op, waiter: Waiter) -> Result<(), CallError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut line = match serde_json::to_vec(&Request { id, op }) {
            Ok(line) => line,
            Err(error) => {
                return Err(CallError::Remote(WireError::protocol(format!(
                    "couldn't encode the request: {error}"
                ))));
            }
        };
        line.push(b'\n');
        {
            let mut pending = self.shared.pending.lock();
            if let Some(reason) = &pending.dead {
                return Err(CallError::Dead(reason.clone()));
            }
            pending.calls.insert(id, waiter);
        }
        let written = match self.shared.writer.lock().as_mut() {
            Some(writer) => writer.write_all(&line).and_then(|()| writer.flush()),
            None => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "dv's stdin is closed",
            )),
        };
        if let Err(error) = written {
            // The waiter is answered by the death.
            self.shared.die(format!("writing to dv failed: {error}"));
        }
        Ok(())
    }

    fn wait<T: DeserializeOwned>(
        &self,
        name: &str,
        rx: &Receiver<Reply>,
        timeout: Option<Duration>,
    ) -> Result<T, CallError> {
        let reply = match timeout {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(timeout) => rx.recv_timeout(timeout),
        };
        let reply = match reply {
            Ok(reply) => reply,
            Err(RecvTimeoutError::Timeout) => {
                let reason = format!(
                    "dv didn't answer {name} within {} s",
                    timeout.unwrap_or_default().as_secs()
                );
                self.shared.die(reason.clone());
                return Err(CallError::Dead(reason));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(CallError::Dead(
                    self.death().unwrap_or_else(|| "the link closed".into()),
                ));
            }
        };
        match reply {
            Reply::Ok(raw) => {
                let parsed = match &raw {
                    Some(raw) => serde_json::from_str(raw.get()),
                    None => serde_json::from_value(serde_json::Value::Null),
                };
                parsed.map_err(|error| {
                    let reason = format!("dv answered {name} with something else: {error}");
                    self.shared.die(reason.clone());
                    CallError::Dead(reason)
                })
            }
            Reply::Err(error) => Err(CallError::Remote(error)),
            Reply::Dead(reason) => Err(CallError::Dead(reason)),
        }
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        // Closing stdin ends the server; its process gets a few seconds to exit before it is
        // killed, waited for off this thread.
        drop(self.shared.writer.lock().take());
        if let Some(child) = self.shared.child.lock().take() {
            reap(child, Duration::from_secs(5));
        }
    }
}

impl LinkShared {
    fn is_dead(&self) -> bool {
        self.pending.lock().dead.is_some()
    }

    /// The death (module docs): kills the server's process, tells the connection, fails every
    /// call in flight with `reason`, drops the watches' routes and closes the server's stdin.
    /// Only the first death counts. The connection hears before the calls, so a call that fails
    /// sees its new state.
    fn die(&self, reason: String) {
        let calls = {
            let mut pending = self.pending.lock();
            if pending.dead.is_some() {
                return;
            }
            log::warn!("the connection to dv died: {reason}");
            pending.dead = Some(reason.clone());
            std::mem::take(&mut pending.calls)
        };
        // The process before anything waits on the writer: a write blocked on a full pipe
        // holds it until the write fails.
        if let Some(mut child) = self.child.lock().take() {
            let _ = child.kill();
            reap(child, Duration::ZERO);
        }
        if let Some(connection) = self.connection.upgrade() {
            connection.link_died(self.generation, &reason);
        }
        for (_, waiter) in calls {
            match waiter {
                Waiter::Call(reply) | Waiter::Watch { reply, .. } => {
                    let _ = reply.send(Reply::Dead(reason.clone()));
                }
                Waiter::Discard => {}
            }
        }
        // A watch's delivery ends once every sender of its channel is gone; a dead link keeps
        // none.
        self.routes.lock().clear();
        drop(self.writer.lock().take());
    }

    /// Routes one line (module docs); an `Err` is a protocol failure.
    fn route(&self, text: &[u8], answered: &mut bool) -> Result<(), String> {
        let incoming = match serde_json::from_slice::<Incoming>(text) {
            Ok(incoming) => incoming,
            Err(error) if !*answered => {
                self.note(format!("stdout: {}", diagnostic_text(text)));
                log::debug!("ignored a line before dv answered: {error}");
                return Ok(());
            }
            Err(error) => {
                return Err(format!(
                    "dv printed a line that isn't the protocol ({error}): {}",
                    excerpt(text)
                ));
            }
        };
        match (incoming.id, incoming.watch, incoming.event) {
            (Some(id), _, _) => {
                let waiter = self.pending.lock().calls.remove(&id);
                let Some(waiter) = waiter else {
                    if !*answered {
                        self.note(format!("stdout: {}", diagnostic_text(text)));
                        return Ok(());
                    }
                    return Err(format!("dv answered a request it wasn't sent (id {id})"));
                };
                *answered = true;
                let reply = match incoming.err {
                    Some(error) => Reply::Err(error),
                    None => Reply::Ok(incoming.ok),
                };
                match waiter {
                    Waiter::Call(tx) => {
                        let _ = tx.send(reply);
                    }
                    Waiter::Watch { reply: tx, events } => {
                        if let Reply::Ok(Some(raw)) = &reply
                            && let Ok(head) = serde_json::from_str::<WatchReply>(raw.get())
                        {
                            // Checked under the lock the death takes before it clears the
                            // routes, so a dead link never keeps one.
                            let pending = self.pending.lock();
                            if pending.dead.is_none() {
                                self.routes.lock().insert(head.watch, events);
                            }
                        }
                        let _ = tx.send(reply);
                    }
                    Waiter::Discard => {}
                }
                Ok(())
            }
            (None, Some(watch), Some(event)) => {
                // A batch for a watch already stopped is the unwatch racing it: dropped.
                if let Some(events) = self.routes.lock().get(&watch) {
                    let _ = events.send(event);
                }
                Ok(())
            }
            _ if !*answered => {
                self.note(format!("stdout: {}", diagnostic_text(text)));
                Ok(())
            }
            _ => Err(format!(
                "dv printed a line that is neither a reply nor a watch batch: {}",
                excerpt(text)
            )),
        }
    }

    fn note(&self, line: String) {
        push_diagnostic(&self.diagnostics, line);
    }

    /// Why the output ended: the server's exit status when its process has one by now.
    fn ended(&self) -> String {
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            let status = match self.child.lock().as_mut() {
                None => return "dv's output ended".into(),
                Some(child) => child.try_wait(),
            };
            match status {
                Ok(Some(status)) => return format!("dv exited ({status})"),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => return "dv's output ended".into(),
            }
        }
    }
}

/// The reader thread (module docs).
fn read_lines(reader: Box<dyn Read + Send>, shared: &LinkShared) {
    let mut reader = BufReader::with_capacity(64 * 1024, reader);
    let mut line = Vec::new();
    let mut answered = false;
    let mut after_death = 0;
    let reason = loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break shared.ended(),
            Ok(_) => {}
            Err(error) => break format!("reading from dv failed: {error}"),
        }
        if shared.is_dead() {
            // Dead: whatever the server still prints is nobody's. What a server that never
            // answered printed may say why (a `hello` written into its closed stdin can beat
            // its output here), so the diagnostics keep the first lines of it.
            let text = trim_line(&line);
            if answered || after_death == STRAY_AFTER_DEATH {
                return;
            }
            after_death += 1;
            if !text.is_empty() {
                shared.note(format!("stdout: {}", diagnostic_text(text)));
            }
            continue;
        }
        let text = trim_line(&line);
        if text.is_empty() {
            continue;
        }
        if let Err(reason) = shared.route(text, &mut answered) {
            break reason;
        }
    };
    shared.die(reason);
}

/// The process watcher (module docs): the server's exit is the link's death. It stops once the
/// link is dead or its process was taken to be reaped.
fn watch_child(shared: &LinkShared) {
    loop {
        std::thread::sleep(CHILD_POLL);
        if shared.is_dead() {
            return;
        }
        let status = match shared.child.lock().as_mut() {
            None => return,
            Some(child) => child.try_wait(),
        };
        match status {
            Ok(None) => {}
            Ok(Some(status)) => {
                shared.die(format!("dv exited ({status})"));
                return;
            }
            Err(error) => {
                log::debug!("couldn't check on dv's process: {error}");
                return;
            }
        }
    }
}

/// Keeps the server's stderr lines in the diagnostics until it closes.
fn drain_stderr(stderr: Box<dyn Read + Send>, diagnostics: &Mutex<VecDeque<String>>) {
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let text = diagnostic_text(trim_line(&line));
                if !text.is_empty() {
                    log::debug!("dv backend: {text}");
                    push_diagnostic(diagnostics, format!("stderr: {text}"));
                }
            }
        }
    }
}

pub(crate) fn push_diagnostic(diagnostics: &Mutex<VecDeque<String>>, line: String) {
    let mut lines = diagnostics.lock();
    if lines.len() == DIAGNOSTIC_LINES {
        lines.pop_front();
    }
    lines.push_back(line);
}

/// Bytes as diagnostic text: `wsl.exe` prints its own messages as UTF-16LE, whose NULs go.
fn diagnostic_text(bytes: &[u8]) -> String {
    let text: Vec<u8> = bytes.iter().copied().filter(|&b| b != 0).collect();
    String::from_utf8_lossy(&text).trim().to_string()
}

/// The start of a line, for a message.
fn excerpt(bytes: &[u8]) -> String {
    let text = diagnostic_text(bytes);
    match text.char_indices().nth(120) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text,
    }
}

/// Waits for `child` off this thread, killing it once `grace` has passed.
fn reap(mut child: Child, grace: Duration) {
    let reaped = std::thread::Builder::new()
        .name("dv-backend-reaper".into())
        .spawn(move || {
            let deadline = Instant::now() + grace;
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(_)) | Err(_) => return,
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        });
    if let Err(error) = reaped {
        log::warn!("couldn't start a thread to reap dv: {error}");
    }
}
