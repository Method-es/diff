//! Pure helpers the diff stream's rows are built from (docs/PLAN.md §6 steps 5 and 7, DESIGN
//! §4.1): the pairing of a split view's two columns, and the `@@ -a,b +c,d @@` range a gap row
//! shows for the lines that follow it.

use std::fmt;
use std::ops::Range;

use serde::{Deserialize, Serialize};

use super::{DiffLine, LineKind};

/// One row of the split view: what the old (left) and the new (right) column show, as indexes
/// into the diff's lines. An unchanged line fills both columns; within a run of changed lines
/// the i-th removed line pairs with the i-th added one (as on GitHub, and as intraline marks
/// pair them, [`super::mark_intraline`]), and the longer side's extra lines face an empty cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitRow {
    pub old: Option<usize>,
    pub new: Option<usize>,
}

/// The split view's rows for `lines[range]` (a shown [`super::Segment::Lines`] run), in order.
/// A run of changed lines is paired within `range`; segments never cut one, because only
/// unchanged lines collapse into gaps.
pub fn split_rows(lines: &[DiffLine], range: Range<usize>) -> Vec<SplitRow> {
    let mut rows = Vec::with_capacity(range.len());
    let mut i = range.start;
    while i < range.end {
        if !lines[i].kind.is_change() {
            rows.push(SplitRow {
                old: Some(i),
                new: Some(i),
            });
            i += 1;
            continue;
        }
        let end = (i..range.end)
            .find(|&j| !lines[j].kind.is_change())
            .unwrap_or(range.end);
        let removed: Vec<usize> = (i..end)
            .filter(|&j| lines[j].kind == LineKind::Removed)
            .collect();
        let added: Vec<usize> = (i..end)
            .filter(|&j| lines[j].kind == LineKind::Added)
            .collect();
        for k in 0..removed.len().max(added.len()) {
            rows.push(SplitRow {
                old: removed.get(k).copied(),
                new: added.get(k).copied(),
            });
        }
        i = end;
    }
    rows
}

/// A hunk's line ranges as git's `@@ -a,b +c,d @@` header states them: where the shown lines
/// start on each side and how many there are. A side with no lines in the hunk starts at the
/// line before it (`0` at the top of the file), as in git.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HunkRange {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
}

impl fmt::Display for HunkRange {
    /// `@@ -12,7 +12,9 @@`; a count of 1 is left out (`-5 +5,2`), as git leaves it out.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let side = |start: u32, count: u32| {
            if count == 1 {
                start.to_string()
            } else {
                format!("{start},{count}")
            }
        };
        write!(
            f,
            "@@ -{} +{} @@",
            side(self.old_start, self.old_count),
            side(self.new_start, self.new_count)
        )
    }
}

/// The [`HunkRange`] of `lines[range]`: the shown run a gap row introduces (DESIGN §4.1, "the
/// gap row is the hunk separator"). Lines carry their own numbers, so a side's start is its
/// first line's number in the range, or, when the range has none on that side, the number of
/// the last line before it on that side.
pub fn hunk_range(lines: &[DiffLine], range: Range<usize>) -> HunkRange {
    let side = |number: fn(&DiffLine) -> Option<u32>| {
        let shown = &lines[range.clone()];
        let count = shown.iter().filter(|line| number(line).is_some()).count();
        let start = shown.iter().find_map(number).unwrap_or_else(|| {
            lines[..range.start]
                .iter()
                .rev()
                .find_map(number)
                .unwrap_or(0)
        });
        (start, u32::try_from(count).unwrap_or(u32::MAX))
    };
    let (old_start, old_count) = side(|line| line.old_no);
    let (new_start, new_count) = side(|line| line.new_no);
    HunkRange {
        old_start,
        old_count,
        new_start,
        new_count,
    }
}

#[cfg(test)]
mod tests;
