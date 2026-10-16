//! The file finder (`Ctrl P`, PLAN §18.1, DESIGN §4.1 "File finder"): every file at the
//! review's new side, filtered as the file list filters, in an overlay hung under the title
//! bar's command field over the scrim, in the switcher's look. Picking one opens it whole in the
//! file view, a file the review changes too ("open the whole file").
//!
//! **The files.** The side's paths are read once per review open and round
//! (`ReviewSession::ensure_side_paths`) and joined with the review's own files (a file the
//! agent created since, a deleted one, which the file view shows at its old side), so thousands
//! of files list at once; filtering runs in memory on every keystroke ([`candidates`],
//! [`filter`]). While the side's paths are read, the review's files list alone.
//!
//! **Order.** A file passes when every word typed is in its path, ignoring case (the file
//! list's filter, `crate::file_list::matches_filter`). Files whose name holds every word come
//! before those that match only through their directories; among equal matches the review's
//! changed files (marked with their status letter) come first, then the rest by path.
//!
//! `↑`/`↓` move the selection (wrapping), `Enter` opens, `Esc` closes. The finder only reports
//! what was picked ([`FileFinderEvent`]); the shell opens it.
//!
//! Automation targets: `file_finder`, `file_finder/input`, `file_finder/row/<path>`.

mod render;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;

use dv_core::diff::FileStatus;
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, Focusable as _, Subscription,
    UniformListScrollHandle, Window,
};
use gpui_component::input::{InputEvent, InputState};

use crate::actions::file_finder::{Confirm, Dismiss, SelectNext, SelectPrevious};
use crate::session::ReviewSession;

/// The panel's width (the switcher's).
pub const FINDER_WIDTH: gpui::Pixels = gpui::px(560.);
/// The filter field's placeholder.
pub const PLACEHOLDER: &str = "Go to file";
/// How many rows `state.file_finder` reports.
pub const STATE_ROWS: usize = 20;

/// A file the finder can list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub path: String,
    /// The review's status for a file it changes.
    pub status: Option<FileStatus>,
    /// `path` and its name lowercased, for filtering.
    lower: String,
    name_at: usize,
}

impl Candidate {
    pub fn new(path: String, status: Option<FileStatus>) -> Self {
        let lower = path.to_lowercase();
        let name_at = lower.rfind('/').map_or(0, |at| at + 1);
        Self {
            path,
            status,
            lower,
            name_at,
        }
    }
}

/// The finder's files (module docs, "The files"): `side` (the new side's paths, sorted) joined
/// with `changed` (the review's files and their statuses), sorted by path, each once.
pub fn candidates(side: &[String], changed: &[(String, FileStatus)]) -> Vec<Candidate> {
    let statuses: HashMap<&str, FileStatus> = changed
        .iter()
        .map(|(path, status)| (path.as_str(), *status))
        .collect();
    let mut paths: Vec<&str> = side
        .iter()
        .map(String::as_str)
        .chain(changed.iter().map(|(path, _)| path.as_str()))
        .collect();
    paths.sort_unstable();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| Candidate::new(path.to_string(), statuses.get(path).copied()))
        .collect()
}

/// The indexes of the `candidates` that pass `query`, in the finder's order (module docs,
/// "Order").
pub fn filter(candidates: &[Candidate], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found: Vec<(u8, bool, usize)> = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| words.iter().all(|word| candidate.lower.contains(word)))
        .map(|(index, candidate)| {
            let name = &candidate.lower[candidate.name_at..];
            let in_name = words.iter().all(|word| name.contains(word));
            (u8::from(!in_name), candidate.status.is_none(), index)
        })
        .collect();
    // Candidates are sorted by path already, so the index orders the rest.
    found.sort_unstable();
    found.into_iter().map(|(_, _, index)| index).collect()
}

/// What the candidates are built from: the new side's paths (the session's own list) and the
/// review's files with their statuses.
type Sources = (Option<Arc<Vec<String>>>, Vec<(String, FileStatus)>);

/// What was picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileFinderEvent {
    /// Show this file in the file view.
    Open(String),
    /// Closed without picking (`Esc`).
    Dismiss,
}

/// The file finder (see the module docs).
pub struct FileFinder {
    session: Entity<ReviewSession>,
    input: Entity<InputState>,
    query: String,
    candidates: Arc<Vec<Candidate>>,
    /// Whether the side's paths had landed when `candidates` was built.
    complete: bool,
    /// What `candidates` was built from: the side's paths (the session's own list) and the
    /// review's files.
    sources: Sources,
    built: bool,
    /// Into `candidates`, in order.
    shown: Vec<usize>,
    /// Into `shown`.
    selected: usize,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FileFinderEvent> for FileFinder {}

impl FileFinder {
    /// A finder over `session`'s new side; its paths are read if they aren't.
    pub fn new(
        session: Entity<ReviewSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(PLACEHOLDER));
        session.update(cx, |session, cx| session.ensure_side_paths(cx));
        let subscriptions = vec![
            cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query_changed(cx);
                }
            }),
            // The side's paths land, or the review's files change.
            cx.observe(&session, |this, _, cx| this.rebuild(cx)),
        ];
        let mut this = Self {
            session,
            input,
            query: String::new(),
            candidates: Arc::new(Vec::new()),
            complete: false,
            sources: (None, Vec::new()),
            built: false,
            shown: Vec::new(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        this.rebuild(cx);
        this
    }

    /// Puts the keyboard in the filter field.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Whether the filter field has the keyboard.
    pub fn is_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.input.read(cx).focus_handle(cx).is_focused(window)
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// How many files pass the filter.
    pub fn count(&self) -> usize {
        self.shown.len()
    }

    /// The first `limit` paths shown, in order.
    pub fn rows(&self, limit: usize) -> Vec<&str> {
        self.shown
            .iter()
            .take(limit)
            .map(|&index| self.candidates[index].path.as_str())
            .collect()
    }

    /// The selected path.
    pub fn selected_path(&self) -> Option<&str> {
        self.shown
            .get(self.selected)
            .map(|&index| self.candidates[index].path.as_str())
    }

    /// Whether every file of the side is listed (its paths landed).
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Builds the candidates again when what they come from changed, keeping the selection.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let (side, same) = {
            let session = self.session.read(cx);
            let side = session.side_paths().cloned();
            let same_side = match (&side, &self.sources.0) {
                (Some(now), Some(was)) => Arc::ptr_eq(now, was),
                (None, None) => true,
                _ => false,
            };
            let files = session.target_files();
            let same_files = files.len() == self.sources.1.len()
                && files
                    .iter()
                    .zip(&self.sources.1)
                    .all(|(entry, (path, status))| entry.path == *path && entry.status == *status);
            (side, same_side && same_files)
        };
        // The session notifies for much else (a file landing, a highlight): the candidates are
        // built again only when what they come from changed.
        if same && self.built {
            return;
        }
        let changed: Vec<(String, FileStatus)> = self
            .session
            .read(cx)
            .target_files()
            .iter()
            .map(|entry| (entry.path.clone(), entry.status))
            .collect();
        let complete = side.is_some();
        let candidates = candidates(side.as_deref().map_or(&[], Vec::as_slice), &changed);
        self.sources = (side, changed);
        self.built = true;
        let keep = self.selected_path().map(str::to_string);
        self.candidates = Arc::new(candidates);
        self.complete = complete;
        self.shown = filter(&self.candidates, &self.query);
        self.selected = keep
            .and_then(|path| {
                self.shown
                    .iter()
                    .position(|&index| self.candidates[index].path == path)
            })
            .unwrap_or(0);
        self.scroll_to_selected();
        cx.notify();
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_string();
        if query == self.query {
            return;
        }
        self.query = query;
        self.shown = filter(&self.candidates, &self.query);
        self.selected = 0;
        self.scroll_to_selected();
        cx.notify();
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = ix.min(self.shown.len().saturating_sub(1));
        self.scroll_to_selected();
        cx.notify();
    }

    fn scroll_to_selected(&self) {
        if !self.shown.is_empty() {
            self.scroll
                .scroll_to_item(self.selected, gpui::ScrollStrategy::Nearest);
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.shown.len();
        if len > 0 {
            self.select((self.selected + 1) % len, cx);
        }
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.shown.len();
        if len > 0 {
            self.select((self.selected + len - 1) % len, cx);
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.pick(self.selected, cx);
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(FileFinderEvent::Dismiss);
    }

    /// Picks row `ix` (Enter on it, or a click).
    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(&index) = self.shown.get(ix) else {
            return;
        };
        self.selected = ix;
        cx.emit(FileFinderEvent::Open(self.candidates[index].path.clone()));
    }

    /// Picks the row showing `path` (automation's click on `file_finder/row/<path>`); `false`
    /// when it isn't shown.
    pub fn pick_path(&mut self, path: &str, cx: &mut Context<Self>) -> bool {
        match self
            .shown
            .iter()
            .position(|&index| self.candidates[index].path == path)
        {
            Some(ix) => {
                self.pick(ix, cx);
                true
            }
            None => false,
        }
    }
}
