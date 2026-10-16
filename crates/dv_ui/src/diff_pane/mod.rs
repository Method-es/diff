//! The diff stream (PLAN §6.7, DESIGN §4.1 "Diff"): one virtualised list over every file of the
//! open review, each file its header and then its rows ([`rows`]: code rows, gap rows, notices
//! and the next-file footer).
//!
//! **Rows and the list.** Each file is a block of rows in the session's file order; the gpui
//! `list` holds one item per row, plus a spacer at the end that lets the last file's header
//! reach the top. A block is rebuilt when its diff lands, a gap expands, it collapses or the view
//! mode changes, and only its rows are spliced, so the rest of the list keeps its measurements.
//! The list scrolls by item, so a file loading above the viewport moves nothing in it; a block
//! rebuilt under the reading position keeps it on the same line ([`RowKey`]), and a file that
//! loads again with other lines carries the reader's lines onto the new ones (`reload`). Rows
//! are built for the view mode in use only, from lines already loaded.
//!
//! **Heights.** Code rows, gaps, headers and notices have fixed heights ([`heights`]); cards,
//! the composer and an opened banner are measured, and their heights are kept as gpui last
//! reported them, since it reports none above the viewport. The spacer counts a row not
//! measured yet at the least any can be, so it is never too short for the last header.
//!
//! **Loading.** The stream asks the session for the files in and around its viewport
//! ([`ReviewSession::ensure_loaded`]); a file shows skeleton rows until its diff lands (and a
//! quiet "Loading…" in its header once that takes longer than [`SKELETON_FOR`]).
//!
//! **Cursor and current file.** A cursor line (and in split view a side) moves with the keys
//! (`nav`); `Shift ↑`/`Shift ↓` extend a selection from it within one file and one side. The
//! **current file** is the cursor's file while the cursor is on screen, else the file at the top
//! of the viewport, whose header stays pinned there. The panel's file list highlights it
//! ([`DiffPaneEvent::CurrentFileChanged`]) and shows a file through [`DiffPane::reveal_file`].
//!
//! **Viewed files** collapse to their header (DESIGN §4.1); the header's chevron opens one
//! again. Long lines clip with a fade and scroll sideways together, one offset for the whole
//! stream (`Shift`+wheel, a trackpad).
//!
//! **Threads** (`threads`, cards from [`crate::thread`]): a thread's card follows the last line
//! of its range on its side, an outdated one heads its file, one whose file left the diff heads
//! the stream in its "Not in the diff" section (`section`, before the first file), and the
//! composer sits under the selected lines (or in the card it answers). `C` comments, `]`/`[` focus the next and previous
//! unresolved thread, `R` replies to the focused one and `X` resolves or reopens it; every
//! change goes through the session's `review::ops` writes.
//!
//! The keyboard goes to the main area, which the shell renders with this pane's focus handle
//! and key context ([`crate::keymap::DIFF_PANE`], [`DiffPane::bind_actions`]).
//!
//! Automation targets: `stream`, `stream/file/<path>/header` (and its `/chevron`, `/viewed`,
//! `/split`, `/unified`, `/prev`, `/next`, `/menu`), `stream/pinned` (the same parts),
//! `stream/not_in_diff/header` (and its `/chevron`; pinned, `stream/pinned` and its `/chevron`),
//! `stream/menu/<copy_path|reveal>`, `stream/gap/<path>/<n>` (`/expand`, `/expand_all`),
//! `stream/line/<path>/<old|new>/<n>` (a line number), `stream/notice/<path>` (`/load_anyway`,
//! and a failure's banner parts), `stream/footer/<path>` (`/next`), and the cards' and the
//! composer's own (`thread/<id>`, `composer`; [`crate::thread`]).

mod nav;
mod reload;
mod render;
pub mod rows;
mod section;
pub mod text;
mod threads;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dv_core::diff::{DiffLine, Expand, FileDiff, Segment, expand_gap};
use dv_core::review::{AnchorSide, ThreadId};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, ListAlignment, ListOffset,
    ListState, Pixels, Point, Subscription, Task, px,
};

use crate::failure::Failure;
use crate::services::{ActiveServices as _, ViewMode};
use crate::session::{Comparison, FileState, ReviewSession, SessionEvent};
use crate::thread::Composer;
use rows::{
    Content, LinePos, Row, RowKey, RowsSpec, file_rows, find_row, heights, selected_numbers,
    side_number,
};
pub use section::{SECTION, SectionSummary};

/// How long a file shows skeleton rows before its header also says "Loading…" (DESIGN §4.4).
pub const SKELETON_FOR: Duration = Duration::from_millis(300);

/// The viewport's height until the list has been laid out once.
const DEFAULT_VIEWPORT: f32 = 800.;

/// How far past the viewport the list lays rows out (and measures them).
const OVERDRAW: f32 = 400.;

/// Files loaded ahead of the viewport, and behind it.
const PREFETCH_AHEAD: usize = 2;
const PREFETCH_BEHIND: usize = 1;

/// What the stream tells the views around it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffPaneEvent {
    /// [`DiffPane::current_file`] changed.
    CurrentFileChanged,
    /// The composer went, its draft with it: posted, cancelled, or dropped with its file or
    /// its thread (not replaced by another, nor gone with the review). A pick that waited for
    /// the draft runs now (`crate::shell::pending`).
    ComposerClosed,
}

/// The cursor: a line of a file's text diff and the side it is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub path: String,
    pub pos: LinePos,
}

/// The selection's fixed end; the cursor is the other.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SelectionAnchor {
    path: String,
    side: AnchorSide,
    line: usize,
}

/// A comment's lines being picked with the mouse (DESIGN §4.1): pressed on a line's `+`, the
/// button held over lines of the same file and side. Released, the composer opens on them.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LineDrag {
    path: String,
    side: AnchorSide,
}

/// A line as automation and the composer name it: its path, side and number on that side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineRef {
    pub path: String,
    pub side: AnchorSide,
    pub line: u32,
}

/// The selected lines: one file, one side, `start_line..=end_line` on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionRef {
    pub path: String,
    pub side: AnchorSide,
    pub start_line: u32,
    pub end_line: u32,
}

/// One file's rows in the list.
#[derive(Clone, Debug)]
struct Block {
    path: String,
    rows: Vec<Row>,
}

/// What the stream keeps per file between rebuilds.
#[derive(Default)]
struct FileUi {
    /// The loaded diff the segments below belong to.
    diff: Option<Arc<FileDiff>>,
    /// The comparison that diff was listed under: an old side under another one is another
    /// text (`reload`).
    comparison: Option<Comparison>,
    segments: Vec<Segment>,
    /// The widest line, in columns.
    columns: usize,
    /// The chevron's choice; `None` follows the viewed state (viewed files collapse).
    open: Option<bool>,
    /// Whether a failed load's banner shows its detail.
    banner_open: bool,
}

/// The file menu (the header's `⋯`): whose, and where it was opened.
#[derive(Clone, Debug)]
struct Menu {
    path: String,
    position: Point<Pixels>,
}

/// Where the reading position was, to put it back after rows are rebuilt.
struct ReadingAnchor {
    path: String,
    block: usize,
    key: RowKey,
    offset: Pixels,
}

/// The stream over the open review's files (see the module docs).
pub struct DiffPane {
    focus_handle: FocusHandle,
    session: Option<Entity<ReviewSession>>,
    mode: ViewMode,
    list: ListState,
    blocks: Vec<Block>,
    /// `starts[i]` is block `i`'s first list item (its header); the last entry is the spacer's.
    starts: Vec<usize>,
    index: HashMap<String, usize>,
    files: HashMap<String, FileUi>,
    cursor: Option<Cursor>,
    /// The side the reviewer put the cursor on in split view (`←`/`→`, a click on a line
    /// number; the new side until then). A file without lines there (a deletion, an addition)
    /// moves the cursor to the other side while it is in that file only: entering the next
    /// file that has lines on this side puts it back (`nav`).
    chosen_side: AnchorSide,
    selection: Option<SelectionAnchor>,
    /// The mouse picking a comment's lines, from a press on a line's `+` until the button
    /// comes up (`nav`).
    line_drag: Option<LineDrag>,
    /// A file shown before its diff landed: the cursor goes to its first change when it does.
    pending_first_change: Option<String>,
    current_file: Option<String>,
    /// The sideways scroll, in pixels, shared by every code row.
    h_offset: f32,
    /// The widest line of the files in view, in columns.
    view_columns: usize,
    menu: Option<Menu>,
    /// The files last asked for.
    wanted: Vec<String>,
    /// What the viewport looked like when the loads and the current file were last worked out.
    viewport_key: Option<ViewportKey>,
    viewport_pending: bool,
    spacer_height: f32,
    /// Bumped by every change to the rows or to how high a measured row is.
    rows_version: u64,
    /// The heights the measured rows (cards, the composer, an opened banner) were last laid
    /// out at, by file and row, at `measured_width`: gpui reports an item's height from the top
    /// of the viewport down only, and positions above it and the spacer need them too.
    measured: HashMap<(String, RowKey), f32>,
    measured_width: f32,
    /// For each file whose diff was just replaced by one with other lines, where each of its
    /// earlier lines went (`reload`): the reading position follows them.
    line_maps: HashMap<String, Vec<usize>>,
    /// The code font's advance, once measured.
    char_width: Option<f32>,
    /// The code cell's width in columns, from the last frame.
    code_columns: usize,
    loading_since: HashMap<String, Instant>,
    loading_timer: Option<Task<()>>,
    /// The composer, while one is open (`threads`).
    composer: Option<Composer>,
    /// The last composer field's focus handle: should it hold the keyboard without a row on
    /// screen, the keyboard goes back to the stream (`threads`).
    composer_focus: Option<FocusHandle>,
    /// A frame is about to hand the keyboard back to the stream.
    refocus_pending: bool,
    /// The keyboard's thread: `]`/`[` move it, `R` replies to it, `X` resolves it.
    focused_thread: Option<ThreadId>,
    /// A thread shown before its card was built (its file loading, or collapsed): it is
    /// scrolled into view once the card is there.
    pending_thread: Option<ThreadId>,
    /// A thread shown while another comparison was being listed: it is shown again once the
    /// list lands (the comments list's "all changes" rows, `threads`).
    reveal_after_listing: Option<ThreadId>,
    /// A draft the comparison shown can't hold, asked for (`C`) while all changes are being
    /// listed: it takes the keyboard once its row is there (`threads`).
    focus_composer_after_listing: bool,
    /// A reply asked for (the comments list's menu) before its thread's card was built: the
    /// reply composer opens in the card once it is (`threads`).
    reply_when_shown: Option<ThreadId>,
    /// Resolved threads the reviewer opened; the rest show collapsed (DESIGN §4.1).
    opened_threads: HashSet<ThreadId>,
    /// Why the last change to a thread's state failed, by thread.
    thread_error: Option<(ThreadId, Failure)>,
    /// Posts and thread-state changes whose follow-up hasn't landed.
    writes: usize,
    _session_events: Vec<Subscription>,
}

/// See [`DiffPane::viewport_key`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewportKey {
    item: usize,
    offset: f32,
    height: f32,
    width: f32,
    rows: u64,
}

impl EventEmitter<DiffPaneEvent> for DiffPane {}

impl Focusable for DiffPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl DiffPane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            session: None,
            mode: ViewMode::default(),
            list: ListState::new(1, ListAlignment::Top, px(OVERDRAW)),
            blocks: Vec::new(),
            starts: vec![0],
            index: HashMap::new(),
            files: HashMap::new(),
            cursor: None,
            chosen_side: AnchorSide::New,
            selection: None,
            line_drag: None,
            pending_first_change: None,
            current_file: None,
            h_offset: 0.,
            view_columns: 0,
            menu: None,
            wanted: Vec::new(),
            viewport_key: None,
            viewport_pending: false,
            spacer_height: 0.,
            rows_version: 0,
            measured: HashMap::new(),
            measured_width: 0.,
            line_maps: HashMap::new(),
            char_width: None,
            code_columns: 80,
            loading_since: HashMap::new(),
            loading_timer: None,
            composer: None,
            composer_focus: None,
            refocus_pending: false,
            focused_thread: None,
            pending_thread: None,
            reveal_after_listing: None,
            focus_composer_after_listing: false,
            reply_when_shown: None,
            opened_threads: HashSet::new(),
            thread_error: None,
            writes: 0,
            _session_events: Vec::new(),
        }
    }

    /// Shows `session`'s files (or none), from the top, in the view mode remembered for its
    /// review.
    pub fn set_session(&mut self, session: Option<Entity<ReviewSession>>, cx: &mut Context<Self>) {
        self._session_events = session
            .as_ref()
            .map(|session| {
                vec![
                    cx.subscribe(session, |this, _, event: &SessionEvent, cx| match event {
                        SessionEvent::FilesChanged => this.rebuild_all(cx),
                        SessionEvent::FileLoaded(path) => this.file_changed(path, cx),
                        SessionEvent::ReviewChanged => this.refresh_blocks(cx),
                        SessionEvent::Error => cx.notify(),
                    }),
                    cx.observe(session, |this, _, cx| {
                        this.track_loading(cx);
                        cx.notify();
                    }),
                ]
            })
            .unwrap_or_default();
        self.mode = session
            .as_ref()
            .map(|session| {
                let id = session.read(cx).review_id().clone();
                cx.services().review_ui.read(cx).view_mode(&id)
            })
            .unwrap_or_default();
        self.session = session;
        self.blocks.clear();
        self.recompute_starts();
        self.files.clear();
        self.cursor = None;
        self.chosen_side = AnchorSide::New;
        self.selection = None;
        self.pending_first_change = None;
        self.menu = None;
        self.h_offset = 0.;
        self.wanted.clear();
        self.loading_since.clear();
        self.composer = None;
        self.focused_thread = None;
        self.pending_thread = None;
        self.reveal_after_listing = None;
        self.focus_composer_after_listing = false;
        self.reply_when_shown = None;
        self.opened_threads.clear();
        self.thread_error = None;
        self.measured.clear();
        self.line_maps.clear();
        self.list.reset(1);
        self.rebuild_all(cx);
        self.list.scroll_to(ListOffset::default());
        self.viewport_changed(cx);
    }

    /// The file the reviewer is reading: the one the panel highlights.
    pub fn current_file(&self) -> Option<&str> {
        self.current_file.as_deref()
    }

    pub fn view_mode(&self) -> ViewMode {
        self.mode
    }

    /// Where the cursor is, by line number.
    pub fn cursor(&self) -> Option<LineRef> {
        let cursor = self.cursor.as_ref()?;
        let line = self.line(&cursor.path, cursor.pos.line)?;
        Some(LineRef {
            path: cursor.path.clone(),
            side: cursor.pos.side,
            line: side_number(line, cursor.pos.side)?,
        })
    }

    /// The selected lines (the selection's anchor to the cursor), if any.
    pub fn selection(&self) -> Option<SelectionRef> {
        let anchor = self.selection.as_ref()?;
        let cursor = self.cursor.as_ref().filter(|c| c.path == anchor.path)?;
        let lines = self.lines(&anchor.path)?;
        let (start_line, end_line) =
            selected_numbers(lines, anchor.side, anchor.line, cursor.pos.line)?;
        Some(SelectionRef {
            path: anchor.path.clone(),
            side: anchor.side,
            start_line,
            end_line,
        })
    }

    /// The sideways scroll in pixels.
    pub fn h_offset(&self) -> f32 {
        self.h_offset
    }

    /// Nothing is waiting on the next frame, and no post or thread change is on its way:
    /// automation's `settled`.
    pub fn is_settled(&self) -> bool {
        !self.viewport_pending && self.writes == 0
    }

    /// Brings `path` into view as the current file (a click on its row in the panel): its
    /// header at the top and the cursor on its first change. A path that isn't in the review's
    /// diff changes nothing.
    pub fn reveal_file(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Some(&index) = self.index.get(path) {
            self.go_to_file(index, cx);
        }
    }

    /// What `state.stream` shows per file: its path, whether it is collapsed, its rows and the
    /// lines each of its gaps hides.
    pub fn file_summaries(&self) -> Vec<FileSummary> {
        self.blocks
            .iter()
            .filter(|block| block.path != SECTION)
            .map(|block| FileSummary {
                path: block.path.clone(),
                collapsed: matches!(
                    block.rows.first(),
                    Some(Row::Header {
                        collapsed: true,
                        ..
                    })
                ),
                rows: block.rows.len(),
                gaps: block
                    .rows
                    .iter()
                    .filter_map(|row| match row {
                        Row::Gap(gap) => Some(gap.lines.len()),
                        _ => None,
                    })
                    .collect(),
                notice: block.rows.iter().find_map(|row| match row {
                    Row::Notice(notice) => Some(notice.key()),
                    _ => None,
                }),
            })
            .collect()
    }

    /// The file whose header is pinned at the top, if one is (not the section's).
    pub fn pinned_file(&self) -> Option<&str> {
        self.pinned()
            .map(|(block, _)| self.blocks[block].path.as_str())
            .filter(|path| *path != SECTION)
    }

    /// The file at the top of the viewport (`None` while the section is).
    pub fn top_file(&self) -> Option<&str> {
        let top = self.list.logical_scroll_top();
        self.locate(top.item_ix)
            .map(|(block, _)| self.blocks[block].path.as_str())
            .filter(|path| *path != SECTION)
    }

    /// Switches split / unified (`S`), remembered for the review; the reading position and the
    /// cursor stay where they were.
    pub fn set_view_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        if let Some(session) = &self.session {
            let id = session.read(cx).review_id().clone();
            let ui = cx.services().review_ui.clone();
            ui.update(cx, |ui, cx| ui.set_view_mode(&id, mode, cx));
        }
        // Cards and the composer take another width, so another height.
        self.measured.clear();
        self.rebuild_all(cx);
        self.reveal_cursor(cx);
    }

    /// Reveals one step (or all) of gap `ordinal` of `path`.
    pub fn expand(&mut self, path: &str, ordinal: usize, expand: Expand, cx: &mut Context<Self>) {
        let Some(&index) = self.index.get(path) else {
            return;
        };
        let segment = self.blocks[index].rows.iter().find_map(|row| match row {
            Row::Gap(gap) if gap.ordinal == ordinal => Some(gap.segment),
            _ => None,
        });
        let (Some(segment), Some(ui)) = (segment, self.files.get_mut(path)) else {
            return;
        };
        ui.segments = expand_gap(&ui.segments, segment, expand);
        self.refresh_block(index, cx);
        self.after_rows_changed(cx);
    }

    /// Opens or collapses `path` (the header's chevron; [`SECTION`] for the section's).
    pub fn set_open(&mut self, path: &str, open: bool, cx: &mut Context<Self>) {
        let viewed = self.is_viewed(path, cx);
        let Some(&index) = self.index.get(path) else {
            return;
        };
        // Opening a viewed file, or collapsing one that isn't, overrides the default.
        self.files.entry(path.to_string()).or_default().open = (open == viewed).then_some(open);
        self.refresh_block(index, cx);
        self.after_rows_changed(cx);
    }

    /// Marks `path` viewed or not viewed in the round in progress; a finished review changes
    /// nothing. The file collapses (or opens) once the change is in the review.
    pub fn toggle_viewed(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        if let Some(ui) = self.files.get_mut(path) {
            ui.open = None;
        }
        session.update(cx, |session, cx| session.toggle_viewed(path, cx));
    }

    fn is_viewed(&self, path: &str, cx: &App) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.read(cx).is_viewed(path))
    }

    /// The loaded text's lines of `path`.
    fn lines(&self, path: &str) -> Option<&[DiffLine]> {
        self.files
            .get(path)?
            .diff
            .as_ref()?
            .as_text()
            .map(|text| text.lines.as_slice())
    }

    fn line(&self, path: &str, line: usize) -> Option<&DiffLine> {
        self.lines(path)?.get(line)
    }

    // The blocks and the list.

    /// Rebuilds block `index`'s rows from the session: the section's, or a file's.
    fn build_block(&mut self, index: usize, cx: &App) -> Option<Block> {
        if self.is_section(index) {
            return self.section_block(cx);
        }
        self.build_file_block(index - self.first_file_block(), cx)
    }

    /// Builds one file's block from the session (`index` is the file's place in its list).
    fn build_file_block(&mut self, index: usize, cx: &App) -> Option<Block> {
        let session = self.session.clone()?;
        let session = session.read(cx);
        let entry = session.files().get(index)?;
        let viewed = session.is_viewed(&entry.path);
        // A file loading again shows the diff it had until the new one lands
        // (`ReviewSession::shown`), so its rows — cards, the composer — stay put meanwhile.
        let state = session.file_state(&entry.path);
        let shown = session.shown_diff(&entry.path);
        let failed = matches!(state, Some(FileState::Failed(_)));
        if let Some(diff) = shown.filter(|_| !failed) {
            let threads = threads::thread_lines(&entry.path, diff, session);
            let comparison = session
                .shown_comparison(&entry.path)
                .unwrap_or(session.comparison());
            self.take_diff(&entry.path, diff, comparison, &threads);
        }
        let composer = self.composer.as_ref().map(|composer| &composer.target);
        let ui = self.files.entry(entry.path.clone()).or_default();
        let content = match shown {
            _ if failed => {
                ui.diff = None;
                Content::Failed
            }
            Some(diff) => Content::Loaded(diff),
            None => Content::Loading,
        };
        let (inserts, outdated) = match content {
            Content::Loaded(diff) => threads::thread_rows(&entry.path, diff, session, composer),
            _ => (Vec::new(), Vec::new()),
        };
        let rows = file_rows(&RowsSpec {
            mode: self.mode,
            collapsed: ui.open.map_or(viewed, |open| !open),
            content,
            segments: &ui.segments,
            inserts: &inserts,
            outdated: &outdated,
            footer: true,
        });
        Some(Block {
            path: entry.path.clone(),
            rows,
        })
    }

    /// Rebuilds every block (the file list changed, or the view mode, or the section came or
    /// went), keeping the reading position.
    fn rebuild_all(&mut self, cx: &mut Context<Self>) {
        let mut anchor = self.capture_anchor();
        let first_before = self.first_file_block();
        let count = self
            .session
            .as_ref()
            .map_or(0, |session| session.read(cx).files().len());
        let section = self.section_block(cx);
        let files: Vec<Block> = (0..count)
            .filter_map(|index| self.build_file_block(index, cx))
            .collect();
        self.blocks = section.into_iter().chain(files).collect();
        // A file gone falls back to the block now in its place: the section coming or going
        // shifts every file's block by one.
        if let Some(anchor) = anchor.as_mut() {
            anchor.block = (anchor.block + self.first_file_block()).saturating_sub(first_before);
        }
        let listed: HashSet<&str> = self.blocks.iter().map(|b| b.path.as_str()).collect();
        self.files.retain(|path, _| listed.contains(path.as_str()));
        self.measured
            .retain(|(path, _), _| listed.contains(path.as_str()));
        self.recompute_starts();
        // A comment on a file that left the review (the agent reverted it) has nowhere to go.
        // One on a file the comparison shown doesn't list (an interdiff) keeps its draft until
        // its file shows again (DESIGN §4.1). A reply goes with its thread's card, wherever that
        // is drawn (the section, once its file left the review).
        let in_review = |path: &str| {
            self.session
                .as_ref()
                .is_some_and(|session| session.read(cx).in_review(path))
        };
        if self.composer.as_ref().is_some_and(|composer| {
            composer.target.reply_to.is_none()
                && !self.index.contains_key(&composer.target.path)
                && !in_review(&composer.target.path)
        }) {
            self.composer = None;
            cx.emit(DiffPaneEvent::ComposerClosed);
        }
        self.list.reset(self.item_count());
        self.attach_composer_focus(cx);
        self.restore_anchor(anchor);
        if self
            .cursor
            .as_ref()
            .is_some_and(|cursor| !self.index.contains_key(&cursor.path))
        {
            self.cursor = None;
            self.selection = None;
        }
        self.after_rows_changed(cx);
        if let Some(thread) = self.reveal_after_listing.take() {
            self.reveal_thread(&thread, cx);
        }
    }

    /// Rebuilds every block in place, splicing only those whose rows changed (the review
    /// changed: viewed files collapse), or the whole stream when the section comes or goes.
    fn refresh_blocks(&mut self, cx: &mut Context<Self>) {
        if self.section_comes_or_goes(cx) {
            self.rebuild_all(cx);
            self.forget_gone_threads(cx);
            return;
        }
        let mut changed = false;
        for index in 0..self.blocks.len() {
            changed |= self.refresh_block(index, cx);
        }
        // A card whose thread changed (a reply, a new state) keeps its row: measure it again.
        for index in 0..self.blocks.len() {
            self.remeasure_cards(index);
        }
        self.forget_gone_threads(cx);
        if changed {
            self.after_rows_changed(cx);
        } else {
            cx.notify();
        }
    }

    /// A file's state changed in the session (its diff or highlights landed, or it loads again),
    /// which can also change the threads the section holds.
    fn file_changed(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.section_comes_or_goes(cx) {
            self.rebuild_all(cx);
            return;
        }
        let section = self.refresh_section(cx);
        let Some(&index) = self.index.get(path) else {
            if section {
                self.after_rows_changed(cx);
            }
            return;
        };
        if self.refresh_block(index, cx) || section {
            self.after_rows_changed(cx);
        } else {
            cx.notify();
        }
    }

    /// Rebuilds block `index` and splices its rows into the list when they changed. The header
    /// item stays (its height is remeasured if it changed); a reading position inside the block
    /// stays on its row.
    fn refresh_block(&mut self, index: usize, cx: &App) -> bool {
        let Some(block) = self.build_block(index, cx) else {
            return false;
        };
        let old = &self.blocks[index];
        if old.rows == block.rows {
            // Rows that didn't move keep the reading position as it is.
            self.line_maps.clear();
            return false;
        }
        let start = self.starts[index];
        let old_len = old.rows.len();
        let header_changed = old.rows.first() != block.rows.first();
        let top = self.list.logical_scroll_top();
        let inside = top.item_ix >= start && top.item_ix < start + old_len;
        let anchor = inside.then(|| self.capture_anchor()).flatten();
        // The composer's row carries its field's focus handle, so the list keeps the field
        // rendered, and the keyboard working, while it is scrolled out of view.
        let focus: Vec<Option<FocusHandle>> = block.rows[1..]
            .iter()
            .map(|row| self.row_focus(&block.path, row, cx))
            .collect();
        self.list
            .splice_focusable(start + 1..start + old_len, focus);
        if header_changed {
            self.list.remeasure_items(start..start + 1);
        }
        self.blocks[index] = block;
        self.recompute_starts();
        self.restore_anchor(anchor);
        true
    }

    /// What follows any change to the rows: the cursor's pending first change, the spacer, the
    /// loads and the current file.
    fn after_rows_changed(&mut self, cx: &mut Context<Self>) {
        self.rows_version += 1;
        self.reveal_pending_thread();
        if let Some(path) = self.pending_first_change.clone()
            && let Some(&index) = self.index.get(&path)
            && self.lines(&path).is_some()
        {
            self.pending_first_change = None;
            if self.current_file.as_deref() == Some(path.as_str()) && self.cursor.is_none() {
                self.cursor = self.first_change(index).map(|pos| Cursor {
                    path: path.clone(),
                    pos,
                });
            }
        }
        self.viewport_changed(cx);
        cx.notify();
    }

    fn recompute_starts(&mut self) {
        self.starts.clear();
        self.index.clear();
        let mut at = 0;
        for (i, block) in self.blocks.iter().enumerate() {
            self.starts.push(at);
            self.index.insert(block.path.clone(), i);
            at += block.rows.len();
        }
        self.starts.push(at);
    }

    /// Items in the list: every row and the spacer.
    fn item_count(&self) -> usize {
        self.starts.last().copied().unwrap_or(0) + 1
    }

    /// The block and row of list item `item`; `None` for the spacer.
    fn locate(&self, item: usize) -> Option<(usize, usize)> {
        if item + 1 >= self.item_count() {
            return None;
        }
        let block = self.starts.partition_point(|&start| start <= item) - 1;
        Some((block, item - self.starts[block]))
    }

    fn row(&self, item: usize) -> Option<&Row> {
        self.locate(item)
            .and_then(|(block, row)| self.blocks[block].rows.get(row))
    }

    /// Item `item`'s height: its row's ([`Row::height`]), or as measured for the rows that
    /// are measured (an estimate until they are).
    fn item_height(&self, item: usize) -> f32 {
        match self.row(item) {
            Some(row) if row.has_fixed_height() => row.height(),
            Some(row) => self.measured_height(item).unwrap_or(row.height()),
            None => self.spacer_height,
        }
    }

    /// How high a measured row was laid out: as gpui reports it, else as it was last seen.
    fn measured_height(&self, item: usize) -> Option<f32> {
        self.list
            .bounds_for_item(item)
            .map(|bounds| f32::from(bounds.size.height))
            .or_else(|| {
                let (block, row) = self.locate(item)?;
                let block = &self.blocks[block];
                let key = (block.path.clone(), block.rows.get(row)?.key());
                self.measured.get(&key).copied()
            })
    }

    /// Notes the heights gpui reports for the measured rows from the top of the viewport to
    /// the end of the overdraw, and for the last file's (which the spacer counts).
    fn note_measured(&mut self) {
        let width = f32::from(self.list.viewport_bounds().size.width);
        if (width - self.measured_width).abs() > 0.5 {
            // Another width wraps cards and the composer otherwise.
            self.measured.clear();
            self.measured_width = width;
        }
        let top = self.list.logical_scroll_top().item_ix;
        let limit = self.viewport_height() + OVERDRAW;
        let mut y = 0.;
        let mut items = Vec::new();
        let mut item = top;
        while y < limit && item + 1 < self.item_count() {
            items.push(item);
            y += self.item_height(item);
            item += 1;
        }
        if let Some(last) = self.blocks.len().checked_sub(1) {
            items.extend(self.starts[last].max(item)..self.starts[last + 1]);
        }
        for item in items {
            let Some((block, row)) = self.locate(item) else {
                continue;
            };
            let block = &self.blocks[block];
            if block.rows[row].has_fixed_height() {
                continue;
            }
            if let Some(bounds) = self.list.bounds_for_item(item) {
                let key = (block.path.clone(), block.rows[row].key());
                self.measured.insert(key, f32::from(bounds.size.height));
            }
        }
    }

    /// Has gpui measure item `item` (a measured row) again, its content having changed, and
    /// forgets its old height meanwhile.
    pub(super) fn remeasure(&mut self, item: usize) {
        if let Some((block, row)) = self.locate(item) {
            let block = &self.blocks[block];
            let key = (block.path.clone(), block.rows[row].key());
            self.measured.remove(&key);
        }
        self.list.remeasure_items(item..item + 1);
        self.rows_version += 1;
        self.update_spacer();
    }

    fn capture_anchor(&self) -> Option<ReadingAnchor> {
        let top = self.list.logical_scroll_top();
        let (block, row) = self.locate(top.item_ix)?;
        Some(ReadingAnchor {
            path: self.blocks[block].path.clone(),
            block,
            key: self.blocks[block].rows[row].key(),
            offset: top.offset_in_item,
        })
    }

    /// Puts the reading position back on the row `anchor` names, following its line when the
    /// file's diff was replaced (`line_maps`); a line that isn't shown puts the nearest one at
    /// the top, a row that is gone its file's header, and a file that is gone the file now in
    /// its place.
    fn restore_anchor(&mut self, anchor: Option<ReadingAnchor>) {
        let maps = std::mem::take(&mut self.line_maps);
        let Some(mut anchor) = anchor else {
            return;
        };
        if let Some(map) = maps.get(&anchor.path) {
            let moved = |line: usize| map.get(line).copied().unwrap_or(line);
            anchor.key = match anchor.key {
                RowKey::Line(line) => RowKey::Line(moved(line)),
                RowKey::Gap(line) => RowKey::Gap(moved(line)),
                key => key,
            };
        }
        let Some(block) = self.index.get(&anchor.path).copied().or_else(|| {
            self.blocks
                .len()
                .checked_sub(1)
                .map(|last| anchor.block.min(last))
        }) else {
            self.list.scroll_to(ListOffset::default());
            return;
        };
        let rows = &self.blocks[block].rows;
        let found = if self.blocks[block].path == anchor.path {
            rows.iter()
                .position(|row| row.key() == anchor.key)
                .map(|row| (row, anchor.offset))
                .or_else(|| match anchor.key {
                    RowKey::Line(line) | RowKey::Gap(line) => rows
                        .iter()
                        .enumerate()
                        .filter_map(|(i, row)| line_distance(row, line).map(|d| (i, d)))
                        .min_by_key(|&(_, distance)| distance)
                        .map(|(i, _)| (i, px(0.))),
                    _ => None,
                })
        } else {
            None
        };
        let (row, offset) = found.unwrap_or((0, px(0.)));
        let height = px(rows[row].height());
        self.list.scroll_to(ListOffset {
            item_ix: self.starts[block] + row,
            offset_in_item: offset.min(height),
        });
    }

    // The viewport.

    fn viewport_height(&self) -> f32 {
        let height = f32::from(self.list.viewport_bounds().size.height);
        if height > 0. {
            height
        } else {
            DEFAULT_VIEWPORT
        }
    }

    fn current_viewport_key(&self) -> ViewportKey {
        let top = self.list.logical_scroll_top();
        let bounds = self.list.viewport_bounds();
        ViewportKey {
            item: top.item_ix,
            offset: f32::from(top.offset_in_item),
            height: f32::from(bounds.size.height),
            width: f32::from(bounds.size.width),
            rows: self.rows_version,
        }
    }

    /// Sizes the spacer after the last file so its header can reach the top (DESIGN §5 `J`):
    /// the viewport less the last file's rows, its measured ones as last measured and the rest
    /// at the least they can be ([`heights::MEASURED_MIN`]). An estimate that ran high would
    /// leave the spacer short, and gpui would stop the list with the header below the top.
    fn update_spacer(&mut self) {
        let last_rows = self.blocks.len().checked_sub(1).map_or(0., |last| {
            let start = self.starts[last];
            (0..self.blocks[last].rows.len())
                .map(|row| {
                    let row_height = self.blocks[last].rows[row].height();
                    if self.blocks[last].rows[row].has_fixed_height() {
                        row_height
                    } else {
                        self.measured_height(start + row)
                            .unwrap_or(heights::MEASURED_MIN)
                    }
                })
                .sum()
        });
        let spacer = (self.viewport_height() - last_rows).max(0.);
        if (spacer - self.spacer_height).abs() > 0.5 {
            self.spacer_height = spacer;
            let last = self.item_count() - 1;
            self.list.remeasure_items(last..last + 1);
        }
    }

    /// Works out what the viewport shows: the spacer's height, the files to load, the current
    /// file and how far the rows can scroll sideways.
    fn viewport_changed(&mut self, cx: &mut Context<Self>) {
        let height = self.viewport_height();
        self.note_measured();
        self.update_spacer();
        let mut wanted = Vec::new();
        let mut columns = 0;
        if let Some((first, last)) = self.visible_blocks(height) {
            let ahead = (last + 1..=last + PREFETCH_AHEAD).filter(|&i| i < self.blocks.len());
            let behind = first.saturating_sub(PREFETCH_BEHIND)..first;
            for index in (first..=last).chain(ahead).chain(behind) {
                if !self.is_section(index) {
                    wanted.push(self.blocks[index].path.clone());
                }
            }
            columns = (first..=last)
                .filter_map(|index| self.files.get(&self.blocks[index].path))
                .map(|ui| ui.columns)
                .max()
                .unwrap_or(0);
        }
        self.view_columns = columns;
        self.clamp_h_offset();
        if wanted != self.wanted {
            self.wanted = wanted.clone();
            if let Some(session) = self.session.clone() {
                session.update(cx, |session, cx| session.ensure_loaded(wanted, cx));
            }
        }
        self.update_current(cx);
        self.viewport_key = Some(self.current_viewport_key());
    }

    /// The first and last block with a row in view.
    fn visible_blocks(&self, height: f32) -> Option<(usize, usize)> {
        let top = self.list.logical_scroll_top();
        let first = self
            .locate(top.item_ix)
            .map(|(block, _)| block)
            .or_else(|| self.blocks.len().checked_sub(1))?;
        let mut last = first;
        let mut y = -f32::from(top.offset_in_item);
        let mut item = top.item_ix;
        while y < height && item + 1 < self.item_count() {
            if let Some((block, _)) = self.locate(item) {
                last = block;
            }
            y += self.item_height(item);
            item += 1;
        }
        Some((first, last))
    }

    fn update_current(&mut self, cx: &mut Context<Self>) {
        // The section isn't a file: while it is at the top, no file is current.
        let current = self
            .compute_current()
            .filter(|&block| !self.is_section(block))
            .map(|block| self.blocks[block].path.clone());
        if current != self.current_file {
            self.current_file = current;
            cx.emit(DiffPaneEvent::CurrentFileChanged);
            cx.notify();
        }
    }

    /// The cursor's file while the cursor is on screen, else the file at the top of the
    /// viewport (decision of DESIGN §5's keyboard additions).
    fn compute_current(&self) -> Option<usize> {
        if self.blocks.is_empty() {
            return None;
        }
        let height = self.viewport_height();
        if let Some(item) = self.cursor_item()
            && let Some(y) = self.offset_of(item, height)
            && y + heights::CODE > 0.
            && y < height
        {
            return self.locate(item).map(|(block, _)| block);
        }
        let top = self.list.logical_scroll_top();
        Some(
            self.locate(top.item_ix)
                .map_or(self.blocks.len() - 1, |(block, _)| block),
        )
    }

    /// The pinned header: the block at the top whose header has scrolled off, and how far up
    /// the next file's header pushes it (`<= 0`).
    fn pinned(&self) -> Option<(usize, f32)> {
        let top = self.list.logical_scroll_top();
        let (block, row) = self.locate(top.item_ix)?;
        let header = self.blocks[block].rows.first()?;
        if matches!(
            header,
            Row::Header {
                collapsed: true,
                ..
            }
        ) || (row == 0 && top.offset_in_item <= px(0.))
        {
            return None;
        }
        let size = header.height();
        // Where the next header is, if it is close enough to push this one up.
        let next = self.starts[block + 1];
        let mut y = -f32::from(top.offset_in_item);
        let mut item = top.item_ix;
        while item < next && y < size {
            y += self.item_height(item);
            item += 1;
        }
        let shift = if item == next { (y - size).min(0.) } else { 0. };
        Some((block, shift))
    }

    // Positions, worked out from row heights without waiting for a layout.

    /// Item `item`'s top relative to the viewport's top; `None` when it is more than `limit`
    /// away.
    fn offset_of(&self, item: usize, limit: f32) -> Option<f32> {
        let top = self.list.logical_scroll_top();
        let mut y = -f32::from(top.offset_in_item);
        let mut at = top.item_ix;
        if item >= at {
            while at < item {
                y += self.item_height(at);
                at += 1;
                if y > limit {
                    return None;
                }
            }
        } else {
            while at > item {
                at -= 1;
                y -= self.item_height(at);
                if y < -limit {
                    return None;
                }
            }
        }
        Some(y)
    }

    /// Scrolls so item `item`'s top sits `y` below the viewport's top (or as close as the top
    /// of the list allows).
    fn scroll_item_to(&mut self, item: usize, y: f32) {
        let mut above = 0.;
        let mut at = item;
        while above < y && at > 0 {
            at -= 1;
            above += self.item_height(at);
        }
        let offset = if above >= y { above - y } else { 0. };
        self.list.scroll_to(ListOffset {
            item_ix: at,
            offset_in_item: px(offset),
        });
    }

    /// Scrolls item `item` into the part of the viewport the pinned header leaves, with
    /// `margin` rows around it when it has to move.
    fn reveal_item(&mut self, item: usize, margin: usize) {
        let height = self.viewport_height();
        let clear = heights::HEADER;
        let size = self.item_height(item);
        let margin = margin as f32 * heights::CODE;
        let above = match self.offset_of(item, height * 2.) {
            Some(y) if y >= clear && y + size <= height => return,
            Some(y) => y < clear,
            None => item < self.list.logical_scroll_top().item_ix,
        };
        if above {
            self.scroll_item_to(item, clear + margin);
        } else {
            self.scroll_item_to(item, (height - size - margin).max(clear));
        }
    }

    /// The list offset `delta` pixels below (or above, negative) `from`, clamped to the list.
    fn offset_by(&self, from: ListOffset, delta: f32) -> ListOffset {
        let last = self.item_count() - 1;
        let mut item = from.item_ix.min(last);
        let mut rest = f32::from(from.offset_in_item) + delta;
        if rest >= 0. {
            while item < last && rest >= self.item_height(item) {
                rest -= self.item_height(item);
                item += 1;
            }
        } else {
            while rest < 0. && item > 0 {
                item -= 1;
                rest += self.item_height(item);
            }
            rest = rest.max(0.);
        }
        ListOffset {
            item_ix: item,
            offset_in_item: px(rest),
        }
    }

    /// The furthest the list scrolls: the offset whose viewport ends at the bottom of the
    /// last item (the spacer).
    fn max_scroll_top(&self) -> ListOffset {
        let height = self.viewport_height();
        let mut below = 0.;
        let mut item = self.item_count();
        while item > 0 && below < height {
            item -= 1;
            below += self.item_height(item);
        }
        ListOffset {
            item_ix: item,
            offset_in_item: px((below - height).max(0.)),
        }
    }

    /// The cursor's list item, when its row is shown.
    fn cursor_item(&self) -> Option<usize> {
        let cursor = self.cursor.as_ref()?;
        let block = *self.index.get(&cursor.path)?;
        let lines = self.lines(&cursor.path)?;
        find_row(&self.blocks[block].rows, lines, self.mode, cursor.pos)
            .map(|row| self.starts[block] + row)
    }

    fn clamp_h_offset(&mut self) {
        let width = self.char_width.unwrap_or(7.2);
        let max = (self.view_columns as f32 - self.code_columns as f32 + 2.).max(0.) * width;
        self.h_offset = self.h_offset.clamp(0., max);
    }

    /// Scrolls the code rows sideways by `delta` pixels (positive: further right).
    pub fn scroll_sideways(&mut self, delta: f32, cx: &mut Context<Self>) {
        let before = self.h_offset;
        self.h_offset += delta;
        self.clamp_h_offset();
        if self.h_offset != before {
            cx.notify();
        }
    }

    /// Notes when visible files start loading, so their headers can say so after
    /// [`SKELETON_FOR`].
    fn track_loading(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let session = session.read(cx);
        let loading: HashSet<&str> = self
            .wanted
            .iter()
            .map(String::as_str)
            .filter(|path| matches!(session.file_state(path), Some(FileState::Loading { .. })))
            .collect();
        self.loading_since
            .retain(|path, _| loading.contains(path.as_str()));
        let mut started = false;
        for path in loading {
            if !self.loading_since.contains_key(path) {
                self.loading_since.insert(path.to_string(), Instant::now());
                started = true;
            }
        }
        if started {
            self.loading_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SKELETON_FOR).await;
                this.update(cx, |this, cx| {
                    this.loading_timer = None;
                    cx.notify();
                })
                .ok();
            }));
        }
    }

    /// Whether `path` has been loading for longer than [`SKELETON_FOR`].
    fn slow_to_load(&self, path: &str) -> bool {
        self.loading_since
            .get(path)
            .is_some_and(|since| since.elapsed() >= SKELETON_FOR)
    }
}

/// How far `row` is from line `line` (an index into its file's lines): 0 for a row that shows it
/// or a gap that hides it; `None` for a row without lines.
fn line_distance(row: &Row, line: usize) -> Option<usize> {
    let (first, last) = match *row {
        Row::Line(at) => (at, at),
        Row::Pair { old, new } => {
            let (a, b) = (old.or(new)?, new.or(old)?);
            (a.min(b), a.max(b))
        }
        Row::Gap(ref gap) => (gap.lines.start, gap.lines.end.saturating_sub(1)),
        _ => return None,
    };
    Some(first.saturating_sub(line) + line.saturating_sub(last))
}

/// Whether two diffs of a file show the same lines, so the gaps the reviewer expanded in one
/// still fit the other.
fn same_lines(a: &FileDiff, b: &FileDiff) -> bool {
    match (a.as_text(), b.as_text()) {
        (Some(a), Some(b)) => a.lines == b.lines,
        _ => false,
    }
}

/// One file as `state.stream` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSummary {
    pub path: String,
    pub collapsed: bool,
    pub rows: usize,
    /// How many lines each of its gaps hides, top to bottom.
    pub gaps: Vec<usize>,
    /// The notice it shows instead of lines ([`rows::Notice::key`]), if any.
    pub notice: Option<&'static str>,
}
