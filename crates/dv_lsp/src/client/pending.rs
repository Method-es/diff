//! A request in flight: [`Pending::wait`] blocks for its answer until its timeout, and dropping
//! it unanswered (a hover the pointer left) sends `$/cancelRequest` (docs/PLAN.md §18.4,
//! §18.5).

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::Shared;

/// Why a request has no answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RequestError {
    #[error("{method} got no answer within {} ms", .after.as_millis())]
    Timeout { method: String, after: Duration },
    /// The server ended (or was stopped) before it answered.
    #[error("the language server ended")]
    Exited,
    /// The server answered with an error.
    #[error("{method} failed: {message} ({code})")]
    Server {
        method: String,
        code: i64,
        message: String,
    },
    /// A request about a document the client hasn't opened: positions are converted with its
    /// text, and a server answers only about what it has.
    #[error("{0} isn't open")]
    NotOpen(String),
    /// The answer didn't parse as the request's result.
    #[error("{method} answered something dv can't read: {detail}")]
    Invalid { method: String, detail: String },
}

/// An LSP error response's `code` and `message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResponseError {
    pub code: i64,
    pub message: String,
}

/// What the dispatcher hands a waiting request.
pub(crate) type Reply = Result<Value, ResponseError>;

type Convert<T> = Box<dyn FnOnce(Value, &Shared) -> Result<T, RequestError> + Send>;

/// A request in flight (module docs).
#[must_use = "dropping a request cancels it"]
pub struct Pending<T> {
    state: PendingState<T>,
}

enum PendingState<T> {
    /// Sent; the dispatcher answers through `reply`.
    Sent {
        shared: Arc<Shared>,
        id: i64,
        method: String,
        reply: Receiver<Reply>,
        deadline: Instant,
        timeout: Duration,
        convert: Convert<T>,
    },
    /// Settled before it was sent (the server had ended, the document wasn't open), or already
    /// waited for.
    Settled(Option<RequestError>),
}

impl<T> Pending<T> {
    pub(crate) fn sent(
        shared: Arc<Shared>,
        id: i64,
        method: &str,
        reply: Receiver<Reply>,
        timeout: Duration,
        convert: Convert<T>,
    ) -> Self {
        Self {
            state: PendingState::Sent {
                shared,
                id,
                method: method.to_string(),
                reply,
                deadline: Instant::now() + timeout,
                timeout,
                convert,
            },
        }
    }

    pub(crate) fn failed(error: RequestError) -> Self {
        Self {
            state: PendingState::Settled(Some(error)),
        }
    }

    /// The request's id on the wire; `None` when it was never sent.
    pub fn id(&self) -> Option<i64> {
        match &self.state {
            PendingState::Sent { id, .. } => Some(*id),
            PendingState::Settled(_) => None,
        }
    }

    /// Blocks until the answer arrives or the request's timeout passes (counted from when it
    /// was sent); a timeout cancels it. Call it off the UI thread.
    pub fn wait(mut self) -> Result<T, RequestError> {
        match std::mem::replace(&mut self.state, PendingState::Settled(None)) {
            PendingState::Settled(error) => Err(error.unwrap_or(RequestError::Exited)),
            PendingState::Sent {
                shared,
                id,
                method,
                reply,
                deadline,
                timeout,
                convert,
            } => {
                let left = deadline.saturating_duration_since(Instant::now());
                match reply.recv_timeout(left) {
                    Ok(Ok(value)) => convert(value, &shared),
                    Ok(Err(error)) => Err(RequestError::Server {
                        method,
                        code: error.code,
                        message: error.message,
                    }),
                    Err(RecvTimeoutError::Timeout) => {
                        cancel(&shared, id);
                        Err(RequestError::Timeout {
                            method,
                            after: timeout,
                        })
                    }
                    Err(RecvTimeoutError::Disconnected) => Err(RequestError::Exited),
                }
            }
        }
    }
}

impl<T> Drop for Pending<T> {
    fn drop(&mut self) {
        if let PendingState::Sent { shared, id, .. } = &self.state {
            cancel(shared, *id);
        }
    }
}

/// Forgets request `id` and, when it was still unanswered, tells the server it is no longer
/// wanted.
fn cancel(shared: &Shared, id: i64) {
    if shared.state.lock().pending.remove(&id).is_some() {
        shared.notify("$/cancelRequest", json!({ "id": id }));
    }
}
