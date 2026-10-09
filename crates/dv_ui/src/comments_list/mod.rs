//! The panel's comments (DESIGN §4.1 "Panel", PLAN §11 `comments_list`), under the file list:
//! "Open comments" with the number of threads still waiting (open, and addressed by the agent
//! but not yet resolved), one row per thread — its state, `path:line` where it shows now and
//! the first line of its comment — and "Copy as prompt", which puts the open threads on the
//! clipboard as Markdown (through the clipboard seam, `crate::system`). Addressed threads come
//! first, then open ones, each in stream order (PLAN §3 workflow A step 5). An outdated thread
//! (PLAN §8) is flagged and shows its first anchored line as it was; one the interdiff doesn't
//! show carries a quiet "all changes" tag and names where it stands in all changes. A thread
//! whose file's text hasn't landed yet shows the line it was written on, quietly, until it is
//! placed (every file a thread shows in loads at once, so that is brief), and "Copy as prompt"
//! says so beside it. Resolved threads are counted at the end and listed on demand.
//!
//! A click on a row shows the thread in the stream ([`DiffPane::reveal_thread`]), which also
//! focuses it, switching to all changes first for an "all changes" row; the focused thread's
//! row is highlighted.
//!
//! Automation targets: `comments_list`, `comments_list/copy`, `comments_list/resolved` (show or
//! hide the resolved threads) and `comments_list/<thread id>`.

mod render;

#[cfg(test)]
mod tests;

use dv_core::review::{Thread, ThreadId, ThreadState};
use gpui::{Context, Entity, Subscription};

use crate::diff_pane::DiffPane;
use crate::session::{Comparison, ReviewSession, ThreadPlacement};
use crate::thread::{first_line, prompt_markdown};

/// One thread as the list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentRow {
    pub id: ThreadId,
    pub state: ThreadState,
    /// Relocation can't find its anchored text, or its file left the diff (PLAN §8).
    pub outdated: bool,
    /// The interdiff on screen doesn't show it: an old-side thread, or its file isn't in it
    /// (the "all changes" tag).
    pub not_in_comparison: bool,
    /// Its file's text hasn't landed yet: `location` is where it was written, not where it is.
    pub pending: bool,
    /// `path:31`, `path:12–14` (`path:31 (old)` on the old side): where it stands now, else
    /// where it was anchored.
    pub location: String,
    /// The comment's first line.
    pub summary: String,
    /// The first anchored line as it was, for an outdated thread.
    pub snippet: Option<String>,
}

/// Where a state's threads go in the list: addressed first, then open, then resolved.
fn group(state: ThreadState) -> u8 {
    match state {
        ThreadState::Addressed => 0,
        ThreadState::Open => 1,
        ThreadState::Resolved => 2,
    }
}

/// The threads of `session`'s review: the addressed ones, then the open ones, each in stream
/// order (by file, then the line they show at; the ones the stream doesn't show last), then,
/// with `resolved`, the resolved ones.
pub fn rows(session: &ReviewSession, resolved: bool) -> Vec<CommentRow> {
    let mut threads: Vec<(&Thread, ThreadPlacement)> = session
        .review()
        .live_threads()
        .filter(|thread| resolved || thread.state != ThreadState::Resolved)
        .map(|thread| (thread, session.placement(thread)))
        .collect();
    let order = |thread: &Thread, placement: &ThreadPlacement| {
        let file = placement
            .file()
            .and_then(|path| session.files().iter().position(|e| e.path == path))
            .unwrap_or(usize::MAX);
        let line = placement
            .lines(&thread.anchor)
            .map_or(thread.anchor.start_line, |(start, _)| start);
        (group(thread.state), file, thread.anchor.path.clone(), line)
    };
    threads.sort_by_key(|(thread, placement)| order(thread, placement));
    threads
        .into_iter()
        .map(|(thread, placement)| {
            let outdated = placement.is_outdated();
            CommentRow {
                id: thread.id.clone(),
                state: thread.state,
                outdated,
                not_in_comparison: placement.is_not_in_comparison(),
                pending: placement.is_pending(),
                location: session.location(thread),
                summary: first_line(&thread.body),
                snippet: outdated
                    .then(|| thread.anchor.text.first().cloned())
                    .flatten()
                    .map(|line| line.trim().to_string()),
            }
        })
        .collect()
}

/// The threads "Copy as prompt" hands an agent: the open ones, in stream order, each with
/// where it stands now (`ReviewSession::location`, as its row names it). One not placed yet
/// says that its lines are the ones it was written on, so an agent doesn't take them for
/// where the code is now.
pub fn prompt_threads(session: &ReviewSession) -> Vec<(&Thread, String)> {
    rows(session, false)
        .into_iter()
        .filter(|row| row.state == ThreadState::Open)
        .filter_map(|row| Some((session.review().thread(&row.id)?, prompt_location(&row))))
        .collect()
}

/// Where "Copy as prompt" says `row`'s thread is: its row's location, and for one not placed
/// yet, that those are the lines it was written on.
pub fn prompt_location(row: &CommentRow) -> String {
    if row.pending {
        format!(
            "{} (where it was written; dv hasn't placed it yet)",
            row.location
        )
    } else {
        row.location.clone()
    }
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

    /// Shows `thread` in the stream (a click on its row): one the interdiff doesn't show is
    /// shown in all changes, which the stream switches to first.
    pub fn reveal(&mut self, thread: &ThreadId, cx: &mut Context<Self>) {
        if let Some(session) = &self.session {
            let elsewhere = {
                let session = session.read(cx);
                session
                    .review()
                    .thread(thread)
                    .is_some_and(|found| session.placement(found).is_not_in_comparison())
            };
            if elsewhere {
                session.update(cx, |session, cx| {
                    session.set_comparison(Comparison::All, cx)
                });
            }
        }
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
