//! The stream's keys and pointer moves (DESIGN §5 and its keyboard additions): files and change
//! blocks, the cursor and the selection, gaps, viewed, split / unified and pages.

use dv_core::diff::Expand;
use dv_core::review::AnchorSide;
use gpui::{App, Context, Entity, InteractiveElement, ListOffset, Window, px};

use super::rows::{
    Dir, LinePos, Row, cell_pos, change_pos, change_starts, end_cell, find_row, heights,
    nearest_gap, other, step,
};
use super::{Cursor, DiffPane, SelectionAnchor};
use crate::actions::diff_pane::{
    ClearSelection, Comment, CursorDown, CursorLeft, CursorRight, CursorUp, ExpandGap,
    ExpandGapFully, NextChange, NextFile, NextThread, PageDown, PageUp, PreviousChange,
    PreviousFile, PreviousThread, Reply, ResolveThread, SelectDown, SelectUp, ToggleSplit,
    ToggleViewed,
};
use crate::services::ViewMode;

/// Binds `$action` on `$element` to the pane's `$method`.
macro_rules! bind {
    ($element:expr, $pane:expr, $($action:ty => $method:ident),* $(,)?) => {
        $element$(.on_action({
            let pane = $pane.clone();
            move |_: &$action, _: &mut Window, cx: &mut App| {
                pane.update(cx, |this, cx| this.$method(cx));
            }
        }))*
    };
}

impl DiffPane {
    /// Registers the stream's actions on `element`: the main area, which the shell renders with
    /// this pane's focus handle and the `DiffPane` key context, so the keys work wherever the
    /// main area has the keyboard (the stream, its loading and empty states).
    pub fn bind_actions<E: InteractiveElement>(element: E, pane: &Entity<Self>) -> E {
        let clear = {
            let pane = pane.clone();
            move |_: &ClearSelection, _: &mut Window, cx: &mut App| {
                if !pane.update(cx, |this, cx| this.clear_selection(cx)) {
                    cx.propagate();
                }
            }
        };
        let comment = {
            let pane = pane.clone();
            move |_: &Comment, window: &mut Window, cx: &mut App| {
                pane.update(cx, |this, cx| this.comment(window, cx));
            }
        };
        let reply = {
            let pane = pane.clone();
            move |_: &Reply, window: &mut Window, cx: &mut App| {
                pane.update(cx, |this, cx| this.reply(window, cx));
            }
        };
        bind!(
            element
                .on_action(clear)
                .on_action(comment)
                .on_action(reply),
            pane,
            NextThread => next_thread,
            PreviousThread => previous_thread,
            ResolveThread => resolve_focused,
            NextFile => next_file,
            PreviousFile => previous_file,
            NextChange => next_change,
            PreviousChange => previous_change,
            ToggleViewed => toggle_viewed_current,
            ExpandGap => expand_step,
            ExpandGapFully => expand_fully,
            ToggleSplit => toggle_split,
            CursorUp => cursor_up,
            CursorDown => cursor_down,
            SelectUp => select_up,
            SelectDown => select_down,
            CursorLeft => cursor_left,
            CursorRight => cursor_right,
            PageUp => page_up,
            PageDown => page_down,
        )
    }

    fn current_index(&self) -> Option<usize> {
        self.current_file
            .as_ref()
            .and_then(|path| self.index.get(path).copied())
    }

    /// Shows file `index`: its header at the top, the cursor on its first change (once its
    /// diff lands, if it hasn't).
    pub(super) fn go_to_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(block) = self.blocks.get(index) else {
            return;
        };
        let path = block.path.clone();
        self.list.scroll_to(ListOffset {
            item_ix: self.starts[index],
            offset_in_item: px(0.),
        });
        self.selection = None;
        self.menu = None;
        self.cursor = self.first_change(index).map(|pos| Cursor {
            path: path.clone(),
            pos,
        });
        self.pending_first_change =
            (self.cursor.is_none() && self.lines(&path).is_none()).then_some(path);
        self.viewport_changed(cx);
        cx.notify();
    }

    /// Where the cursor goes in file `index`: its first change, else its first line.
    pub(super) fn first_change(&self, index: usize) -> Option<LinePos> {
        let block = self.blocks.get(index)?;
        let lines = self.lines(&block.path)?;
        change_starts(&block.rows, lines)
            .first()
            .and_then(|&row| change_pos(&block.rows[row], lines, self.mode))
            .or_else(|| end_cell(&block.rows, lines, self.mode, Dir::Down, None).map(|(_, p)| p))
    }

    fn next_file(&mut self, cx: &mut Context<Self>) {
        let next = self.current_index().map_or(0, |index| index + 1);
        if next < self.blocks.len() {
            self.go_to_file(next, cx);
        }
    }

    fn previous_file(&mut self, cx: &mut Context<Self>) {
        if let Some(previous) = self.current_index().and_then(|index| index.checked_sub(1)) {
            self.go_to_file(previous, cx);
        }
    }

    /// Marks file `index` viewed (unless it is) and shows the next one: the footer's "Mark
    /// viewed and go next".
    pub(super) fn mark_viewed_and_next(&mut self, index: usize, cx: &mut Context<Self>) {
        // A toggle still landing counts as what it asked for.
        if let Some(path) = self.blocks.get(index).map(|block| block.path.clone())
            && !self
                .session
                .as_ref()
                .is_some_and(|session| session.read(cx).intends_viewed(&path))
        {
            self.toggle_viewed(&path, cx);
        }
        self.go_to_file(index + 1, cx);
    }

    fn toggle_viewed_current(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.current_file.clone() {
            self.toggle_viewed(&path, cx);
        }
    }

    fn toggle_split(&mut self, cx: &mut Context<Self>) {
        self.set_view_mode(self.mode.toggled(), cx);
    }

    /// Where the keys start from: the cursor's row, else the row at the top of the viewport.
    fn reference(&self) -> Option<(usize, usize)> {
        let item = self
            .cursor_item()
            .unwrap_or(self.list.logical_scroll_top().item_ix);
        self.locate(item).or_else(|| {
            self.blocks
                .len()
                .checked_sub(1)
                .map(|last| (last, usize::MAX))
        })
    }

    fn next_change(&mut self, cx: &mut Context<Self>) {
        self.go_to_change(Dir::Down, cx);
    }

    fn previous_change(&mut self, cx: &mut Context<Self>) {
        self.go_to_change(Dir::Up, cx);
    }

    /// Moves the cursor to the next (previous) block of changed lines, into later (earlier)
    /// files when this one has no more.
    fn go_to_change(&mut self, dir: Dir, cx: &mut Context<Self>) {
        let Some((from_block, from_row)) = self.reference() else {
            return;
        };
        let order: Vec<usize> = match dir {
            Dir::Down => (from_block..self.blocks.len()).collect(),
            Dir::Up => (0..=from_block).rev().collect(),
        };
        for index in order {
            let block = &self.blocks[index];
            let Some(lines) = self.lines(&block.path) else {
                continue;
            };
            let starts = change_starts(&block.rows, lines);
            let row = match (dir, index == from_block) {
                (Dir::Down, true) => starts.into_iter().find(|&row| row > from_row),
                (Dir::Down, false) => starts.first().copied(),
                (Dir::Up, true) => starts.into_iter().rev().find(|&row| row < from_row),
                (Dir::Up, false) => starts.last().copied(),
            };
            if let Some(row) = row
                && let Some(pos) = change_pos(&block.rows[row], lines, self.mode)
            {
                self.cursor = Some(Cursor {
                    path: block.path.clone(),
                    pos,
                });
                self.selection = None;
                self.menu = None;
                let item = self.starts[index] + row;
                self.reveal_item(item, 3);
                self.viewport_changed(cx);
                cx.notify();
                return;
            }
        }
    }

    fn cursor_up(&mut self, cx: &mut Context<Self>) {
        self.move_cursor(Dir::Up, false, cx);
    }

    fn cursor_down(&mut self, cx: &mut Context<Self>) {
        self.move_cursor(Dir::Down, false, cx);
    }

    fn select_up(&mut self, cx: &mut Context<Self>) {
        self.move_cursor(Dir::Up, true, cx);
    }

    fn select_down(&mut self, cx: &mut Context<Self>) {
        self.move_cursor(Dir::Down, true, cx);
    }

    /// Moves the cursor a line; with `extend`, the selection grows from where the cursor was,
    /// within its file and side. Without a cursor it starts on the first line in view.
    fn move_cursor(&mut self, dir: Dir, extend: bool, cx: &mut Context<Self>) {
        self.menu = None;
        let Some(cursor) = self.cursor.clone() else {
            self.cursor = self.first_visible_cell();
            self.selection = None;
            self.after_cursor_move(cx);
            return;
        };
        let Some(&index) = self.index.get(&cursor.path) else {
            return;
        };
        let side = match (self.mode, extend) {
            (ViewMode::Split, _) => Some(cursor.pos.side),
            (ViewMode::Unified, true) => Some(
                self.selection
                    .as_ref()
                    .map_or(cursor.pos.side, |anchor| anchor.side),
            ),
            (ViewMode::Unified, false) => None,
        };
        // A selection never jumps a gap, which would take in lines nobody was shown: the gap
        // opens a step towards the cursor instead (as `E` does), and the cursor moves onto the
        // first line that reveals.
        if extend && let Some((ordinal, expand)) = self.gap_ahead(index, cursor.pos, dir, side) {
            self.expand(&cursor.path, ordinal, expand, cx);
        }
        let Some(lines) = self.lines(&cursor.path) else {
            return;
        };
        let rows = &self.blocks[index].rows;
        let within = match find_row(rows, lines, self.mode, cursor.pos) {
            Some(row) => step(rows, lines, self.mode, row, dir, side),
            // The cursor's row is hidden (its file collapsed): start over from the file's edge.
            None => end_cell(rows, lines, self.mode, dir, side),
        };
        let moved = match within {
            Some((_, pos)) => Some(Cursor {
                path: cursor.path.clone(),
                pos,
            }),
            // The next file on the side the reviewer chose, whichever one this file left the
            // cursor on (`chosen_side`).
            None if !extend => self.cell_in_next_file(index, dir, self.chosen_side),
            None => None,
        };
        let Some(moved) = moved else {
            return;
        };
        if extend {
            self.selection.get_or_insert(SelectionAnchor {
                path: cursor.path.clone(),
                side: side.unwrap_or(cursor.pos.side),
                line: cursor.pos.line,
            });
        } else {
            self.selection = None;
        }
        self.cursor = Some(moved);
        self.after_cursor_move(cx);
    }

    /// The gap between `pos` and the next place for the cursor in `dir` within file `index`
    /// (or the file's edge), as its ordinal and the step that reveals the lines next to `pos`.
    fn gap_ahead(
        &self,
        index: usize,
        pos: LinePos,
        dir: Dir,
        side: Option<AnchorSide>,
    ) -> Option<(usize, Expand)> {
        let block = self.blocks.get(index)?;
        let lines = self.lines(&block.path)?;
        let rows = &block.rows;
        let row = find_row(rows, lines, self.mode, pos)?;
        let next = step(rows, lines, self.mode, row, dir, side).map(|(next, _)| next);
        let between: Vec<usize> = match dir {
            Dir::Down => (row + 1..next.unwrap_or(rows.len())).collect(),
            Dir::Up => (next.map_or(0, |next| next + 1)..row).rev().collect(),
        };
        between.into_iter().find_map(|at| match &rows[at] {
            Row::Gap(gap) => Some((
                gap.ordinal,
                match dir {
                    Dir::Down if gap.above => Expand::FromAbove,
                    Dir::Up if gap.below => Expand::FromBelow,
                    _ => Expand::All,
                },
            )),
            _ => None,
        })
    }

    /// The first (last) line of the next (previous) file that shows lines, on `side` in split
    /// view when that file has lines there.
    fn cell_in_next_file(&self, from: usize, dir: Dir, side: AnchorSide) -> Option<Cursor> {
        let order: Vec<usize> = match dir {
            Dir::Down => (from + 1..self.blocks.len()).collect(),
            Dir::Up => (0..from).rev().collect(),
        };
        let side = (self.mode == ViewMode::Split).then_some(side);
        order.into_iter().find_map(|index| {
            let block = &self.blocks[index];
            let lines = self.lines(&block.path)?;
            end_cell(&block.rows, lines, self.mode, dir, side).map(|(_, pos)| Cursor {
                path: block.path.clone(),
                pos,
            })
        })
    }

    /// The first line in view below the pinned header.
    pub(super) fn first_visible_cell(&self) -> Option<Cursor> {
        let height = self.viewport_height();
        let top = self.list.logical_scroll_top();
        let mut y = -f32::from(top.offset_in_item);
        let mut item = top.item_ix;
        while y < height && item + 1 < self.item_count() {
            // A row whose bottom edge meets the pinned header's is hidden under it.
            if y > heights::HEADER - heights::CODE
                && let Some(cursor) = self.cell_of_item(item, None)
            {
                return Some(cursor);
            }
            y += self.item_height(item);
            item += 1;
        }
        None
    }

    /// Where the cursor can sit on item `item`, on `side` when given (else split view prefers
    /// the side the reviewer chose, `chosen_side`).
    fn cell_of_item(&self, item: usize, side: Option<AnchorSide>) -> Option<Cursor> {
        let (block, row) = self.locate(item)?;
        let block = &self.blocks[block];
        let lines = self.lines(&block.path)?;
        let row = &block.rows[row];
        let pos = match (self.mode, side) {
            (ViewMode::Unified, _) => cell_pos(row, lines, self.mode, None),
            (ViewMode::Split, Some(side)) => cell_pos(row, lines, self.mode, Some(side)),
            (ViewMode::Split, None) => cell_pos(row, lines, self.mode, Some(self.chosen_side))
                .or_else(|| cell_pos(row, lines, self.mode, Some(other(self.chosen_side)))),
        }?;
        Some(Cursor {
            path: block.path.clone(),
            pos,
        })
    }

    fn after_cursor_move(&mut self, cx: &mut Context<Self>) {
        if let Some(item) = self.cursor_item() {
            self.reveal_item(item, 0);
        }
        self.viewport_changed(cx);
        cx.notify();
    }

    /// Scrolls the cursor into view, if it is shown.
    pub(super) fn reveal_cursor(&mut self, cx: &mut Context<Self>) {
        self.after_cursor_move(cx);
    }

    fn cursor_left(&mut self, cx: &mut Context<Self>) {
        self.switch_side(AnchorSide::Old, cx);
    }

    fn cursor_right(&mut self, cx: &mut Context<Self>) {
        self.switch_side(AnchorSide::New, cx);
    }

    /// Puts the cursor on `side` in split view: the same row's line there, else the nearest
    /// line on that side. It is the side the cursor keeps from then on (`chosen_side`), also
    /// when this file has no line there.
    fn switch_side(&mut self, side: AnchorSide, cx: &mut Context<Self>) {
        if self.mode != ViewMode::Split {
            return;
        }
        self.chosen_side = side;
        let Some(cursor) = self.cursor.clone().filter(|cursor| cursor.pos.side != side) else {
            return;
        };
        let (Some(&index), Some(lines)) = (self.index.get(&cursor.path), self.lines(&cursor.path))
        else {
            return;
        };
        let rows = &self.blocks[index].rows;
        let Some(row) = find_row(rows, lines, self.mode, cursor.pos) else {
            return;
        };
        let pos = rows[row]
            .cell(lines, side)
            .map(|line| LinePos { line, side })
            .or_else(|| {
                let up = step(rows, lines, self.mode, row, Dir::Up, Some(side));
                let down = step(rows, lines, self.mode, row, Dir::Down, Some(side));
                match (up, down) {
                    (Some(up), Some(down)) => Some(if row - up.0 <= down.0 - row {
                        up.1
                    } else {
                        down.1
                    }),
                    (up, down) => up.or(down).map(|(_, pos)| pos),
                }
            });
        if let Some(pos) = pos {
            self.selection = None;
            self.cursor = Some(Cursor {
                path: cursor.path,
                pos,
            });
            self.after_cursor_move(cx);
        }
    }

    fn page_up(&mut self, cx: &mut Context<Self>) {
        self.page(Dir::Up, cx);
    }

    fn page_down(&mut self, cx: &mut Context<Self>) {
        self.page(Dir::Down, cx);
    }

    /// Scrolls a page (less the pinned header and a line of overlap); a cursor on screen keeps
    /// its place on screen, on the line that is there now.
    fn page(&mut self, dir: Dir, cx: &mut Context<Self>) {
        self.menu = None;
        let height = self.viewport_height();
        let distance = (height - heights::HEADER - heights::CODE).max(heights::CODE);
        let cursor_y = self
            .cursor_item()
            .and_then(|item| self.offset_of(item, height))
            .filter(|y| (0. ..height).contains(y));
        let side = self.cursor.as_ref().map(|cursor| cursor.pos.side);
        let top = self.list.logical_scroll_top();
        let delta = match dir {
            Dir::Down => distance,
            Dir::Up => -distance,
        };
        // No further than the list itself scrolls, so the cursor lands where it will show.
        let target = self.offset_by(top, delta);
        let max = self.max_scroll_top();
        let past_end = (target.item_ix, f32::from(target.offset_in_item))
            > (max.item_ix, f32::from(max.offset_in_item));
        self.list.scroll_to(if past_end { max } else { target });
        if let Some(y) = cursor_y
            && let Some(cursor) = self.cell_at(y, side)
        {
            self.cursor = Some(cursor);
        }
        self.selection = None;
        self.viewport_changed(cx);
        cx.notify();
    }

    /// The line at `y` (from the viewport's top), or the next one below, on `side` in split
    /// view.
    fn cell_at(&self, y: f32, side: Option<AnchorSide>) -> Option<Cursor> {
        let height = self.viewport_height();
        let top = self.list.logical_scroll_top();
        let mut at = -f32::from(top.offset_in_item);
        let mut item = top.item_ix;
        let side = side.filter(|_| self.mode == ViewMode::Split);
        while at < height && item + 1 < self.item_count() {
            let size = self.item_height(item);
            // The row whose top is nearest `y`, so paging back and forth lands where it began.
            if at + size / 2. > y
                && let Some(cursor) = self.cell_of_item(item, side)
            {
                return Some(cursor);
            }
            at += size;
            item += 1;
        }
        None
    }

    fn expand_step(&mut self, cx: &mut Context<Self>) {
        self.expand_nearest(false, cx);
    }

    fn expand_fully(&mut self, cx: &mut Context<Self>) {
        self.expand_nearest(true, cx);
    }

    /// Expands the gap nearest the cursor (else the top of the viewport): one step towards the
    /// cursor, or all of it.
    fn expand_nearest(&mut self, full: bool, cx: &mut Context<Self>) {
        let Some((index, from)) = self.reference() else {
            return;
        };
        let rows = &self.blocks[index].rows;
        let from = from.min(rows.len().saturating_sub(1));
        let Some(gap_row) = nearest_gap(rows, from) else {
            return;
        };
        let Row::Gap(gap) = &rows[gap_row] else {
            return;
        };
        let expand = match (full, gap_row > from) {
            (true, _) => Expand::All,
            // Below the cursor: reveal the lines that continue down from it.
            (false, true) if gap.above => Expand::FromAbove,
            (false, true) => Expand::FromBelow,
            // Above it: reveal the lines that lead into it.
            (false, false) if gap.below => Expand::FromBelow,
            (false, false) => Expand::FromAbove,
        };
        let (path, ordinal) = (self.blocks[index].path.clone(), gap.ordinal);
        self.expand(&path, ordinal, expand, cx);
        if self.cursor.is_some() {
            self.reveal_cursor(cx);
        }
    }

    /// Closes the file menu, else drops the composer, else clears the selection (DESIGN §5's
    /// `Esc`); `false` when there was none of them.
    fn clear_selection(&mut self, cx: &mut Context<Self>) -> bool {
        if self.menu.take().is_some() || self.drop_composer(cx) || self.selection.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    fn next_thread(&mut self, cx: &mut Context<Self>) {
        self.step_thread(Dir::Down, cx);
    }

    fn previous_thread(&mut self, cx: &mut Context<Self>) {
        self.step_thread(Dir::Up, cx);
    }

    /// A click on a line number: the cursor goes there; with Shift, the selection reaches from
    /// the cursor to it (same file, and in split view the same side).
    pub(super) fn click_line(
        &mut self,
        path: &str,
        pos: LinePos,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        let extendable = self.cursor.as_ref().filter(|cursor| {
            cursor.path == path && (self.mode == ViewMode::Unified || cursor.pos.side == pos.side)
        });
        match (extend, extendable) {
            (true, Some(cursor)) => {
                let anchor = SelectionAnchor {
                    path: cursor.path.clone(),
                    side: cursor.pos.side,
                    line: cursor.pos.line,
                };
                self.selection.get_or_insert(anchor);
            }
            _ => self.selection = None,
        }
        if self.mode == ViewMode::Split {
            self.chosen_side = pos.side;
        }
        self.cursor = Some(Cursor {
            path: path.to_string(),
            pos,
        });
        self.viewport_changed(cx);
        cx.notify();
    }
}
