//! Remote processes (docs/PLAN.md §5.1, §18.3): a `ProcessHandle` over the wire, for a process
//! the server runs where the repository lives.
//!
//! - A process's output arrives on a channel, which the reader thread routes the server's
//!   `proc` lines to, and is delivered to the sink on a thread of the process's own, in order,
//!   its exit last. The channel has no bound: the reader thread serves every call and can't
//!   wait for one sink. The server bounds what it queues for the client (`server::process`), so
//!   a sink must keep up, as a watch's must.
//! - A process belongs to the server that started it. When that server dies its route goes
//!   with the link, the channel closes, and the sink hears `ProcessEvent::Exit(None)`: the
//!   caller sees the process end and starts another once the connection is back. Nothing is
//!   started again by itself.
//! - While a process runs its server isn't closed for idleness, as for a watch: the
//!   connection counts it until its delivery ends (`super`'s module docs).
//! - Dropping the handle stops the delivery, kills the process (`proc_kill`) and drops its
//!   route, which closes the channel, then joins the delivery thread before it returns, so the
//!   sink is never called again and has been dropped — except when the handle is dropped from
//!   inside its own sink, where delivery stops after the sink returns.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;

use dv_core::backend::{
    BackendError, ProcessControl, ProcessEvent, ProcessHandle, ProcessSink, ProcessSpec, process,
};

use super::Connection;
use super::remote::Worktree;
use crate::proto::Op;

impl Connection {
    /// `Backend::spawn_stdio` in `worktree`, on the live server.
    pub(super) fn spawn(
        self: &Arc<Self>,
        worktree: &Worktree,
        spec: &ProcessSpec,
        sink: ProcessSink,
    ) -> Result<ProcessHandle, BackendError> {
        let _busy = self.busy();
        let link = self.link()?;
        let repo = worktree
            .handle_on(&link)
            .map_err(|error| self.error(error))?;
        let (events, rx) = mpsc::channel();
        let reply = link
            .call_spawn(
                Op::Spawn {
                    repo,
                    spec: spec.clone(),
                },
                events,
            )
            .map_err(|error| self.error(error))?;
        // Counted before the call stops counting, so no idle close comes between the two.
        let running = Running::new(self);
        let stop = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name("dv-remote-proc".into())
            .spawn({
                let (stop, ended) = (Arc::clone(&stop), Arc::clone(&ended));
                move || deliver(&rx, &stop, &ended, sink, running)
            });
        let thread = match thread {
            Ok(thread) => thread,
            Err(error) => {
                link.unroute_process(reply.process);
                link.notify(Op::ProcKill {
                    process: reply.process,
                });
                return Err(BackendError::StartProcess {
                    program: spec.program.clone(),
                    source: std::io::Error::other(format!(
                        "couldn't start the thread that delivers its output: {error}"
                    )),
                });
            }
        };
        Ok(ProcessHandle::new(
            reply.pid,
            RemoteProcess {
                connection: Arc::clone(self),
                generation: link.generation(),
                process: reply.process,
                program: spec.program.clone(),
                stop,
                ended,
                thread: Some(thread),
            },
        ))
    }

    /// Stops routing the output of process `process` on the server of `generation`, when that
    /// server is still the live one (a dead one's routes are gone with it).
    fn unroute_process(&self, generation: u64, process: u32) {
        if let Some(link) = self.live_link(generation) {
            link.unroute_process(process);
        }
    }
}

/// A process the connection counts as running (module docs) until this is dropped.
struct Running {
    connection: Weak<Connection>,
}

impl Running {
    fn new(connection: &Arc<Connection>) -> Self {
        connection.inner.lock().procs += 1;
        Self {
            connection: Arc::downgrade(connection),
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.upgrade() {
            let mut inner = connection.inner.lock();
            inner.procs = inner.procs.saturating_sub(1);
            // Its idle period starts now.
            inner.last_used = std::time::Instant::now();
        }
    }
}

/// A remote process's delivery thread: the sink hears its output until its exit, which a
/// closed channel is too (its server died), or until the handle drops.
fn deliver(
    rx: &Receiver<ProcessEvent>,
    stop: &AtomicBool,
    ended: &AtomicBool,
    mut sink: ProcessSink,
    _running: Running,
) {
    loop {
        let event = rx.recv().unwrap_or(ProcessEvent::Exit(None));
        if stop.load(Ordering::Acquire) {
            break;
        }
        let exit = matches!(event, ProcessEvent::Exit(_));
        if exit {
            ended.store(true, Ordering::Release);
        }
        sink(event);
        if exit {
            break;
        }
    }
    ended.store(true, Ordering::Release);
}

/// A remote process's [`ProcessControl`] (module docs).
struct RemoteProcess {
    connection: Arc<Connection>,
    /// The server that runs it.
    generation: u64,
    /// Its id on that server.
    process: u32,
    program: String,
    stop: Arc<AtomicBool>,
    /// Its exit was delivered, or its server is gone: writes fail at once.
    ended: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ProcessControl for RemoteProcess {
    fn write(&self, bytes: &[u8]) -> Result<(), BackendError> {
        if self.ended.load(Ordering::Acquire) {
            return Err(process::ended(&self.program));
        }
        let _busy = self.connection.busy();
        let Some(link) = self.connection.live_link(self.generation) else {
            return Err(process::ended(&self.program));
        };
        link.call::<()>(
            Op::ProcWrite {
                process: self.process,
                bytes: bytes.to_vec(),
            },
            None,
        )
        .map_err(|error| match self.connection.error(error) {
            // Ended on the server, which names it by its id: named here as locally.
            BackendError::Io { source, .. } if source.kind() == std::io::ErrorKind::BrokenPipe => {
                process::ended(&self.program)
            }
            error => error,
        })
    }

    fn kill(&self) {
        if !self.ended.load(Ordering::Acquire) {
            self.connection.notify(
                self.generation,
                Op::ProcKill {
                    process: self.process,
                },
            );
        }
    }
}

impl Drop for RemoteProcess {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.kill();
        // With the route gone the channel closes, which ends the delivery thread's wait.
        self.connection
            .unroute_process(self.generation, self.process);
        if let Some(thread) = self.thread.take() {
            // Dropped from inside the sink, the thread is this one: it stops when the sink
            // returns and sees `stop`, and joining it here would deadlock.
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}
