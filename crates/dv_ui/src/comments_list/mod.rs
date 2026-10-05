//! The panel's comments (DESIGN §4.1 "Panel", PLAN §11 `comments_list`), under the file list:
//! "Open comments" with the number of threads still waiting (open, and addressed by the agent
//! but not yet resolved), one row per thread — its state, `path:line` and the first line of its
//! comment — and "Copy as prompt", which puts the open threads on the clipboard as Markdown
//! (through the clipboard seam, `crate::system`). An outdated thread (PLAN §8) is flagged and
//! shows its original first line. Resolved threads are counted at the end and listed on demand.
//!
//! A click on a row shows the thread in the stream ([`DiffPane::reveal_thread`]), which also
//! focuses it; the focused thread's row is highlighted.
//!
//! Automation targets: `comments_list`, `comments_list/copy`, `comments_list/resolved` (show or
//! hide the resolved threads) and `comments_list/<thread id>`.

mod render;

#[cfg(test)]
mod tests;

use dv_core::review::{Thread, ThreadId, ThreadState};
use gpui::{Context, Entity, Subscription};

use crate::diff_pane::DiffPane;
use crate::session::{Placement, ReviewSession};
use crate::thread::{first_line, location_label, prompt_markdown};

/// One thread as the list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentRow {
    pub id: ThreadId,
    pub state: ThreadState,
    /// Its anchored text no longer stands at its lines (or its file left the diff).
    pub outdated: bool,
    /// `path:31`, `path:12–14`.
    pub location: String,
    /// The comment's first line.
    pub summary: String,
    /// The first anchored line as it was, for an outdated thread.
    pub snippet: Option<String>,
}

/// The threads of `session`'s review in stream order (by file, then line; threads whose file
/// left the diff last): the unresolved ones, then, with `resolved`, the resolved ones.
pub fn rows(session: &ReviewSession, resolved: bool) -> Vec<CommentRow> {
    let mut threads: Vec<&Thread> = session
        .review()
        .live_threads()
        .filter(|thread| resolved || thread.state != ThreadState::Resolved)
        .collect();
    let order = |thread: &Thread| {
        let file = session
            .file_of(thread)
            .and_then(|entry| session.files().iter().position(|e| e.path == entry.path))
            .unwrap_or(usize::MAX);
        (
            thread.state == ThreadState::Resolved,
            file,
            thread.anchor.path.clone(),
            thread.anchor.start_line,
        )
    };
    threads.sort_by_key(|thread| order(thread));
    threads
        .into_iter()
        .map(|thread| {
            let outdated = session.placement(thread) == Placement::Outdated;
            CommentRow {
                id: thread.id.clone(),
                state: thread.state,
                outdated,
                location: location_label(thread),
                summary: first_line(&thread.body),
                snippet: outdated
                    .then(|| thread.anchor.text.first().cloned())
                    .flatten()
                    .map(|line| line.trim().to_string()),
            }
        })
        .collect()
}

/// The threads "Copy as prompt" hands an agent: the open ones, in stream order.
pub fn prompt_threads(session: &ReviewSession) -> Vec<&Thread> {
    rows(session, false)
        .iter()
        .filter(|row| row.state == ThreadState::Open)
        .filter_map(|row| session.review().thread(&row.id))
        .collect()
}

/// The panel's comments (see the module docs).
pub struct CommentsList {
    session: Option<Entity<ReviewSession>>,
    diff_pane: Entity<DiffPane>,
    show_resolved: bool,
    _session_events: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl CommentsList {
    pub fn new(diff_pane: Entity<DiffPane>, cx: &mut Context<Self>) -> Self {
        // The focused thread's row follows the stream.
        let subscriptions = vec![cx.observe(&diff_pane, |_, _, cx| cx.notify())];
        Self {
            session: None,
            diff_pane,
            show_resolved: false,
            _session_events: None,
            _subscriptions: subscriptions,
        }
    }

    /// Lists `session`'s threads (or nothing).
    pub fn set_session(&mut self, session: Option<Entity<ReviewSession>>, cx: &mut Context<Self>) {
        self._session_events = session
            .as_ref()
            .map(|session| cx.observe(session, |_, _, cx| cx.notify()));
        self.session = session;
        self.show_resolved = false;
        cx.notify();
    }

    /// The rows shown now.
    pub fn rows(&self, cx: &gpui::App) -> Vec<CommentRow> {
        self.session
            .as_ref()
            .map(|session| rows(session.read(cx), self.show_resolved))
            .unwrap_or_default()
    }

    /// Whether the resolved threads are listed too.
    pub fn shows_resolved(&self) -> bool {
        self.show_resolved
    }

    pub fn set_show_resolved(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.show_resolved != show {
            self.show_resolved = show;
            cx.notify();
        }
    }

    /// Shows `thread` in the stream (a click on its row).
    pub fn reveal(&mut self, thread: &ThreadId, cx: &mut Context<Self>) {
        self.diff_pane
            .update(cx, |pane, cx| pane.reveal_thread(thread, cx));
    }

    /// "Copy as prompt": the open threads as Markdown, on the clipboard (recorded under
    /// automation). Nothing happens without an open thread.
    pub fn copy_as_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        let text = {
            let session = session.read(cx);
            let threads = prompt_threads(session);
            if threads.is_empty() {
                return;
            }
            prompt_markdown(&session.target_label(), &threads)
        };
        crate::system::write_clipboard(text, cx);
    }
}
