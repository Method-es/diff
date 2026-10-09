//! The session's connection to its repository's `dv backend --stdio` (PLAN §5.2), when the
//! review's backend is a remote one (a WSL distro's, a loopback's): its state, which the
//! status bar and the backend-down banner show (DESIGN §4.1, §4.4), and what a lost
//! connection means for the session.
//!
//! - **While it reconnects** every call fails at once with `BackendError::Disconnected`. Such a
//!   failure ([`Failure::disconnected`]) is no refresh failure: the session keeps what it
//!   shows, raises no banner, and starts no more loads until the connection is back
//!   ([`ReviewSession::is_disconnected`]).
//! - **When it is back**, the remote watch's synthetic rescan reads the list, the diffs and the
//!   fold again (`watch`); a watch that didn't survive starts again, its catch-up reading
//!   everything too, and the loads that waited start.
//! - **When it failed** for good, or found its distro stopped, the shell shows the banner; its
//!   Retry (or Start) is [`ReviewSession::retry_connection`], `Connection::retry` off the UI
//!   thread. Calls fail meanwhile without starting anything, and the session waits for the
//!   connection as while it reconnects, raising nothing of its own.

use std::sync::Arc;

use dv_backend::{Connection, ConnectionState};
use futures::StreamExt as _;
use gpui::{AppContext as _, Context};

use super::ReviewSession;
use crate::failure::Failure;

/// The session's side of its connection.
#[derive(Default)]
pub(super) struct Link {
    connection: Option<Arc<Connection>>,
    /// The state last heard.
    state: Option<ConnectionState>,
    /// Keeps the state coming ([`ReviewSession::follow_connection`]).
    _subscription: Option<dv_backend::Subscription>,
    _states: Option<gpui::Task<()>>,
    /// A call failed because the connection was down: loads wait for it to come back.
    pub(super) down: bool,
    /// A `Connection::retry` is running.
    retrying: Option<gpui::Task<()>>,
}

impl Link {
    pub(super) fn new(connection: Option<Arc<Connection>>) -> Self {
        Self {
            state: connection.as_ref().map(|connection| connection.state()),
            connection,
            ..Self::default()
        }
    }

    pub(super) fn is_settled(&self) -> bool {
        self.retrying.is_none()
    }
}

impl ReviewSession {
    /// Starts hearing the connection's state, on this session's executor.
    pub(super) fn follow_connection(&mut self, cx: &mut Context<Self>) {
        let Some(connection) = self.link.connection.clone() else {
            return;
        };
        let (sender, mut states) = futures::channel::mpsc::unbounded::<ConnectionState>();
        let subscription = connection.subscribe(move |state| {
            let _ = sender.unbounded_send(state.clone());
        });
        self.link._states = Some(cx.spawn(async move |this, cx| {
            while let Some(state) = states.next().await {
                if this
                    .update(cx, |this, cx| this.connection_changed(state, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        self.link._subscription = Some(subscription);
        // A change between the open and the subscription would otherwise go unheard.
        let now = connection.state();
        if self.link.state.as_ref() != Some(&now) {
            self.connection_changed(now, cx);
        }
    }

    /// The connection the review's backend goes through; `None` for a repository opened in
    /// process.
    pub fn connection(&self) -> Option<&Arc<Connection>> {
        self.link.connection.as_ref()
    }

    /// The connection's state as last heard; `None` without one.
    pub fn connection_state(&self) -> Option<&ConnectionState> {
        self.link.state.as_ref()
    }

    /// Where the backend runs, as the status bar names it: the connection's name (the distro,
    /// a loopback's name), else `Local`.
    pub fn backend_place(&self) -> String {
        match &self.link.connection {
            Some(connection) => connection.name().to_string(),
            None => "Local".to_string(),
        }
    }

    /// Whether a call failed because the connection was down and the session waits for it to
    /// come back (module docs).
    pub fn is_disconnected(&self) -> bool {
        self.link.down
    }

    /// Whether a [`ReviewSession::retry_connection`] is running.
    pub fn is_retrying_connection(&self) -> bool {
        self.link.retrying.is_some()
    }

    /// The banner's Retry once the connection failed: starts a server again off the UI
    /// thread (`Connection::retry` blocks until it answers or fails). Its state says how it
    /// went: connected, the session reads everything again; failed, the banner is back.
    pub fn retry_connection(&mut self, cx: &mut Context<Self>) {
        let Some(connection) = self.link.connection.clone() else {
            return;
        };
        if self.link.retrying.is_some() {
            return;
        }
        let job = cx.background_spawn(async move { connection.retry() });
        self.link.retrying = Some(cx.spawn(async move |this, cx| {
            let retried = job.await;
            this.update(cx, |this, cx| {
                this.link.retrying = None;
                if let Err(error) = retried {
                    log::warn!("review {}: retry failed: {error}", this.review.id);
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn connection_changed(&mut self, state: ConnectionState, cx: &mut Context<Self>) {
        let back = state == ConnectionState::Connected
            && self
                .link
                .state
                .as_ref()
                .is_some_and(|was| *was != ConnectionState::Connected);
        self.link.state = Some(state);
        if back {
            self.reconnected(cx);
        }
        cx.notify();
    }

    /// The connection is up again after a death or a retry (module docs).
    fn reconnected(&mut self, cx: &mut Context<Self>) {
        self.link.down = false;
        if !self.watch.is_running() {
            self.ensure_watch(cx);
        }
        self.requeue_wanted();
        self.pump(cx);
    }

    /// A call failed with `failure`: `None` when the connection wasn't lost under it (the
    /// caller shows it), else whether the connection is back already, so the call is worth
    /// making again now. One that isn't back yet leaves the session waiting for it
    /// ([`ReviewSession::reconnected`]). A call that failed while the connection is down for
    /// good (failed, or its distro stopped) counts as lost too: the banner says why.
    pub(super) fn lost_connection(&mut self, failure: &Failure) -> Option<bool> {
        let state = self
            .link
            .connection
            .as_ref()
            .map(|connection| connection.state());
        let down = matches!(
            state,
            Some(ConnectionState::Failed { .. } | ConnectionState::Stopped)
        );
        if !failure.disconnected && !down {
            return None;
        }
        let back = state == Some(ConnectionState::Connected);
        if !back {
            self.link.down = true;
        }
        Some(back)
    }
}
