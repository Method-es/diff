//! The stream's viewport (`DiffPane`, module docs of the parent): the spacer after the last
//! file, the files to load around what is in view, the current file and the pinned header, and
//! positions worked out from row heights without waiting for a layout.

use std::collections::HashSet;
use std::time::Instant;

use gpui::{Context, ListOffset, px};

use super::rows::{Row, find_row, heights};
use super::{
    DEFAULT_VIEWPORT, DiffPane, DiffPaneEvent, PREFETCH_AHEAD, PREFETCH_BEHIND, SKELETON_FOR,
    ViewportKey,
};
use crate::session::FileState;

impl DiffPane {
    pub(super) fn viewport_height(&self) -> f32 {
        let height = f32::from(self.list.viewport_bounds().size.height);
        if height > 0. {
            height
        } else {
            DEFAULT_VIEWPORT
        }
    }

    pub(super) fn current_viewport_key(&self) -> ViewportKey {
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
    pub(super) fn update_spacer(&mut self) {
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
    pub(super) fn viewport_changed(&mut self, cx: &mut Context<Self>) {
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
        // The file view's file loads through the session's own slot for it, not the stream's.
        if wanted != self.wanted && !self.is_file_view() {
            self.wanted = wanted.clone();
            if let Some(session) = self.session.clone() {
                session.update(cx, |session, cx| session.ensure_loaded(wanted, cx));
            }
        }
        self.update_current(cx);
        self.viewport_key = Some(self.current_viewport_key());
    }

    /// The first and last block with a row in view.
    pub(super) fn visible_blocks(&self, height: f32) -> Option<(usize, usize)> {
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

    pub(super) fn update_current(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn compute_current(&self) -> Option<usize> {
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
    pub(super) fn pinned(&self) -> Option<(usize, f32)> {
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
    pub(super) fn offset_of(&self, item: usize, limit: f32) -> Option<f32> {
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
    pub(super) fn scroll_item_to(&mut self, item: usize, y: f32) {
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
    pub(super) fn reveal_item(&mut self, item: usize, margin: usize) {
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

    /// Scrolls up just enough that item `item`'s bottom sits `margin` rows above the viewport's
    /// bottom, when it is below that: the end of a tall item (a card's composer) comes into view
    /// while the rest of it stays where it is, its top going above the viewport if it must.
    pub(super) fn reveal_item_bottom(&mut self, item: usize, margin: usize) {
        let height = self.viewport_height();
        let size = self.item_height(item);
        let margin = margin as f32 * heights::CODE;
        if let Some(y) = self.offset_of(item, height * 2.)
            && y + size > height - margin
        {
            self.scroll_item_to(item, height - size - margin);
        }
    }

    /// The list offset `delta` pixels below (or above, negative) `from`, clamped to the list.
    pub(super) fn offset_by(&self, from: ListOffset, delta: f32) -> ListOffset {
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
    pub(super) fn max_scroll_top(&self) -> ListOffset {
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
    pub(super) fn cursor_item(&self) -> Option<usize> {
        let cursor = self.cursor.as_ref()?;
        let block = *self.index.get(&cursor.path)?;
        let lines = self.lines(&cursor.path)?;
        find_row(&self.blocks[block].rows, lines, self.mode, cursor.pos)
            .map(|row| self.starts[block] + row)
    }

    pub(super) fn clamp_h_offset(&mut self) {
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
    pub(super) fn track_loading(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn slow_to_load(&self, path: &str) -> bool {
        self.loading_since
            .get(path)
            .is_some_and(|since| since.elapsed() >= SKELETON_FOR)
    }
}
