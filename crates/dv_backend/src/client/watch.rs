//! Remote watches (docs/PLAN.md §5.2, §5.3): a `WatchHandle` over the wire that outlives the
//! server it started on.
//!
//! - A watch's batches arrive on a channel, which the reader thread routes the server's batches
//!   to, and are delivered to the sink on a thread of the watch's own.
//! - The connection keeps every live watch ([`WatchEntry`]): when a new server answers after
//!   a death, each subscribes again on it (opening its worktree there first) and its channel
//!   gets one synthetic `WatchEvent { rescan: true, .. }`, since whatever changed while no
//!   server watched is lost.
//! - A watch that can't subscribe again (the server refuses: its worktree is gone, the OS
//!   won't watch it), or that the new server watches differently than its handle says (only
//!   the store now, its `worktree_error`, or the worktree again), ends instead: its channel
//!   closes, so the sink is dropped and its consumer, seeing the watch end, starts a new one,
//!   whose handle says what it covers. A server that dies while a watch subscribes leaves it
//!   for the next server.
//! - Dropping the handle stops routing, sends `unwatch`, drops the channel and joins the
//!   delivery thread before it returns, so the sink is never called again and has been
//!   dropped (the `WatchHandle` contract) — except when the handle is dropped from inside its
//!   own sink, where the watch stops after the sink returns. The connection keeps the watch
//!   until its `unwatch` is sent, so an idle close can't come between the two, and its idle
//!   period counts from the moment a watch ends (`super`'s module docs).
//!
//! An entry's lock is held while it subscribes, and a drop takes it too, so a watch dropped
//! during a resubscription never leaves a server watch behind.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use dv_core::backend::{BackendError, WatchEvent, WatchHandle, WatchSink, WatchSpec};
use parking_lot::Mutex;

use super::Connection;
use super::link::{CallError, Link};
use super::remote::Worktree;
use crate::proto::{Op, WatchReply};

/// A live watch, as the connection keeps it to subscribe it on every new server.
pub(super) struct WatchEntry {
    worktree: Arc<Worktree>,
    spec: WatchSpec,
    state: Mutex<EntryState>,
}

struct EntryState {
    /// The delivery thread's channel; `None` once the handle dropped, or the watch ended.
    events: Option<Sender<WatchEvent>>,
    /// The server watch delivering to it, when one is.
    server: Option<ServerWatch>,
    /// Its handle says the server watches the store alone (`worktree_error`).
    degraded: bool,
}

/// A watch on one server: its id there and that server's generation.
#[derive(Debug, Clone, Copy)]
struct ServerWatch {
    generation: u64,
    id: u32,
}

impl Connection {
    /// `Backend::watch` of `worktree` (whose root, for messages, is `root`), on the live
    /// server.
    pub(super) fn watch(
        self: &Arc<Self>,
        worktree: &Arc<Worktree>,
        root: &str,
        spec: WatchSpec,
        sink: WatchSink,
    ) -> Result<WatchHandle, BackendError> {
        let _busy = self.busy();
        let link = self.link()?;
        let (events, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name("dv-remote-watch".into())
            .spawn({
                let stop = Arc::clone(&stop);
                move || deliver(&rx, &stop, sink)
            })
            .map_err(|error| BackendError::Watch {
                path: root.to_string(),
                message: format!("couldn't start the watch's thread: {error}"),
            })?;
        let entry = Arc::new(WatchEntry {
            worktree: Arc::clone(worktree),
            spec,
            state: Mutex::new(EntryState {
                events: Some(events.clone()),
                server: None,
                degraded: false,
            }),
        });
        let key = self.next_watch.fetch_add(1, Ordering::Relaxed);
        // From here the guard's drop undoes everything.
        let guard = RemoteWatch {
            connection: Arc::clone(self),
            entry: Arc::clone(&entry),
            key,
            stop,
            thread: Some(thread),
        };
        let reply = {
            let mut state = entry.state.lock();
            // Kept before it subscribes: a server that dies meanwhile and its successor
            // must find it (the resubscription waits for this lock).
            self.watches.lock().insert(key, Arc::clone(&entry));
            match subscribe(&link, &entry, events) {
                Ok(reply) => {
                    state.server = Some(ServerWatch {
                        generation: link.generation(),
                        id: reply.watch,
                    });
                    state.degraded = reply.worktree_error.is_some();
                    reply
                }
                Err(error) => {
                    drop(state);
                    drop(guard);
                    return Err(self.error(error));
                }
            }
        };
        let handle = WatchHandle::new(guard);
        Ok(match reply.worktree_error {
            Some(message) => handle.with_worktree_error(message),
            None => handle,
        })
    }

    /// Subscribes every live watch on `link`, the new server, and gives each one synthetic
    /// rescan; ends a watch the server refuses, or watches differently than its handle says
    /// (module docs).
    pub(super) fn resubscribe(&self, link: &Link) {
        let entries: Vec<(u64, Arc<WatchEntry>)> = self
            .watches
            .lock()
            .iter()
            .map(|(key, entry)| (*key, Arc::clone(entry)))
            .collect();
        for (key, entry) in entries {
            let mut state = entry.state.lock();
            let Some(events) = state.events.clone() else {
                continue;
            };
            if state
                .server
                .is_some_and(|server| server.generation == link.generation())
            {
                continue;
            }
            let why = match subscribe(link, &entry, events.clone()) {
                Ok(reply) if reply.worktree_error.is_some() == state.degraded => {
                    state.server = Some(ServerWatch {
                        generation: link.generation(),
                        id: reply.watch,
                    });
                    let _ = events.send(WatchEvent {
                        rescan: true,
                        ..WatchEvent::default()
                    });
                    continue;
                }
                Ok(reply) => {
                    link.unroute(reply.watch);
                    link.notify(Op::Unwatch { watch: reply.watch });
                    match reply.worktree_error {
                        Some(error) => format!("it watches the store alone now: {error}"),
                        None => "it watches the worktree again".to_string(),
                    }
                }
                // The new server died too: the next one subscribes it.
                Err(CallError::Dead(_)) => {
                    state.server = None;
                    continue;
                }
                Err(CallError::Remote(error)) => error.into_backend(&self.name).to_string(),
            };
            log::warn!(
                "a watch of {} ended when {} came back ({why}); its consumer starts another",
                entry.worktree_path(),
                self.name,
            );
            state.server = None;
            state.events = None;
            self.unused_now();
            self.watches.lock().remove(&key);
        }
    }

    /// Stops `server`'s watch, when that server is still the live one (a dead one's routes are
    /// gone with it).
    fn unwatch(&self, server: ServerWatch) {
        let link = self.inner.lock().link.clone();
        if let Some(link) = link
            && link.generation() == server.generation
        {
            link.unroute(server.id);
            link.notify(Op::Unwatch { watch: server.id });
        }
    }
}

impl WatchEntry {
    fn worktree_path(&self) -> &str {
        self.worktree.path()
    }
}

/// Starts `entry`'s watch on `link`'s server, its batches going to `events`.
fn subscribe(
    link: &Link,
    entry: &WatchEntry,
    events: Sender<WatchEvent>,
) -> Result<WatchReply, CallError> {
    let repo = entry.worktree.handle_on(link)?;
    link.call_watch(
        Op::Watch {
            repo,
            spec: entry.spec,
        },
        events,
    )
}

/// A remote watch's delivery thread: the sink sees each batch until the watch stops.
fn deliver(rx: &Receiver<WatchEvent>, stop: &AtomicBool, mut sink: WatchSink) {
    while let Ok(event) = rx.recv() {
        if stop.load(Ordering::Acquire) {
            return;
        }
        sink(event);
    }
}

/// Stops a remote watch when dropped (module docs).
struct RemoteWatch {
    connection: Arc<Connection>,
    entry: Arc<WatchEntry>,
    key: u64,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for RemoteWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let server = {
            let mut state = self.entry.state.lock();
            state.events = None;
            state.server.take()
        };
        // With the route gone, nothing sends on the channel any more: the delivery thread's
        // `recv` ends once what is queued is skipped.
        if let Some(server) = server {
            self.connection.unwatch(server);
        }
        // Only now does the connection forget it, and its idle period starts.
        self.connection.unused_now();
        self.connection.watches.lock().remove(&self.key);
        if let Some(thread) = self.thread.take() {
            // Dropped from inside the sink, the thread is this one: it stops when the sink
            // returns and sees `stop`, and joining it here would deadlock.
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}
