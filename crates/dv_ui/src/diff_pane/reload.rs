//! A file's diff loading again (PLAN §11 step 7, DESIGN §4.4): the agent saved it, staged it or
//! committed part of it while the reviewer reads. The same lines again keep everything as it
//! was. Other lines are a new diff, and what the stream kept by line index (the context the
//! reviewer opened, the lines on screen, the cursor, the selection, the composer's lines) is
//! carried onto it through [`remap_lines`], so an edit elsewhere in the file moves nothing the
//! reviewer is looking at.

use std::sync::Arc;

use dv_core::diff::{DiffLine, FileDiff, Segment, remap_lines, segments, segments_showing};
use dv_core::review::AnchorSide;

use super::rows::{CONTEXT_LINES, LinePos, Row, on_side, side_number};
use super::text::columns;
use super::{DiffPane, same_lines};

impl DiffPane {
    /// Takes `diff` as `path`'s loaded diff, when it isn't already (see the module docs).
    pub(super) fn take_diff(&mut self, path: &str, diff: &Arc<FileDiff>) {
        let kept = self.files.get(path).and_then(|ui| ui.diff.clone());
        if kept.as_ref().is_some_and(|kept| Arc::ptr_eq(kept, diff)) {
            return;
        }
        // The same lines again (staged, rescanned): the gaps stay as expanded.
        if !kept.as_ref().is_some_and(|kept| same_lines(kept, diff)) {
            let carried = kept.and_then(|kept| self.carry_over(path, &kept, diff));
            let ui = self.files.entry(path.to_string()).or_default();
            ui.segments = carried.unwrap_or_else(|| diff.segments(CONTEXT_LINES));
            ui.columns = diff
                .as_text()
                .and_then(|text| text.lines.iter().map(|line| columns(&line.text)).max())
                .unwrap_or(0);
        }
        self.files.entry(path.to_string()).or_default().diff = Some(diff.clone());
    }

    /// Carries what the stream holds of `path` from `from`'s lines onto `to`'s: the cursor, the
    /// selection and the composer move with their lines, and the new segments, returned, keep
    /// shown what the reviewer opened beyond the usual context, what is on screen and the
    /// cursor's line. `None` when either diff has no lines to carry.
    fn carry_over(&mut self, path: &str, from: &FileDiff, to: &FileDiff) -> Option<Vec<Segment>> {
        let (from, to) = (&from.as_text()?.lines, &to.as_text()?.lines);
        if from.is_empty() || to.is_empty() {
            return None;
        }
        let map = remap_lines(from, to);
        let mut keep = vec![false; to.len()];
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
                Some(line) => anchor.line = line,
                None => self.selection = None,
            }
        }
        // A new thread's composer keeps to the lines it was opened on (once its post is on its
        // way, its lines are settled).
        if let Some(composer) = self.composer.as_mut().filter(|composer| {
            composer.target.path == path && composer.target.reply_to.is_none() && !composer.posting
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
