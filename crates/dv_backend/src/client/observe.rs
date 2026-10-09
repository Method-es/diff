//! A connection's state and who listens to it (docs/PLAN.md §5.2): [`ConnectionState`],
//! readable with `Connection::state` and observable with `Connection::subscribe`.
//!
//! Every change is sent, in order, to a notifier thread of the connection's own, which calls
//! the callbacks: never on the thread that changed the state, never under the connection's
//! locks, so a callback may call back into the connection. The app's callback hands the state
//! to its own executor.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, OnceLock, Weak};
use std::thread::ThreadId;

use parking_lot::Mutex;

/// Where a connection stands (PLAN §5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// No server runs: none has started yet, or the last one was closed for idleness. The next
    /// call starts one, automatically (PLAN §5.2's connecting rule).
    Idle,
    /// The server is starting, or `hello` is on its way; calls wait for it.
    Connecting,
    /// The Linux build is being copied into the distro: `done` of `total` bytes. Calls wait
    /// for it on a first start, and fail at once with `BackendError::Disconnected` while a
    /// reconnect installs.
    Installing { done: u64, total: u64 },
    /// `hello` was answered; calls go through.
    Connected,
    /// The server died and attempt `attempt` (from 1) to start another is waiting out its
    /// backoff or under way. Calls fail at once with `BackendError::Disconnected`.
    Reconnecting { attempt: u32 },
    /// An automatic start found the host the server runs on stopped (a WSL distro the user
    /// shut down) and started nothing. Calls fail with `BackendError::Unavailable` until an
    /// explicit start (`Connection::retry`), which boots it.
    Stopped,
    /// The server couldn't start, or died and every attempt to start another failed; `error`
    /// says why. Calls fail with `BackendError::Unavailable` until `Connection::retry`.
    Failed { error: String },
}

impl fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectionState::Idle => f.write_str("idle"),
            ConnectionState::Connecting => f.write_str("connecting"),
            ConnectionState::Installing { done, total } => {
                write!(f, "installing dv ({done} of {total} bytes)")
            }
            ConnectionState::Connected => f.write_str("connected"),
            ConnectionState::Reconnecting { attempt } => {
                write!(f, "reconnecting (attempt {attempt})")
            }
            ConnectionState::Stopped => f.write_str("stopped"),
            ConnectionState::Failed { error } => write!(f, "failed: {error}"),
        }
    }
}

type Callback = dyn Fn(&ConnectionState) + Send + Sync;

/// A connection's subscribers.
#[derive(Default)]
pub(super) struct Observers {
    callbacks: Mutex<Vec<(u64, Arc<Callback>)>>,
    next: AtomicU64,
    /// Held while the callbacks run, so a [`Subscription`] dropped elsewhere waits out a
    /// delivery in progress.
    delivering: Mutex<()>,
    /// The notifier thread, which a callback dropping a subscription runs on.
    thread: OnceLock<ThreadId>,
}

impl Observers {
    pub(super) fn add(self: &Arc<Self>, callback: Arc<Callback>) -> Subscription {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.callbacks.lock().push((id, callback));
        Subscription {
            observers: Arc::downgrade(self),
            id,
        }
    }
}

/// Keeps a `Connection::subscribe` callback subscribed. Once dropped, the callback is never
/// called again (when dropped from inside a callback, after that callback returns).
#[must_use = "dropping a subscription unsubscribes"]
pub struct Subscription {
    observers: Weak<Observers>,
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let Some(observers) = self.observers.upgrade() else {
            return;
        };
        observers.callbacks.lock().retain(|(id, _)| *id != self.id);
        if observers.thread.get() != Some(&std::thread::current().id()) {
            drop(observers.delivering.lock());
        }
    }
}

impl fmt::Debug for Subscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subscription")
            .field("id", &self.id)
            .finish()
    }
}

/// The notifier thread: every state to every callback, in order, until the connection is
/// dropped (its sender with it).
pub(super) fn notify(observers: &Observers, states: Receiver<ConnectionState>) {
    let _ = observers.thread.set(std::thread::current().id());
    for state in states {
        let _delivering = observers.delivering.lock();
        let callbacks: Vec<Arc<Callback>> = observers
            .callbacks
            .lock()
            .iter()
            .map(|(_, callback)| Arc::clone(callback))
            .collect();
        for callback in callbacks {
            callback(&state);
        }
    }
}
