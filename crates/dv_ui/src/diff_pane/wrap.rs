//! Long lines wrapping (DESIGN §4.1 "Wrap"): with the `wrap` setting on, a line of code wraps at
//! the code column's width ([`text::wrap`]) instead of clipping and scrolling sideways, each of
//! its visual lines a [`Segment`] that paints as its own slice of columns (`render::code`). A
//! code row is [`heights::CODE`] for each visual line of its longer side, so a split row's
//! shorter side leaves room under its text; its numbers and marker stay on its first line.
//!
//! Each frame, before the list lays out, the setting and the code column's width are read
//! ([`DiffPane::update_wrap`]); when either changed the segments are forgotten and every row is
//! measured again, the reading position staying on its row. The segments of a line are worked
//! out when its row is first measured or drawn and kept until then, or until its file's diff
//! changes ([`DiffPane::forget_segments`]).

use std::rc::Rc;

use dv_core::review::AnchorSide;
use gpui::Context;

use super::DiffPane;
use super::render::code::{MARKER_WIDTH, NUMBER_WIDTH, TEXT_INSET};
use super::rows::{Row, heights, natural_side};
use super::text::{self, Segment};
use crate::services::{ActiveServices as _, ViewMode};

impl DiffPane {
    /// Follows the `wrap` setting and, while it is on, the code column's width in whole
    /// columns (module docs).
    pub(super) fn update_wrap(&mut self, cx: &mut Context<Self>) {
        let wrap = cx
            .try_services()
            .is_some_and(|services| services.settings.read(cx).wrap());
        let width = if wrap { self.fitting_columns() } else { 0 };
        if wrap == self.wrap && width == self.wrap_width {
            return;
        }
        self.wrap = wrap;
        self.wrap_width = width;
        self.segments.borrow_mut().clear();
        if wrap {
            self.h_offset = 0.;
        }
        let count = self.item_count();
        self.list.remeasure_items(0..count);
        self.rows_version += 1;
        cx.notify();
    }

    /// Whether long lines wrap.
    pub(super) fn wraps(&self) -> bool {
        self.wrap
    }

    /// How many whole columns of code fit the code cell (a side's in split view): the width
    /// lines wrap at.
    fn fitting_columns(&self) -> usize {
        let width = f32::from(self.list.viewport_bounds().size.width);
        let width = if width > 0. { width } else { 1160. };
        let cell = match self.mode {
            ViewMode::Unified => {
                width - self.number_columns() * NUMBER_WIDTH - MARKER_WIDTH - TEXT_INSET
            }
            ViewMode::Split => width / 2. - NUMBER_WIDTH - MARKER_WIDTH - TEXT_INSET - 1.,
        };
        let char_width = self.char_width.unwrap_or(7.2).max(1.);
        // A column's worth of room at the end, so a line's last character never meets the edge.
        ((cell.max(0.) / char_width).floor() as usize)
            .saturating_sub(1)
            .max(1)
    }

    /// The visual lines of `path`'s line `line` (an index into its lines) on `side` while lines
    /// wrap; `None` when they don't, or the line isn't loaded.
    pub(super) fn segments(
        &self,
        path: &str,
        side: AnchorSide,
        line: usize,
    ) -> Option<Rc<Vec<Segment>>> {
        if !self.wrap {
            return None;
        }
        let key = (path.to_string(), side, line);
        if let Some(found) = self.segments.borrow().get(&key) {
            return Some(found.clone());
        }
        let segments = Rc::new(text::wrap(&self.line(path, line)?.text, self.wrap_width));
        self.segments.borrow_mut().insert(key, segments.clone());
        Some(segments)
    }

    /// How many visual lines `path`'s line `line` takes on `side`: one unless it wraps.
    pub(super) fn visual_lines(&self, path: &str, side: AnchorSide, line: usize) -> usize {
        self.segments(path, side, line)
            .map_or(1, |segments| segments.len())
    }

    /// `row`'s height in `path`: a code row's for its longer side's visual lines (module docs),
    /// any other row's own ([`Row::height`]).
    pub(super) fn row_height(&self, path: &str, row: &Row) -> f32 {
        let lines = match *row {
            Row::Line(line) => {
                let side = self.line(path, line).map_or(AnchorSide::New, natural_side);
                self.visual_lines(path, side, line)
            }
            Row::Pair { old, new } => {
                let on = |line: Option<usize>, side| {
                    line.map_or(1, |line| self.visual_lines(path, side, line))
                };
                on(old, AnchorSide::Old).max(on(new, AnchorSide::New))
            }
            _ => return row.height(),
        };
        heights::CODE * lines as f32
    }

    /// Forgets the segments of `path`'s lines: its diff was replaced, so its lines are other
    /// lines now.
    pub(super) fn forget_segments(&self, path: &str) {
        self.segments
            .borrow_mut()
            .retain(|(file, _, _), _| file != path);
    }
}
