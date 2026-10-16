//! `Ctrl F` in the stream (PLAN §18.1 "Find", DESIGN §4.1): the find bar over the main area
//! and its search over every file of the review, both sides, the text inside gaps included.
//!
//! **What it searches.** Every file the stream lists, in its order, through the diff the stream
//! shows for it ([`ReviewSession::shown_diff`], an interdiff's in an interdiff), matched as
//! [`search`] says. A file not loaded yet is loaded for the search
//! ([`ReviewSession::load_for_search`], after the files the viewport asks for) and searched as
//! it lands, so the count grows; until every file has landed the bar reads "3 of 17+". A file
//! without text to search (binary, too large to show, a submodule, an unmerged one, a failed
//! load) is skipped, and the bar says how many ("2 files not searched"); one whose content
//! didn't change shows a notice, not its lines, and isn't searched either. A file's matches are
//! kept while the diff they were found in is the one shown, so a file that lands costs only its
//! own search.
//!
//! **The current match.** Typing (once it pauses for [`FIND_DEBOUNCE`]) searches again and goes
//! to the first match at or after the cursor's row (else the current file's top), in the
//! stream's row order ([`Hit::row`]); while the files after it are still loading, it waits for
//! one there rather than jumping behind the cursor, and wraps to the first match only once every
//! file has been searched. `Enter` / `Shift Enter` go to the next / previous one, wrapping; an
//! `Enter` while typing hasn't been searched yet searches it first and goes to its first match.
//! Going to a match opens its file when it is collapsed, opens the gap that hides it around it
//! ([`dv_core::diff::reveal_in_gap`], [`CONTEXT_LINES`] either side), scrolls it to the middle
//! of the stream unless it is in view below the pinned header and the find bar, and puts the
//! cursor on its line and the caret at its start. A match the agent's edit took away hands its
//! place to the next one without moving the stream.
//!
//! **Closing.** `Esc` in the field (or in the stream, DESIGN §5) closes the bar; the keyboard
//! goes back to the stream with the cursor on the current match's line. Matches paint only while
//! the bar is open.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use dv_core::diff::{FileDiff, reveal_in_gap};
use dv_core::review::AnchorSide;
use gpui::{AppContext as _, Context, Entity, Focusable as _, Subscription, Task, Window};
use gpui_component::input::{InputEvent, InputState};

use super::rows::{CONTEXT_LINES, LinePos, Row, find_row, heights, side_number};
use super::search::{Hit, Query, file_hits, row_of};
use super::select::TextPoint;
use super::{Cursor, DiffPane};
use crate::services::ViewMode;
use crate::session::{FileState, ReviewSession};

/// How long typing pauses before the query is searched for.
pub const FIND_DEBOUNCE: Duration = Duration::from_millis(120);

/// The find field's placeholder.
pub const FIND_PLACEHOLDER: &str = "Find in review";

/// The find bar's state (module docs).
#[derive(Default)]
pub(super) struct Find {
    pub open: bool,
    /// The field, made the first time the bar opens (it needs a window).
    pub input: Option<Entity<InputState>>,
    /// The query searched for (the field's text once typing paused).
    pub query: String,
    parsed: Option<Query>,
    /// Typing that hasn't paused yet.
    pending: Option<Task<()>>,
    files: HashMap<String, FileHits>,
    pub current: Option<(String, Hit)>,
    /// Where the current match is looked for from: a file's place in the list and a row there
    /// ([`Hit::row`]).
    origin: (usize, usize),
    /// Go to the current match as soon as there is one: the query changed.
    go_when_found: bool,
    pub complete: bool,
    pub skipped: usize,
    _subscription: Option<Subscription>,
}

/// One file's matches, for the diff and view mode they were found in.
struct FileHits {
    diff: Arc<FileDiff>,
    mode: ViewMode,
    hits: Vec<Hit>,
    /// Indexes into `hits` by line and side, for painting.
    by_line: HashMap<(usize, AnchorSide), Vec<usize>>,
}

impl FileHits {
    fn new(diff: Arc<FileDiff>, mode: ViewMode, query: &Query) -> Self {
        let hits = diff
            .as_text()
            .map(|text| file_hits(&text.lines, query, mode))
            .unwrap_or_default();
        let mut by_line: HashMap<(usize, AnchorSide), Vec<usize>> = HashMap::new();
        for (index, hit) in hits.iter().enumerate() {
            by_line.entry((hit.line, hit.side)).or_default().push(index);
        }
        Self {
            diff,
            mode,
            hits,
            by_line,
        }
    }
}

/// The find bar as `state.find` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindSummary {
    pub open: bool,
    pub query: String,
    /// The current match's place among them all, from 1.
    pub current: Option<usize>,
    pub count: usize,
    /// Every file has been searched (none is still loading).
    pub complete: bool,
    /// Files without text to search.
    pub skipped: usize,
}

impl FindSummary {
    /// What the bar says beside the field: nothing for an empty query, "Searching…" before a
    /// first match while files still load, "No results", else "3 of 17" ("3 of 17+" while files
    /// still load).
    pub fn label(&self) -> Option<String> {
        if self.query.is_empty() {
            return None;
        }
        let more = if self.complete { "" } else { "+" };
        Some(match (self.count, self.current) {
            (0, _) if !self.complete => "Searching…".to_string(),
            (0, _) => "No results".to_string(),
            (count, Some(current)) => format!("{current} of {count}{more}"),
            (count, None) => format!("{count}{more}"),
        })
    }

    /// "2 files not searched", when files were skipped.
    pub fn skipped_label(&self) -> Option<String> {
        match self.skipped {
            0 => None,
            1 => Some("1 file not searched".to_string()),
            n => Some(format!("{n} files not searched")),
        }
    }
}

impl DiffPane {
    /// The find bar, or `None` with no review open (`state.find`).
    pub fn find_summary(&self) -> Option<FindSummary> {
        self.session.as_ref()?;
        let find = &self.find;
        let current = find.current.as_ref().and_then(|(path, hit)| {
            let mut before = 0;
            for entry in self.listed_paths() {
                let hits = self.find.files.get(entry).map_or(&[][..], |f| &f.hits[..]);
                if entry == path {
                    return hits.iter().position(|h| h == hit).map(|at| before + at + 1);
                }
                before += hits.len();
            }
            None
        });
        Some(FindSummary {
            open: find.open,
            query: find.query.clone(),
            current,
            count: find.files.values().map(|file| file.hits.len()).sum(),
            complete: find.complete,
            skipped: find.skipped,
        })
    }

    /// Whether the find field has the keyboard.
    pub fn is_find_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.find
            .input
            .as_ref()
            .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window))
    }

    /// The paths the stream lists, in order (not the "Not in the diff" section's).
    fn listed_paths(&self) -> impl Iterator<Item = &str> {
        self.blocks
            .iter()
            .map(|block| block.path.as_str())
            .filter(|path| *path != super::SECTION)
    }

    /// Whether code paints find matches: the bar is open with a query.
    pub(super) fn paints_matches(&self) -> bool {
        self.find.open && self.find.parsed.is_some()
    }

    /// The matches on line `line` (an index into `path`'s lines) on `side`, each with whether it
    /// is the current one.
    pub(super) fn matches_on(
        &self,
        path: &str,
        line: usize,
        side: AnchorSide,
    ) -> Vec<(Range<usize>, bool)> {
        let Some(file) = self.find.files.get(path) else {
            return Vec::new();
        };
        let current = self
            .find
            .current
            .as_ref()
            .filter(|(at, _)| at == path)
            .map(|(_, hit)| hit);
        file.by_line
            .get(&(line, side))
            .into_iter()
            .flatten()
            .map(|&index| {
                let hit = &file.hits[index];
                (hit.range.clone(), current == Some(hit))
            })
            .collect()
    }

    /// `Ctrl F`: opens the bar with its field focused (the selected text in it when it is on
    /// one line), or, open, focuses the field again with its text selected.
    pub(super) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.is_none() {
            return;
        }
        let input = match self.find.input.clone() {
            Some(input) => input,
            None => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(FIND_PLACEHOLDER));
                self.find._subscription =
                    Some(cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            this.find_typed(cx);
                        }
                    }));
                self.find.input = Some(input.clone());
                input
            }
        };
        let prefill = self.one_line_selection();
        self.find.open = true;
        if let Some(text) = &prefill {
            input.update(cx, |input, cx| input.set_value(text.clone(), window, cx));
        }
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        let text = input.read(cx).value().to_string();
        self.find.pending = None;
        if text != self.find.query {
            self.apply_find_query(text, cx);
        } else if self.find.current.is_none() {
            // Opened again on the same query: from the cursor, as a new search.
            self.find.origin = self.find_origin(cx);
            self.find.go_when_found = true;
            self.search_whole_review(cx);
            self.refresh_find(cx);
        }
        cx.notify();
    }

    /// The field's text changed: searched once typing pauses.
    fn find_typed(&mut self, cx: &mut Context<Self>) {
        if !self.find.open {
            return;
        }
        self.find.pending = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FIND_DEBOUNCE).await;
            this.update(cx, |this, cx| {
                this.find.pending = None;
                let text = this
                    .find
                    .input
                    .as_ref()
                    .map(|input| input.read(cx).value().to_string())
                    .unwrap_or_default();
                this.apply_find_query(text, cx);
            })
            .ok();
        }));
    }

    /// Whether typing in the field hasn't been searched for yet (automation's `settled`).
    pub(super) fn find_pending(&self) -> bool {
        self.find.pending.is_some()
    }

    /// Searches for `text` from the cursor's line (module docs).
    fn apply_find_query(&mut self, text: String, cx: &mut Context<Self>) {
        if text == self.find.query {
            return;
        }
        self.find.parsed = Query::new(&text);
        self.find.query = text;
        self.find.files.clear();
        self.find.current = None;
        self.find.origin = self.find_origin(cx);
        self.find.go_when_found = true;
        self.search_whole_review(cx);
        self.refresh_find(cx);
        cx.notify();
    }

    /// Has every file load while the bar is open with a query.
    fn search_whole_review(&mut self, cx: &mut Context<Self>) {
        let on = self.find.open && self.find.parsed.is_some();
        if let Some(session) = self.session.clone() {
            session.update(cx, |session, cx| session.load_for_search(on, cx));
        }
    }

    /// Where a new search looks for its first match from: the cursor's row, else the top of
    /// the current file.
    fn find_origin(&self, cx: &gpui::App) -> (usize, usize) {
        let Some(session) = self.session.as_ref() else {
            return (0, 0);
        };
        let files = session.read(cx).files();
        let place = |path: &str| files.iter().position(|entry| entry.path == path);
        if let Some(cursor) = &self.cursor
            && let Some(file) = place(&cursor.path)
        {
            let row = self
                .lines(&cursor.path)
                .and_then(|lines| row_of(lines, self.mode, cursor.pos.line, cursor.pos.side))
                .unwrap_or(cursor.pos.line);
            return (file, row);
        }
        self.current_file
            .as_deref()
            .and_then(place)
            .map_or((0, 0), |file| (file, 0))
    }

    /// Searches the files whose shown diff changed (or every file, for a new query or view
    /// mode), works out what is complete and skipped, and keeps the current match (or finds
    /// one). Runs whenever a file lands, the list changes or the view mode does.
    pub(super) fn refresh_find(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(query) = self.find.parsed.clone().filter(|_| self.find.open) else {
            self.find.files.clear();
            self.find.current = None;
            self.find.complete = true;
            self.find.skipped = 0;
            return;
        };
        let (complete, skipped) =
            search_files(&mut self.find.files, session.read(cx), &query, self.mode);
        self.find.complete = complete;
        self.find.skipped = skipped;
        let (current, go) = {
            let files = session.read(cx).files();
            let place = |path: &str| files.iter().position(|entry| entry.path == path);
            let mut current = self.find.current.clone();
            let stale = current.as_ref().and_then(|(path, hit)| {
                let kept = self
                    .find
                    .files
                    .get(path)
                    .is_some_and(|file| file.hits.contains(hit));
                (!kept).then(|| (place(path).unwrap_or(0), hit.row))
            });
            if let Some(from) = stale {
                current = self.first_hit_from(from, files, true);
            }
            let mut go = false;
            if current.is_none() {
                // At or after the origin; round to the first match only once nothing after it
                // can still land.
                current = self
                    .first_hit_from(self.find.origin, files, false)
                    .or_else(|| {
                        complete
                            .then(|| self.first_hit_from(self.find.origin, files, true))
                            .flatten()
                    });
                go = current.is_some() && self.find.go_when_found;
            }
            (current, go)
        };
        self.find.current = current.clone();
        if go && let Some((path, hit)) = current {
            self.find.go_when_found = false;
            self.go_to_hit(&path, &hit, cx);
        }
        cx.notify();
    }

    /// The first match at or after row `row` of the `file`-th listed file, on through the later
    /// files and, with `wrap`, round to the earlier ones and the file's own earlier rows.
    fn first_hit_from(
        &self,
        (file, row): (usize, usize),
        files: &[dv_core::diff::FileEntry],
        wrap: bool,
    ) -> Option<(String, Hit)> {
        let count = files.len();
        let hits = |index: usize| {
            self.find
                .files
                .get(&files[index].path)
                .map_or(&[][..], |file| &file.hits[..])
        };
        let found = |index: usize, hit: &Hit| (files[index].path.clone(), hit.clone());
        if file < count
            && let Some(hit) = hits(file).iter().find(|hit| hit.row >= row)
        {
            return Some(found(file, hit));
        }
        let later =
            (file + 1..count).find_map(|index| hits(index).first().map(|h| found(index, h)));
        if later.is_some() || !wrap {
            return later;
        }
        (0..=file.min(count.saturating_sub(1)))
            .find_map(|index| hits(index).first().map(|hit| found(index, hit)))
    }

    /// `Enter` / `Shift Enter` (and the bar's arrows): the next / previous match, wrapping. Typing
    /// not searched yet is searched first, and this goes to its first match.
    pub(super) fn step_find(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.find.pending.take().is_some() {
            let text = self
                .find
                .input
                .as_ref()
                .map(|input| input.read(cx).value().to_string())
                .unwrap_or_default();
            if text != self.find.query {
                self.apply_find_query(text, cx);
                return;
            }
        }
        let Some(summary) = self.find_summary().filter(|summary| summary.count > 0) else {
            return;
        };
        if summary.current.is_none() {
            let first = self.session.clone().and_then(|session| {
                self.first_hit_from(self.find.origin, session.read(cx).files(), true)
            });
            if let Some((path, hit)) = first {
                self.find.current = Some((path.clone(), hit.clone()));
                self.find.go_when_found = false;
                self.go_to_hit(&path, &hit, cx);
            }
            return;
        }
        let count = summary.count;
        let next = match summary.current {
            Some(current) if forward => current % count,
            Some(current) => (current + count - 2) % count,
            None => 0,
        };
        let mut at = next;
        let mut found = None;
        for path in self.listed_paths() {
            let hits = self.find.files.get(path).map_or(&[][..], |f| &f.hits[..]);
            if at < hits.len() {
                found = Some((path.to_string(), hits[at].clone()));
                break;
            }
            at -= hits.len();
        }
        if let Some((path, hit)) = found {
            self.find.current = Some((path.clone(), hit.clone()));
            self.find.go_when_found = false;
            self.go_to_hit(&path, &hit, cx);
        }
    }

    /// Shows `hit` of `path` (module docs, "The current match").
    fn go_to_hit(&mut self, path: &str, hit: &Hit, cx: &mut Context<Self>) {
        let Some(&block) = self.index.get(path) else {
            return;
        };
        let collapsed = matches!(
            self.blocks[block].rows.first(),
            Some(Row::Header {
                collapsed: true,
                ..
            })
        );
        if collapsed {
            self.set_open(path, true, cx);
        }
        let pos = LinePos {
            line: hit.line,
            side: hit.side,
        };
        let shown = self.lines(path).is_some_and(|lines| {
            find_row(&self.blocks[block].rows, lines, self.mode, pos).is_some()
        });
        if !shown {
            let segment = self.blocks[block].rows.iter().find_map(|row| match row {
                Row::Gap(gap) if gap.lines.contains(&hit.line) => Some(gap.segment),
                _ => None,
            });
            if let (Some(segment), Some(ui)) = (segment, self.files.get_mut(path)) {
                let around = hit.line.saturating_sub(CONTEXT_LINES)..hit.line + CONTEXT_LINES + 1;
                ui.segments = reveal_in_gap(&ui.segments, segment, around);
                self.refresh_block(block, cx);
                self.after_rows_changed(cx);
            }
        }
        let row = self
            .lines(path)
            .and_then(|lines| find_row(&self.blocks[block].rows, lines, self.mode, pos));
        if let Some(row) = row {
            self.centre_item(self.starts[block] + row);
        }
        let number = self
            .line(path, hit.line)
            .and_then(|line| side_number(line, hit.side));
        self.cursor = Some(Cursor {
            path: path.to_string(),
            pos,
        });
        self.selection = None;
        self.menu = None;
        if self.mode == ViewMode::Split {
            self.chosen_side = hit.side;
        }
        self.caret = number.map(|line| TextPoint {
            path: path.to_string(),
            side: hit.side,
            line,
            offset: hit.range.start,
        });
        self.viewport_changed(cx);
        cx.notify();
    }

    /// Scrolls item `item` to the middle of the stream, unless it is in view below the pinned
    /// header and the find bar already.
    fn centre_item(&mut self, item: usize) {
        let height = self.viewport_height();
        let size = self.item_height(item);
        // The find bar hangs over the stream's top while it is open.
        let clear = if self.find.open {
            self.find_bar_bottom().max(heights::HEADER)
        } else {
            heights::HEADER
        };
        if let Some(y) = self.offset_of(item, height * 2.)
            && y >= clear
            && y + size <= height
        {
            return;
        }
        self.scroll_item_to(item, ((height - size) / 2.).max(clear));
    }

    /// Closes the bar (module docs, "Closing"), the cursor on the current match's line; `false`
    /// when it wasn't open. The caller gives the keyboard back to the stream.
    pub(super) fn close_find(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.find.open {
            return false;
        }
        self.find.open = false;
        self.find.pending = None;
        self.search_whole_review(cx);
        if let Some((path, hit)) = self.find.current.clone()
            && self.index.contains_key(&path)
        {
            self.cursor = Some(Cursor {
                path,
                pos: LinePos {
                    line: hit.line,
                    side: hit.side,
                },
            });
        }
        self.refresh_find(cx);
        cx.notify();
        true
    }

    /// `Esc` in the field: closes the bar and gives the keyboard back to the stream.
    pub(super) fn close_find_and_refocus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_find(cx);
        self.focus_handle.focus(window, cx);
    }

    /// Forgets the bar's search, closed (another review shows).
    pub(super) fn reset_find(&mut self) {
        let input = self.find.input.take();
        let subscription = self.find._subscription.take();
        self.find = Find {
            input,
            _subscription: subscription,
            complete: true,
            ..Find::default()
        };
    }
}

/// Searches every listed file of `session` for `query` in `mode` into `files`, keeping the
/// matches of a file whose shown diff is the one they were found in: whether every file has
/// been searched, and how many have no text to search (module docs).
fn search_files(
    files: &mut HashMap<String, FileHits>,
    session: &ReviewSession,
    query: &Query,
    mode: ViewMode,
) -> (bool, usize) {
    let mut complete = true;
    let mut skipped = 0;
    for entry in session.files() {
        let path = &entry.path;
        let state = session.file_state(path);
        let pending = matches!(
            state,
            Some(FileState::Unloaded | FileState::Loading { .. }) | None
        );
        complete &= !pending;
        let failed = matches!(state, Some(FileState::Failed(_)));
        let Some(diff) = session.shown_diff(path).filter(|_| !failed) else {
            skipped += usize::from(failed);
            continue;
        };
        match diff.as_text() {
            Some(text) if text.has_changes() => {
                let kept = files
                    .get(path)
                    .is_some_and(|file| Arc::ptr_eq(&file.diff, diff) && file.mode == mode);
                if !kept {
                    files.insert(path.clone(), FileHits::new(diff.clone(), mode, query));
                }
            }
            // A notice instead of lines: nothing shown to search.
            Some(_) => {}
            None => skipped += usize::from(!pending),
        }
    }
    // A file no longer listed, or no longer shown with the diff its matches came from.
    files.retain(|path, file| {
        session
            .shown_diff(path)
            .is_some_and(|diff| Arc::ptr_eq(&file.diff, diff))
    });
    (complete, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(count: usize, current: Option<usize>, complete: bool) -> FindSummary {
        FindSummary {
            open: true,
            query: "x".to_string(),
            current,
            count,
            complete,
            skipped: 0,
        }
    }

    #[test]
    fn the_count_says_more_may_come_while_files_load() {
        assert_eq!(
            summary(17, Some(3), true).label().as_deref(),
            Some("3 of 17")
        );
        assert_eq!(
            summary(17, Some(3), false).label().as_deref(),
            Some("3 of 17+")
        );
        assert_eq!(
            summary(0, None, false).label().as_deref(),
            Some("Searching…")
        );
        assert_eq!(
            summary(0, None, true).label().as_deref(),
            Some("No results")
        );
        let empty = FindSummary {
            query: String::new(),
            ..summary(0, None, true)
        };
        assert_eq!(empty.label(), None);
        let skipped = |n| FindSummary {
            skipped: n,
            ..summary(1, Some(1), true)
        };
        assert_eq!(skipped(0).skipped_label(), None);
        assert_eq!(
            skipped(1).skipped_label().as_deref(),
            Some("1 file not searched")
        );
        assert_eq!(
            skipped(2).skipped_label().as_deref(),
            Some("2 files not searched")
        );
    }
}
