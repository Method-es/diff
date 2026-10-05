//! Threads in the stream (DESIGN §4.1, §5): where their cards and the composer go, the keys
//! that move between threads (`]`/`[`), reply (`R`), resolve (`X`) and comment (`C`), and the
//! writes behind them, each a `dv_core::review::ops` call through the session.
//!
//! A thread shows inline after the last line of its range on its side when its anchored text
//! still stands at those lines; one whose text moved on is outdated (PLAN §8, until relocation)
//! and its card heads its file, with its original lines. Resolved threads collapse to one line
//! until opened. The composer sits under the selection's last line (a new thread) or inside the
//! card it answers (a reply), which stays open meanwhile, and takes the keyboard while it is
//! open; posting or cancelling hands it back to the stream.
//!
//! The composer's row carries its field's focus handle into the list, so the field stays
//! rendered, and the keys work, while the stream scrolls it out of view; typing brings it back
//! into view. A composer whose row is gone (its file collapsed or failed to load) keeps its
//! text, but never the keyboard, which goes back to the stream; `C` opens its file again and
//! takes it up. One whose file left the diff, or whose thread was deleted, is dropped.

use dv_core::diff::{DiffLine, FileDiff};
use dv_core::review::{AnchorSide, ThreadId, ThreadState};
use gpui::{App, AppContext as _, Context, FocusHandle, Focusable as _, Window};
use gpui_component::input::{InputEvent, TextareaState};

use super::rows::{Dir, Insert, InsertItem, Row, side_number};
use super::{DiffPane, rows};
use crate::failure::Failure;
use crate::session::{Placement, ReviewSession, placement_in};
use crate::thread::{COMPOSER_ROWS, Composer, ComposerTarget, REPLY_ROWS};

/// Whether `row` of `path` holds `composer`: its own row, or the card of the thread it answers.
fn holds_composer(composer: &Composer, path: &str, row: &Row) -> bool {
    composer.target.path == path
        && match &composer.target.reply_to {
            Some(thread) => row.thread() == Some(thread),
            None => matches!(
                row,
                Row::Insert(Insert {
                    item: InsertItem::Composer,
                    ..
                })
            ),
        }
}

/// The thread cards and the composer of one file's rows: the inline inserts, and the outdated
/// threads whose cards head the file.
pub(super) fn thread_rows(
    path: &str,
    diff: &FileDiff,
    session: &ReviewSession,
    composer: Option<&ComposerTarget>,
) -> (Vec<Insert>, Vec<ThreadId>) {
    let mut inserts = Vec::new();
    let mut outdated = Vec::new();
    let lines = diff.as_text().map(|text| text.lines.as_slice());
    for thread in session.threads_in(path) {
        let anchor = &thread.anchor;
        // Against the diff these rows show, which may be the one a reload replaces.
        match (placement_in(thread, diff), lines) {
            (Placement::Inline, Some(lines)) => {
                match line_index(lines, anchor.side, anchor.end_line) {
                    Some(line) => inserts.push(Insert {
                        line,
                        side: anchor.side,
                        item: InsertItem::Thread(thread.id.clone()),
                    }),
                    None => outdated.push(thread.id.clone()),
                }
            }
            _ => outdated.push(thread.id.clone()),
        }
    }
    if let (Some(target), Some(lines)) = (composer, lines)
        && target.reply_to.is_none()
        && target.path == path
        && let Some(line) = nearest_line(lines, target.side, target.end_line)
    {
        inserts.push(Insert {
            line,
            side: target.side,
            item: InsertItem::Composer,
        });
    }
    (inserts, outdated)
}

/// The index of the line numbered `number` on `side`.
fn line_index(lines: &[DiffLine], side: AnchorSide, number: u32) -> Option<usize> {
    lines
        .iter()
        .position(|line| side_number(line, side) == Some(number))
}

/// The line numbered `number` on `side`, else the last one before it, else the first line on
/// that side: where the composer stays when the file changed under it.
fn nearest_line(lines: &[DiffLine], side: AnchorSide, number: u32) -> Option<usize> {
    line_index(lines, side, number)
        .or_else(|| {
            lines
                .iter()
                .enumerate()
                .filter(|(_, line)| side_number(line, side).is_some_and(|n| n < number))
                .map(|(index, _)| index)
                .next_back()
        })
        .or_else(|| lines.iter().position(|line| rows::on_side(line, side)))
}

impl DiffPane {
    /// The open composer.
    pub fn composer(&self) -> Option<&crate::thread::Composer> {
        self.composer.as_ref()
    }

    /// The composer's text as typed.
    pub fn composer_text(&self, cx: &gpui::App) -> Option<String> {
        self.composer
            .as_ref()
            .map(|composer| composer.input.read(cx).value().to_string())
    }

    /// The keyboard's thread.
    pub fn focused_thread(&self) -> Option<&ThreadId> {
        self.focused_thread.as_ref()
    }

    /// Whether `thread`'s card shows whole: every thread but a resolved one the reviewer hasn't
    /// opened and isn't replying to.
    pub fn is_thread_open(&self, thread: &ThreadId, state: ThreadState) -> bool {
        state != ThreadState::Resolved
            || self.opened_threads.contains(thread)
            || self
                .composer
                .as_ref()
                .is_some_and(|composer| composer.target.reply_to.as_ref() == Some(thread))
    }

    /// Why the last change to `thread`'s state failed.
    pub fn thread_error(&self, thread: &ThreadId) -> Option<&Failure> {
        self.thread_error
            .as_ref()
            .filter(|(id, _)| id == thread)
            .map(|(_, failure)| failure)
    }

    fn read_only(&self, cx: &gpui::App) -> bool {
        self.session
            .as_ref()
            .is_none_or(|session| session.read(cx).is_read_only())
    }

    /// `C`: opens the composer on the selected lines, else on the cursor's line; without a
    /// cursor, it is put on the first line in view first, as `↓` does. A composer that already
    /// holds text keeps it and takes the keyboard instead, wherever the cursor is (its file
    /// opens again if it collapsed).
    pub(super) fn comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) || self.keep_composer(window, cx) {
            return;
        }
        if self.selection.is_none() && self.cursor.is_none() {
            self.cursor = self.first_visible_cell();
        }
        let target = match (self.selection(), self.cursor()) {
            (Some(selection), _) => ComposerTarget {
                path: selection.path,
                side: selection.side,
                start_line: selection.start_line,
                end_line: selection.end_line,
                reply_to: None,
            },
            (None, Some(cursor)) => ComposerTarget {
                path: cursor.path,
                side: cursor.side,
                start_line: cursor.line,
                end_line: cursor.line,
                reply_to: None,
            },
            (None, None) => return,
        };
        // A collapsed (viewed) file opens to show the composer.
        if self.is_collapsed(&target.path) {
            self.set_open(&target.path.clone(), true, cx);
        }
        self.open_composer(target, window, cx);
    }

    /// `R`: replies to the focused thread.
    pub(super) fn reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(thread) = self.focused_thread.clone() {
            self.start_reply(&thread, window, cx);
        }
    }

    /// Opens the reply composer in `thread`'s card (a click on its reply field, or `R`).
    pub fn start_reply(&mut self, thread: &ThreadId, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let target = {
            let session = session.read(cx);
            let Some(found) = session.review().thread(thread) else {
                return;
            };
            let Some(entry) = session.file_of(found) else {
                return;
            };
            ComposerTarget {
                path: entry.path.clone(),
                side: found.anchor.side,
                start_line: found.anchor.start_line,
                end_line: found.anchor.end_line,
                reply_to: Some(thread.clone()),
            }
        };
        self.focused_thread = Some(thread.clone());
        if self
            .composer
            .as_ref()
            .is_some_and(|composer| composer.target == target)
        {
            self.focus_composer(window, cx);
            return;
        }
        if self.keep_composer(window, cx) {
            return;
        }
        self.opened_threads.insert(thread.clone());
        self.open_composer(target, window, cx);
    }

    /// A composer that holds text stays: it takes the keyboard and comes into view. `true`
    /// when there was one, shown or not: a draft whose file can't show it yet (a failed load)
    /// is kept for when it can, never replaced by a new composer.
    fn keep_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let has_text = self
            .composer
            .as_ref()
            .is_some_and(|composer| !composer.input.read(cx).value().trim().is_empty());
        if has_text {
            self.focus_composer(window, cx);
        }
        has_text
    }

    /// Gives the composer the keyboard and brings it into view, opening its file first when it
    /// is collapsed. A composer still without a row (its file failed to load) never takes the
    /// keyboard, which a field that isn't drawn would swallow: the stream keeps it, and `false`
    /// says so.
    fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(path) = self
            .composer
            .as_ref()
            .map(|composer| composer.target.path.clone())
        else {
            return false;
        };
        if self.composer_item().is_none() && self.is_collapsed(&path) {
            self.set_open(&path, true, cx);
        }
        let shown = match (self.composer_item(), &self.composer) {
            (Some(item), Some(composer)) => {
                composer
                    .input
                    .update(cx, |input, cx| input.focus(window, cx));
                self.reveal_item(item, 1);
                true
            }
            _ => {
                self.focus_handle.focus(window, cx);
                false
            }
        };
        self.viewport_changed(cx);
        cx.notify();
        shown
    }

    /// Gives list item `item` `focus` to carry (gpui keeps rendering an item whose handle has
    /// the keyboard), keeping the reading position where it is.
    fn set_item_focus(&mut self, item: usize, focus: Option<FocusHandle>) {
        let top = self.list.logical_scroll_top();
        self.list.splice_focusable(item..item + 1, [focus]);
        if top.item_ix == item {
            self.list.scroll_to(top);
        }
    }

    /// The focus handle list item `row` of `path` carries: the composer field's, on the row
    /// that holds the composer ([`holds_composer`]).
    pub(super) fn row_focus(&self, path: &str, row: &Row, cx: &App) -> Option<FocusHandle> {
        self.composer
            .as_ref()
            .filter(|composer| holds_composer(composer, path, row))
            .map(|composer| composer.input.focus_handle(cx))
    }

    /// Hands the composer's row its field's focus handle: after the list was reset, or when no
    /// rebuild did (a reply's card, which was there already).
    pub(super) fn attach_composer_focus(&mut self, cx: &App) {
        let handle = self
            .composer
            .as_ref()
            .map(|composer| composer.input.focus_handle(cx));
        if let (Some(item), Some(handle)) = (self.composer_item(), handle) {
            self.set_item_focus(item, Some(handle));
        }
    }

    /// Closes the composer: its row goes (a reply's card drops its handle) and the file's rows
    /// are rebuilt. The keyboard is the caller's to place.
    fn close_composer(&mut self, cx: &mut Context<Self>) -> Option<Composer> {
        let item = self
            .composer
            .as_ref()
            .filter(|composer| composer.target.reply_to.is_some())
            .and_then(|_| self.composer_item());
        let composer = self.composer.take()?;
        if let Some(item) = item {
            self.set_item_focus(item, None);
        }
        self.refresh_path(&composer.target.path, cx);
        Some(composer)
    }

    /// Hands the keyboard back to the stream when the composer's field holds it without a row
    /// to be drawn in (its file collapsed, failed to load or left the diff, its thread was
    /// deleted): every key would go nowhere otherwise. Runs as the stream is drawn.
    pub(super) fn release_orphaned_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let orphaned = self
            .composer_focus
            .as_ref()
            .is_some_and(|focus| focus.is_focused(window))
            && self.composer_item().is_none();
        if !orphaned || self.refocus_pending {
            return;
        }
        self.refocus_pending = true;
        cx.defer_in(window, |this, window, cx| {
            this.refocus_pending = false;
            let still = this
                .composer_focus
                .as_ref()
                .is_some_and(|focus| focus.is_focused(window))
                && this.composer_item().is_none();
            if still {
                this.focus_handle.focus(window, cx);
                cx.notify();
            }
        });
    }

    fn open_composer(
        &mut self,
        target: ComposerTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (rows, placeholder) = match target.reply_to {
            Some(_) => (REPLY_ROWS, "Reply… (Markdown)"),
            None => (COMPOSER_ROWS, "Leave a comment… (Markdown)"),
        };
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(rows.0, rows.1)
                .placeholder(placeholder)
        });
        let subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                // The field grows with its text: its row is measured again. Typed into while
                // scrolled out of view, it comes back into view, as an editor's caret does.
                if let Some(item) = this.composer_item() {
                    this.remeasure(item);
                    if this.is_off_screen(item) {
                        this.reveal_item(item, 1);
                        this.viewport_changed(cx);
                    }
                }
                cx.notify();
            }
        });
        // The composer this one replaces lets go of its row first.
        self.close_composer(cx);
        self.composer_focus = Some(input.focus_handle(cx));
        self.composer = Some(Composer {
            target: target.clone(),
            input,
            posting: false,
            error: None,
            _subscription: subscription,
        });
        self.refresh_path(&target.path, cx);
        // A reply's card was there already, so no rebuild gave it the field's handle.
        if target.reply_to.is_some() {
            self.attach_composer_focus(cx);
        }
        self.focus_composer(window, cx);
    }

    /// Whether item `item` is wholly out of the viewport.
    pub(super) fn is_off_screen(&self, item: usize) -> bool {
        let height = self.viewport_height();
        let size = self.item_height(item);
        self.offset_of(item, height * 2.)
            .is_none_or(|y| y + size <= 0. || y >= height)
    }

    /// Posts the composer's text: a new thread at its lines (`ops::add_thread`, the anchor from
    /// the loaded lines) or a reply (`ops::reply`). On success the composer closes, the thread
    /// is focused and the keyboard goes back to the stream; a failure stays in the composer.
    pub fn post_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(composer) = self.composer.as_mut() else {
            return;
        };
        if composer.posting {
            return;
        }
        let body = composer.input.read(cx).value().trim().to_string();
        if body.is_empty() {
            composer.error = Some(Failure::new("Write something first"));
            cx.notify();
            return;
        }
        let target = composer.target.clone();
        let posting: gpui::Task<Result<ThreadId, Failure>> = match &target.reply_to {
            Some(thread) => {
                let reply =
                    session.update(cx, |session, cx| session.reply(thread.clone(), body, cx));
                let thread = thread.clone();
                cx.background_spawn(async move { reply.await.map(|_| thread) })
            }
            None => {
                let anchor = session.read(cx).anchor_for(
                    &target.path,
                    target.side,
                    target.start_line,
                    target.end_line,
                );
                match anchor {
                    Ok(anchor) => {
                        session.update(cx, |session, cx| session.add_thread(anchor, body, cx))
                    }
                    Err(failure) => {
                        composer.error = Some(failure);
                        cx.notify();
                        return;
                    }
                }
            }
        };
        composer.posting = true;
        composer.error = None;
        self.writes += 1;
        cx.spawn_in(window, async move |this, cx| {
            let result = posting.await;
            this.update_in(cx, |this, window, cx| {
                this.writes -= 1;
                this.posted(target, result, window, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn posted(
        &mut self,
        target: ComposerTarget,
        result: Result<ThreadId, Failure>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .composer
            .as_mut()
            .filter(|composer| composer.target == target);
        match (result, current) {
            (Ok(thread), current) => {
                if current.is_some() {
                    self.close_composer(cx);
                    self.focus_handle.focus(window, cx);
                }
                if target.reply_to.is_none() {
                    self.selection = None;
                }
                self.focused_thread = Some(thread.clone());
                self.pending_thread = Some(thread);
                self.refresh_path(&target.path, cx);
            }
            (Err(failure), Some(composer)) => {
                composer.posting = false;
                composer.error = Some(failure);
            }
            (Err(failure), None) => log::warn!("a comment wasn't posted: {failure}"),
        }
        cx.notify();
    }

    /// Discards the composer (`Esc` in it) and hands the keyboard back to the stream.
    pub fn cancel_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.close_composer(cx).is_none() {
            return;
        }
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    /// Drops the composer without touching the keyboard (`Esc` in the stream). `false` when
    /// there was none.
    pub(super) fn drop_composer(&mut self, cx: &mut Context<Self>) -> bool {
        self.close_composer(cx).is_some()
    }

    /// `]` / `[`: focuses the next (previous) thread that isn't resolved, scrolling to it, on
    /// into later (earlier) files.
    pub(super) fn step_thread(&mut self, dir: Dir, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let unresolved: Vec<(usize, ThreadId)> = {
            let review = session.read(cx).review();
            self.thread_items()
                .into_iter()
                .filter(|(_, id)| {
                    review
                        .thread(id)
                        .is_some_and(|thread| thread.state != ThreadState::Resolved)
                })
                .collect()
        };
        let focused = self
            .focused_thread
            .as_ref()
            .and_then(|id| self.thread_item(id));
        let from = focused
            .or_else(|| self.cursor_item())
            .unwrap_or(self.list.logical_scroll_top().item_ix);
        let next = match dir {
            Dir::Down => unresolved.iter().find(|(item, _)| {
                if focused.is_some() {
                    *item > from
                } else {
                    *item >= from
                }
            }),
            Dir::Up => unresolved.iter().rev().find(|(item, _)| *item < from),
        };
        if let Some((item, id)) = next.cloned() {
            self.focus_item(item, id, cx);
            return;
        }
        // The next thread may live in a file whose cards aren't built: collapsed, or loading.
        let Some((from_block, _)) = self.locate(from) else {
            return;
        };
        let order: Vec<usize> = match dir {
            Dir::Down => (from_block + 1..self.blocks.len()).collect(),
            Dir::Up => (0..from_block).rev().collect(),
        };
        for index in order {
            let path = self.blocks[index].path.clone();
            let waiting: Vec<ThreadId> = session
                .read(cx)
                .threads_in(&path)
                .filter(|thread| thread.state != ThreadState::Resolved)
                .map(|thread| thread.id.clone())
                .collect();
            if waiting.is_empty() {
                continue;
            }
            let pick = match dir {
                Dir::Down => waiting.first(),
                Dir::Up => waiting.last(),
            };
            if let Some(id) = pick.cloned() {
                self.reveal_thread(&id, cx);
            }
            return;
        }
    }

    /// Focuses `thread` and scrolls to its card (a click on its row in the comments list).
    /// A thread whose card isn't built yet — its file collapsed or still loading — is shown
    /// once it is.
    pub fn reveal_thread(&mut self, thread: &ThreadId, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        if let Some(item) = self.thread_item(thread) {
            self.focus_item(item, thread.clone(), cx);
            return;
        }
        let path = {
            let session = session.read(cx);
            session
                .review()
                .thread(thread)
                .and_then(|found| session.file_of(found))
                .map(|entry| entry.path.clone())
        };
        let Some(path) = path else {
            // Its file left the diff: the comments list is the only place it shows.
            self.focused_thread = Some(thread.clone());
            cx.notify();
            return;
        };
        self.focused_thread = Some(thread.clone());
        self.pending_thread = Some(thread.clone());
        if self.is_collapsed(&path) {
            self.set_open(&path, true, cx);
        }
        if self.thread_item(thread).is_none()
            && let Some(&index) = self.index.get(&path)
        {
            self.go_to_file(index, cx);
        }
        self.reveal_pending_thread();
        self.viewport_changed(cx);
        cx.notify();
    }

    /// A click on a card: it becomes the focused thread.
    pub fn focus_thread(&mut self, thread: &ThreadId, cx: &mut Context<Self>) {
        if self.focused_thread.as_ref() != Some(thread) {
            self.focused_thread = Some(thread.clone());
            cx.notify();
        }
    }

    fn focus_item(&mut self, item: usize, thread: ThreadId, cx: &mut Context<Self>) {
        self.focused_thread = Some(thread);
        self.pending_thread = None;
        self.menu = None;
        self.reveal_item(item, 2);
        self.viewport_changed(cx);
        cx.notify();
    }

    /// `X`: resolves the focused thread, or reopens it when it is resolved.
    pub(super) fn resolve_focused(&mut self, cx: &mut Context<Self>) {
        let Some(thread) = self.focused_thread.clone() else {
            return;
        };
        let state = self.session.as_ref().and_then(|session| {
            session
                .read(cx)
                .review()
                .thread(&thread)
                .map(|found| found.state)
        });
        match state {
            Some(ThreadState::Resolved) => self.set_thread_state(&thread, ThreadState::Open, cx),
            Some(_) => self.set_thread_state(&thread, ThreadState::Resolved, cx),
            None => {}
        }
    }

    /// Resolves or reopens `thread` as the reviewer (`ops::set_thread_state`); nothing on a
    /// finished review. A resolved thread collapses.
    pub fn set_thread_state(
        &mut self,
        thread: &ThreadId,
        state: ThreadState,
        cx: &mut Context<Self>,
    ) {
        if self.read_only(cx) {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        if state == ThreadState::Resolved {
            self.opened_threads.remove(thread);
        }
        self.thread_error = None;
        let writing = session.update(cx, |session, cx| {
            session.set_thread_state(thread.clone(), state, None, cx)
        });
        self.writes += 1;
        let thread = thread.clone();
        cx.spawn(async move |this, cx| {
            let result = writing.await;
            this.update(cx, |this, cx| {
                this.writes -= 1;
                if let Err(failure) = result {
                    this.thread_error = Some((thread, failure));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Opens or collapses a resolved thread's card (its chevron).
    pub fn toggle_thread(&mut self, thread: &ThreadId, cx: &mut Context<Self>) {
        if !self.opened_threads.remove(thread) {
            self.opened_threads.insert(thread.clone());
        }
        if let Some(item) = self.thread_item(thread) {
            self.remeasure(item);
        }
        cx.notify();
    }

    /// Every thread card in the stream, as `(list item, thread)`, top to bottom.
    fn thread_items(&self) -> Vec<(usize, ThreadId)> {
        self.blocks
            .iter()
            .enumerate()
            .flat_map(|(block, rows)| {
                let start = self.starts[block];
                rows.rows.iter().enumerate().filter_map(move |(row, item)| {
                    item.thread().map(|id| (start + row, id.clone()))
                })
            })
            .collect()
    }

    pub(super) fn thread_item(&self, thread: &ThreadId) -> Option<usize> {
        self.thread_items()
            .into_iter()
            .find(|(_, id)| id == thread)
            .map(|(item, _)| item)
    }

    /// The composer's list item: its own row, or the card of the thread it answers.
    pub(super) fn composer_item(&self) -> Option<usize> {
        let composer = self.composer.as_ref()?;
        // Only its own file's rows can hold it.
        let block = *self.index.get(&composer.target.path)?;
        self.blocks[block]
            .rows
            .iter()
            .position(|row| holds_composer(composer, &composer.target.path, row))
            .map(|row| self.starts[block] + row)
    }

    /// Scrolls to the thread `]`/`[` or the comments list went to, once its card is built.
    pub(super) fn reveal_pending_thread(&mut self) {
        let Some(thread) = self.pending_thread.clone() else {
            return;
        };
        if let Some(item) = self.thread_item(&thread) {
            self.pending_thread = None;
            self.reveal_item(item, 2);
        }
    }

    /// Whether `path`'s rows are its header alone.
    fn is_collapsed(&self, path: &str) -> bool {
        self.index.get(path).is_some_and(|&block| {
            matches!(
                self.blocks[block].rows.first(),
                Some(Row::Header {
                    collapsed: true,
                    ..
                })
            )
        })
    }

    /// Rebuilds `path`'s rows (the composer moved, or a thread's card changed size).
    fn refresh_path(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(&index) = self.index.get(path) else {
            return;
        };
        let changed = self.refresh_block(index, cx);
        self.remeasure_cards(index);
        if changed {
            self.after_rows_changed(cx);
        } else {
            cx.notify();
        }
    }

    /// Measures block `index`'s cards and composer again: their content can change without
    /// their rows changing.
    pub(super) fn remeasure_cards(&mut self, index: usize) {
        let start = self.starts[index];
        let rows: Vec<usize> = self.blocks[index]
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| !row.has_fixed_height())
            .map(|(row, _)| start + row)
            .collect();
        for item in rows {
            self.remeasure(item);
        }
    }

    /// Forgets the focused, pending and opened threads that no longer exist (deleted), and
    /// drops a reply to one.
    pub(super) fn forget_gone_threads(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let review = session.read(cx).review();
        let exists = |id: &ThreadId| review.thread(id).is_some_and(|thread| !thread.is_deleted());
        if self.focused_thread.as_ref().is_some_and(|id| !exists(id)) {
            self.focused_thread = None;
        }
        if self.pending_thread.as_ref().is_some_and(|id| !exists(id)) {
            self.pending_thread = None;
        }
        self.opened_threads.retain(exists);
        let reply_gone = self
            .composer
            .as_ref()
            .and_then(|composer| composer.target.reply_to.as_ref())
            .is_some_and(|thread| !exists(thread));
        if reply_gone {
            self.close_composer(cx);
        }
    }
}
