//! A request to a language server made off the UI thread, which the UI can abandon (a hover
//! the pointer left, PLAN §18.4): dropping the [`InFlight`] cancels it at once
//! (`$/cancelRequest`), even while a background thread blocks in `Pending::wait` on it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dv_lsp::{Canceller, Pending};
use gpui::Task;
use parking_lot::Mutex;

/// What the UI and the background thread share about one request.
#[derive(Default)]
struct Shared {
    abandoned: AtomicBool,
    canceller: Mutex<Option<Canceller>>,
}

/// The background thread's side: it hands over each request it sends ([`Arming::arm`]).
#[derive(Clone)]
pub struct Arming(Arc<Shared>);

impl Arming {
    /// Makes `pending` cancellable from the UI; `false` when the UI has abandoned the request
    /// already (the caller then drops `pending`, which cancels it).
    pub fn arm<T>(&self, pending: &Pending<T>) -> bool {
        let mut slot = self.0.canceller.lock();
        if self.0.abandoned.load(Ordering::SeqCst) {
            return false;
        }
        *slot = pending.canceller();
        true
    }

    /// Whether the UI has abandoned the request.
    pub fn is_abandoned(&self) -> bool {
        self.0.abandoned.load(Ordering::SeqCst)
    }
}

/// The UI's side of a request: its task, and dropping it abandons the request.
pub struct InFlight {
    shared: Arc<Shared>,
    _task: Task<()>,
}

impl InFlight {
    /// A request whose background side gets `arming`; `task` delivers its answer.
    pub fn new(make: impl FnOnce(Arming) -> Task<()>) -> Self {
        let shared = Arc::new(Shared::default());
        let task = make(Arming(shared.clone()));
        Self {
            shared,
            _task: task,
        }
    }
}

#[cfg(test)]
impl InFlight {
    /// A request with no task of its own, and its background side (tests).
    pub fn detached() -> (Self, Arming) {
        let mut arming = None;
        let in_flight = Self::new(|armed| {
            arming = Some(armed);
            Task::ready(())
        });
        let arming = arming.expect("new hands over the arming");
        (in_flight, arming)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        // Under the lock, so a request being armed now sees the flag or is cancelled here.
        let canceller = {
            let mut slot = self.shared.canceller.lock();
            self.shared.abandoned.store(true, Ordering::SeqCst);
            slot.take()
        };
        if let Some(canceller) = canceller {
            canceller.cancel();
        }
    }
}
