//! Collapsing unchanged runs into gaps, and expanding gaps again (docs/PLAN.md §6 step 5,
//! DESIGN §4.1: "··· 27 unchanged lines — Expand 20 · Expand all").
//!
//! Pure functions over a [`TextDiff`](super::TextDiff)'s lines, which already hold both sides'
//! complete text (PLAN D4): expanding a gap never goes back to git.

use std::ops::Range;

use serde::{Deserialize, Serialize};

use super::DiffLine;

/// Lines one "Expand" step reveals (PLAN §6 step 5).
pub const EXPAND_STEP: usize = 20;

/// The shortest run of unchanged lines that collapses into a gap: a gap row standing in for a
/// single line would take the same space as the line.
pub const MIN_GAP: usize = 2;

/// A run of rows in a file's diff: lines shown, or a gap standing in for hidden unchanged lines.
/// Ranges index [`TextDiff::lines`](super::TextDiff::lines).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Segment {
    Lines(Range<usize>),
    Gap {
        lines: Range<usize>,
        /// Shown lines precede the gap, so it can reveal lines from its top
        /// ([`Expand::FromAbove`]).
        expandable_above: bool,
        /// Shown lines follow the gap, so it can reveal lines from its bottom
        /// ([`Expand::FromBelow`]).
        expandable_below: bool,
    },
}

impl Segment {
    /// The lines this segment covers, shown or hidden.
    pub fn range(&self) -> Range<usize> {
        match self {
            Segment::Lines(range) | Segment::Gap { lines: range, .. } => range.clone(),
        }
    }

    pub fn is_gap(&self) -> bool {
        matches!(self, Segment::Gap { .. })
    }
}

/// Which lines of a gap to reveal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expand {
    /// The [`EXPAND_STEP`] lines at the gap's top, continuing the lines above it.
    FromAbove,
    /// The [`EXPAND_STEP`] lines at the gap's bottom, leading into the lines below it.
    FromBelow,
    /// The whole gap.
    All,
}

/// The rows for `lines` with `context` unchanged lines kept around every change; longer
/// unchanged runs become gaps. A diff without changes (no lines at all, or only context) is
/// one gap, or nothing when it has no lines.
pub fn segments(lines: &[DiffLine], context: usize) -> Vec<Segment> {
    segments_showing(lines, context, &[])
}

/// [`segments`], with every line `also` marks (by index into `lines`) shown too, whatever its
/// distance from a change: a file that loads again keeps the context the reviewer had open
/// ([`remap_lines`](super::remap_lines) finds those lines in the new diff). Lines past the end
/// of `also` follow the context rule alone.
pub fn segments_showing(lines: &[DiffLine], context: usize, also: &[bool]) -> Vec<Segment> {
    // Distance from each line to the nearest change, in lines; every line between a change and
    // a context line is itself context, so this counts context lines.
    let mut distance = vec![usize::MAX; lines.len()];
    let mut last_change = None;
    for (i, line) in lines.iter().enumerate() {
        if line.kind.is_change() {
            last_change = Some(i);
        }
        if let Some(change) = last_change {
            distance[i] = i - change;
        }
    }
    let mut next_change = None;
    for (i, line) in lines.iter().enumerate().rev() {
        if line.kind.is_change() {
            next_change = Some(i);
        }
        if let Some(change) = next_change {
            distance[i] = distance[i].min(change - i);
        }
    }

    let visible = |i: usize| distance[i] <= context || also.get(i).copied().unwrap_or(false);
    let mut out = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let shown = visible(start);
        let end = (start..lines.len())
            .find(|&i| visible(i) != shown)
            .unwrap_or(lines.len());
        if shown || end - start < MIN_GAP {
            out.push(Segment::Lines(start..end));
        } else {
            out.push(gap(start..end));
        }
        start = end;
    }
    normalize(out)
}

/// `segments` with gap number `index` (an index into `segments`, which must name a gap) partly or
/// wholly revealed. What a step would leave of a gap shorter than [`MIN_GAP`] is revealed too.
/// Any other `index` returns `segments` unchanged.
pub fn expand_gap(segments: &[Segment], index: usize, expand: Expand) -> Vec<Segment> {
    let Some(Segment::Gap { lines: hidden, .. }) = segments.get(index) else {
        return segments.to_vec();
    };
    let step = EXPAND_STEP.min(hidden.len());
    let (mut shown, mut rest) = match expand {
        Expand::FromAbove => (
            hidden.start..hidden.start + step,
            hidden.start + step..hidden.end,
        ),
        Expand::FromBelow => (
            hidden.end - step..hidden.end,
            hidden.start..hidden.end - step,
        ),
        Expand::All => (hidden.clone(), hidden.end..hidden.end),
    };
    if rest.len() < MIN_GAP {
        shown = hidden.clone();
        rest = hidden.end..hidden.end;
    }
    let mut out: Vec<Segment> = segments[..index].to_vec();
    match expand {
        Expand::FromBelow if !rest.is_empty() => {
            out.push(gap(rest));
            out.push(Segment::Lines(shown));
        }
        _ => {
            out.push(Segment::Lines(shown));
            if !rest.is_empty() {
                out.push(gap(rest));
            }
        }
    }
    out.extend_from_slice(&segments[index + 1..]);
    normalize(out)
}

/// `segments` with the lines of gap number `index` that `around` covers revealed, the rest of the
/// gap left hidden on either side: what going to a line inside a gap opens (the stream's find,
/// DESIGN §4.1). A piece left shorter than [`MIN_GAP`] is revealed too. An `index` that names no
/// gap, or an `around` that misses it, returns `segments` unchanged.
pub fn reveal_in_gap(segments: &[Segment], index: usize, around: Range<usize>) -> Vec<Segment> {
    let Some(Segment::Gap { lines: hidden, .. }) = segments.get(index) else {
        return segments.to_vec();
    };
    let mut shown = around.start.max(hidden.start)..around.end.min(hidden.end);
    if shown.is_empty() {
        return segments.to_vec();
    }
    if shown.start - hidden.start < MIN_GAP {
        shown.start = hidden.start;
    }
    if hidden.end - shown.end < MIN_GAP {
        shown.end = hidden.end;
    }
    let mut out: Vec<Segment> = segments[..index].to_vec();
    if shown.start > hidden.start {
        out.push(gap(hidden.start..shown.start));
    }
    let end = shown.end;
    out.push(Segment::Lines(shown));
    if end < hidden.end {
        out.push(gap(end..hidden.end));
    }
    out.extend_from_slice(&segments[index + 1..]);
    normalize(out)
}

fn gap(lines: Range<usize>) -> Segment {
    Segment::Gap {
        lines,
        expandable_above: false,
        expandable_below: false,
    }
}

/// Merges adjacent shown runs, drops empty segments, and sets each gap's expandable sides from
/// its neighbours.
fn normalize(segments: Vec<Segment>) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::with_capacity(segments.len());
    for segment in segments {
        if segment.range().is_empty() {
            continue;
        }
        if let (Some(Segment::Lines(last)), Segment::Lines(next)) = (out.last_mut(), &segment)
            && last.end == next.start
        {
            last.end = next.end;
            continue;
        }
        out.push(segment);
    }
    let count = out.len();
    for i in 0..count {
        let above = i > 0 && !out[i - 1].is_gap();
        let below = i + 1 < count && !out[i + 1].is_gap();
        if let Segment::Gap {
            expandable_above,
            expandable_below,
            ..
        } = &mut out[i]
        {
            *expandable_above = above;
            *expandable_below = below;
        }
    }
    out
}

#[cfg(test)]
mod tests;
