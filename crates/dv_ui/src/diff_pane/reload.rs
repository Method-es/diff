//! A file's diff loading again (PLAN §11 step 7, DESIGN §4.4): the agent saved it, staged it or
//! committed part of it while the reviewer reads. The same lines again keep everything as it
//! was. Other lines are a new diff, and what the stream kept by line index (the context the
//! reviewer opened, the lines on screen, the cursor, the selection, the composer's lines) is
//! carried onto it through [`remap_lines`], so an edit elsewhere in the file moves nothing the
//! reviewer is looking at.
//!
//! A diff listed under the other comparison (PLAN §2.3: an interdiff's old side is round n's
//! snapshot, all changes' the target's base) has another old side: a selection there is
//! dropped, and a draft comment on the old side keeps the lines it was opened on, which mean
//! nothing in the other text, until its own comparison shows again (an interdiff doesn't draw
//! it, nor does all changes while the file still shows the interdiff's diff, `threads`).

use std::sync::Arc;

use dv_core::diff::{DiffLine, FileDiff, Segment, remap_lines, segments, segments_showing};
use dv_core::review::AnchorSide;

use super::rows::{CONTEXT_LINES, LinePos, Row, on_side, side_number};
use super::text::columns;
use super::{DiffPane, same_lines};
use crate::session::Comparison;

impl DiffPane {
    /// Takes `diff` as `path`'s loaded diff, when it isn't already (see the module docs).
    /// `threads` are the lines (indexes into `diff`'s lines) of the threads placed there, which
    /// stay shown so each card follows its own last line rather than a gap (DESIGN §4.1): a
    /// thread relocation moved can stand on lines no change is near.
    pub(super) fn take_diff(
        &mut self,
        path: &str,
        diff: &Arc<FileDiff>,
        comparison: Comparison,
        threads: &[usize],
    ) {
        let kept = self.files.get(path).and_then(|ui| ui.diff.clone());
        if kept.as_ref().is_some_and(|kept| Arc::ptr_eq(kept, diff)) {
            return;
        }
        // The selection and the caret named the text the file had (other lines now).
        if kept.as_ref().is_some_and(|kept| !same_lines(kept, diff)) {
            self.forget_text_in(path);
        }
        let across = self
            .files
            .get(path)
            .and_then(|ui| ui.comparison)
            .is_some_and(|kept| kept != comparison);
        // The same lines again (staged, rescanned): the gaps stay as expanded.
        if !kept.as_ref().is_some_and(|kept| same_lines(kept, diff)) {
            let carried = kept.and_then(|kept| self.carry_over(path, &kept, diff, across, threads));
            let ui = self.files.entry(path.to_string()).or_default();
            ui.segments = carried.unwrap_or_else(|| match diff.as_text() {
                Some(text) => segments_showing(&text.lines, CONTEXT_LINES, &mask(threads)),
                None => diff.segments(CONTEXT_LINES),
            });
            ui.columns = diff
                .as_text()
                .and_then(|text| text.lines.iter().map(|line| columns(&line.text)).max())
                .unwrap_or(0);
        }
        let ui = self.files.entry(path.to_string()).or_default();
        ui.diff = Some(diff.clone());
        ui.comparison = Some(comparison);
    }

    /// Shows `threads`' lines (indexes into `diff`'s lines, `diff` the one `path` holds) where
    /// a gap hides them, keeping everything else shown as it is: a thread placed after its
    /// file's diff was taken (its round's history landed later, PLAN §8) still has its lines
    /// shown under its card, as [`DiffPane::take_diff`] keeps them.
    pub(super) fn show_thread_lines(
        &mut self,
        path: &str,
        diff: &Arc<FileDiff>,
        threads: &[usize],
    ) {
        let Some(text) = diff.as_text() else {
            return;
        };
        let Some(ui) = self
            .files
            .get_mut(path)
            .filter(|ui| ui.diff.as_ref().is_some_and(|kept| Arc::ptr_eq(kept, diff)))
        else {
            return;
        };
        let mut shown = shown_mask(&ui.segments, text.lines.len());
        let hidden = threads
            .iter()
            .filter(|&&line| line < shown.len() && !shown[line])
            .count();
        if hidden == 0 {
            return;
        }
        let len = shown.len();
        for &line in threads.iter().filter(|&&line| line < len) {
            shown[line] = true;
        }
        ui.segments = segments_showing(&text.lines, CONTEXT_LINES, &shown);
    }

    /// Carries what the stream holds of `path` from `from`'s lines onto `to`'s: the cursor, the
    /// selection and the composer move with their lines, and the new segments, returned, keep
    /// shown what the reviewer opened beyond the usual context, what is on screen, the
    /// cursor's line and `threads`' lines (indexes into `to`). `None` when either diff has no
    /// lines to carry. `across`: `to` was listed under another comparison than `from`, so their
    /// old sides are different texts (module docs).
    fn carry_over(
        &mut self,
        path: &str,
        from: &FileDiff,
        to: &FileDiff,
        across: bool,
        threads: &[usize],
    ) -> Option<Vec<Segment>> {
        let (from, to) = (&from.as_text()?.lines, &to.as_text()?.lines);
        if from.is_empty() || to.is_empty() {
            return None;
        }
        let map = remap_lines(from, to);
        let mut keep = vec![false; to.len()];
        for &line in threads.iter().filter(|&&line| line < to.len()) {
            keep[line] = true;
        }
        let mut keep_line = |line: usize| {
            if let Some(&to_line) = map.get(line) {
                keep[to_line] = true;
            }
        };
        // Lines the reviewer opened: shown now, and not by the context rule.
        let usual = shown_mask(&segments(from, CONTEXT_LINES), from.len());
        if let Some(ui) = self.files.get(path) {
            let shown = shown_mask(&ui.segments, from.len());
            for line in (0..from.len()).filter(|&line| shown[line] && !usual[line]) {
                keep_line(line);
            }
        }
        for line in self.lines_in_view(path) {
            keep_line(line);
        }
        if let Some(cursor) = self.cursor.as_ref().filter(|cursor| cursor.path == path) {
            keep_line(cursor.pos.line);
        }

        // The cursor and the selection's fixed end follow their lines, on their side.
        if let Some(cursor) = self.cursor.as_mut().filter(|cursor| cursor.path == path) {
            match moved(&map, to, cursor.pos.line, cursor.pos.side) {
                Some(line) => cursor.pos = LinePos { line, ..cursor.pos },
                None => self.cursor = None,
            }
        }
        if let Some(anchor) = self.selection.as_mut().filter(|anchor| anchor.path == path) {
            match moved(&map, to, anchor.line, anchor.side) {
                Some(_) if across && anchor.side == AnchorSide::Old => self.selection = None,
                Some(line) => anchor.line = line,
                None => self.selection = None,
            }
        }
        // A new thread's composer keeps to the lines it was opened on (once its post is on its
        // way, its lines are settled); on the old side, only within one comparison.
        if let Some(composer) = self.composer.as_mut().filter(|composer| {
            composer.target.path == path
                && composer.target.reply_to.is_none()
                && !composer.posting
                && !(across && composer.target.side == AnchorSide::Old)
        }) {
            let target = &mut composer.target;
            let side = target.side;
            let number = |n: u32| {
                let line = from
                    .iter()
                    .position(|line| side_number(line, side) == Some(n))?;
                let line = moved(&map, to, line, side)?;
                side_number(&to[line], side)
            };
            if let (Some(start), Some(end)) = (number(target.start_line), number(target.end_line)) {
                target.start_line = start;
                target.end_line = end.max(start);
            }
        }
        let segments = segments_showing(to, CONTEXT_LINES, &keep);
        self.line_maps.insert(path.to_string(), map);
        Some(segments)
    }

    /// The lines of `path` (indexes into its loaded diff's lines) whose rows are on screen.
    fn lines_in_view(&self, path: &str) -> Vec<usize> {
        let Some(&block) = self.index.get(path) else {
            return Vec::new();
        };
        let (start, end) = (self.starts[block], self.starts[block + 1]);
        let height = self.viewport_height();
        let top = self.list.logical_scroll_top();
        let mut y = -f32::from(top.offset_in_item);
        let mut item = top.item_ix;
        let mut lines = Vec::new();
        while y < height && item < end {
            if item >= start {
                match self.blocks[block].rows[item - start] {
                    Row::Line(line) => lines.push(line),
                    Row::Pair { old, new } => lines.extend(old.into_iter().chain(new)),
                    _ => {}
                }
            }
            y += self.item_height(item);
            item += 1;
        }
        lines
    }
}

/// `lines` (indexes) as a mask for `segments_showing`.
fn mask(lines: &[usize]) -> Vec<bool> {
    let mut mask = vec![false; lines.iter().max().map_or(0, |&last| last + 1)];
    for &line in lines {
        mask[line] = true;
    }
    mask
}

/// Which of `len` lines `segments` show.
fn shown_mask(segments: &[Segment], len: usize) -> Vec<bool> {
    let mut shown = vec![false; len];
    for segment in segments {
        if let Segment::Lines(range) = segment {
            for line in range.clone().filter(|&line| line < len) {
                shown[line] = true;
            }
        }
    }
    shown
}

/// Where line `line` of the earlier diff went in `to` ([`remap_lines`]'s `map`), on `side`:
/// that line when it is on `side`, else the nearest one that is (the line above on a tie).
fn moved(map: &[usize], to: &[DiffLine], line: usize, side: AnchorSide) -> Option<usize> {
    let at = *map.get(line)?;
    (0..to.len())
        .filter(|&other| on_side(&to[other], side))
        .min_by_key(|&other| (other.abs_diff(at), other > at))
}
