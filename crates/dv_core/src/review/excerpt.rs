//! A thread's excerpt (docs/PLAN.md §10.3): the rows of the review's diff around the lines a
//! thread stands on now, as the CLI prints them under the thread's header and as `excerpt` in
//! its JSON.
//!
//! - The window is [`ANCHOR_CONTEXT`] lines of the thread's side before and after its lines, and
//!   every row of the other side between them (lines the change removed, for a thread on the new
//!   side; lines it added, for one on the old side).
//! - A run of more than [`RUN_MAX`] other-side rows keeps its first and last [`RUN_KEEP`]; a
//!   commented span of more than [`RANGE_MAX`] lines of the thread's side keeps its first and
//!   last [`RANGE_KEEP`] of them, with every other-side row among those kept ([`hidden_span`]).
//! - A scope row leads: the nearest line of the thread's side above the window that starts with
//!   an ASCII letter, `_` or `$` (git's default hunk-header rule, language-blind), unless such a
//!   line already stands in the window above the commented lines.
//!
//! Every number printed is the line's number on the review's new side now, blank for a removed
//! line, so an agent acts on lines as the file is now whichever side the thread is on.

use super::anchor::ANCHOR_CONTEXT;
use super::model::AnchorSide;
use crate::diff::{DiffLine, LineKind, TextDiff};

/// A run of the other side's rows longer than this keeps only its ends ([`RUN_KEEP`] each).
pub const RUN_MAX: usize = 8;

/// The rows a long run of the other side's rows keeps at each end.
pub const RUN_KEEP: usize = 3;

/// A commented span of more lines than this keeps only its ends ([`RANGE_KEEP`] each).
pub const RANGE_MAX: usize = 24;

/// The lines a long commented span keeps at each end.
pub const RANGE_KEEP: usize = 10;

/// The characters of a line an excerpt prints; a longer line is cut, ending in `…`.
pub const LINE_MAX: usize = 240;

/// One row of an excerpt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExcerptRow {
    /// The diff's line at `index` ([`TextDiff::lines`]); `commented` when the thread is on it.
    Line { index: usize, commented: bool },
    /// Rows left out.
    Gap(ExcerptGap),
}

/// What an [`ExcerptRow::Gap`] leaves out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExcerptGap {
    /// The lines between the scope row and the window.
    Scope,
    /// The middle of a long run of the other side's rows: `count` lines of `kind` (removed or
    /// added).
    Run { count: usize, kind: LineKind },
    /// The middle of a long commented span: `lines` of the thread's side and `other` rows of
    /// the other side among them, of `kind` (removed or added).
    Range {
        lines: usize,
        other: usize,
        kind: LineKind,
    },
}

/// The rows of `text` around lines `start..=end` (1-based, inclusive) of `side` (module docs);
/// `None` when `side` has none of those lines.
pub fn excerpt(text: &TextDiff, side: AnchorSide, start: u32, end: u32) -> Option<Vec<ExcerptRow>> {
    let lines = &text.lines;
    let number = |index: usize| match side {
        AnchorSide::New => lines[index].new_no,
        AnchorSide::Old => lines[index].old_no,
    };
    let on_side = |index: usize| number(index).is_some();
    let commented_line = |index: usize| number(index).is_some_and(|no| (start..=end).contains(&no));
    let first = (0..lines.len()).find(|&index| commented_line(index))?;
    let last = (0..lines.len())
        .rev()
        .find(|&index| commented_line(index))?;

    // The window: ANCHOR_CONTEXT lines of the side either way, the other side's rows between.
    let (mut low, mut seen) = (first, 0);
    while low > 0 && seen < ANCHOR_CONTEXT {
        low -= 1;
        seen += usize::from(on_side(low));
    }
    let (mut high, mut seen) = (last, 0);
    while high + 1 < lines.len() && seen < ANCHOR_CONTEXT {
        high += 1;
        seen += usize::from(on_side(high));
    }

    let mut rows = Vec::new();
    let scope_like = |index: usize| on_side(index) && starts_scope(&lines[index].text);
    if !(low..first).any(scope_like)
        && let Some(scope) = (0..low).rev().find(|&index| scope_like(index))
    {
        rows.push(ExcerptRow::Line {
            index: scope,
            commented: false,
        });
        if scope + 1 < low {
            rows.push(ExcerptRow::Gap(ExcerptGap::Scope));
        }
    }

    // A long commented span keeps its first and last side lines, and the other side's rows
    // among those; `dropped` is the rows from just past the last line kept at the top to just
    // before the first line kept at the bottom.
    let commented: Vec<usize> = (first..=last).filter(|&index| on_side(index)).collect();
    let dropped = hidden_span(commented.len())
        .map(|hidden| commented[hidden.start - 1] + 1..=commented[hidden.end] - 1);
    let is_dropped = |index: usize| dropped.as_ref().is_some_and(|range| range.contains(&index));
    let mut index = low;
    while index <= high {
        if let Some(range) = dropped.as_ref().filter(|range| range.contains(&index)) {
            let lines = range.clone().filter(|&at| on_side(at)).count();
            rows.push(ExcerptRow::Gap(ExcerptGap::Range {
                lines,
                other: range.clone().count() - lines,
                kind: match side {
                    AnchorSide::New => LineKind::Removed,
                    AnchorSide::Old => LineKind::Added,
                },
            }));
            index = range.end() + 1;
            continue;
        }
        if !on_side(index) {
            let run_end = (index..=high)
                .find(|&at| on_side(at) || is_dropped(at))
                .unwrap_or(high + 1);
            let run = index..run_end;
            let line = |at: usize| ExcerptRow::Line {
                index: at,
                commented: false,
            };
            if run.len() > RUN_MAX {
                rows.extend((index..index + RUN_KEEP).map(line));
                rows.push(ExcerptRow::Gap(ExcerptGap::Run {
                    count: run.len() - 2 * RUN_KEEP,
                    kind: lines[index].kind,
                }));
                rows.extend((run_end - RUN_KEEP..run_end).map(line));
            } else {
                rows.extend(run.map(line));
            }
            index = run_end;
            continue;
        }
        rows.push(ExcerptRow::Line {
            index,
            commented: (first..=last).contains(&index),
        });
        index += 1;
    }
    Some(rows)
}

/// `rows` of `text` as printed, one line each (PLAN §10.3): `>` on a commented line or a space,
/// the line's number on the new side right-aligned to the widest printed (blank on a removed
/// line), a space, `+`, `-` or a space, ` |`, then a space and the line, cut at [`LINE_MAX`]
/// characters, and no row ends in whitespace (a line of only whitespace ends at `|` too). A gap
/// is `⋮` under the numbers, then what it leaves out: `N more removed lines` (or `added`) in a
/// run; `N lines` in a commented span, `N lines, M removed` (or `added`) when it hides rows of
/// the other side too; nothing after the scope row.
pub fn excerpt_lines(text: &TextDiff, rows: &[ExcerptRow]) -> Vec<String> {
    let width = rows
        .iter()
        .filter_map(|row| match row {
            ExcerptRow::Line { index, .. } => text.lines[*index].new_no,
            ExcerptRow::Gap(_) => None,
        })
        .map(|no| no.to_string().len())
        .max()
        .unwrap_or(1);
    rows.iter()
        .map(|row| match row {
            ExcerptRow::Line { index, commented } => {
                line_row(&text.lines[*index], *commented, width)
            }
            ExcerptRow::Gap(gap) => {
                let what = match gap {
                    ExcerptGap::Scope => String::new(),
                    ExcerptGap::Run { count, kind } => {
                        format!(" {count} more {} lines", kind_word(*kind))
                    }
                    ExcerptGap::Range {
                        lines, other: 0, ..
                    } => format!(" {lines} lines"),
                    ExcerptGap::Range { lines, other, kind } => {
                        format!(" {lines} lines, {other} {}", kind_word(*kind))
                    }
                };
                format!(" {:>width$}{what}", "⋮")
            }
        })
        .collect()
}

/// One code row of [`excerpt_lines`].
fn line_row(line: &DiffLine, commented: bool, width: usize) -> String {
    let mark = if commented { '>' } else { ' ' };
    let number = line.new_no.map(|no| no.to_string()).unwrap_or_default();
    let kind = match line.kind {
        LineKind::Added => '+',
        LineKind::Removed => '-',
        LineKind::Context => ' ',
    };
    code_row(&format!("{mark}{number:>width$} {kind} |"), &line.text)
}

/// `head` (a row up to its `|`), then a space and `line` cut at [`LINE_MAX`] characters, with no
/// whitespace at the end: a line of only whitespace ends at `|`.
pub fn code_row(head: &str, line: &str) -> String {
    let line = cut(line);
    let line = line.trim_end();
    if line.is_empty() {
        head.to_string()
    } else {
        format!("{head} {line}")
    }
}

/// The middle a span of `len` lines leaves out (0-based indices into it): over [`RANGE_MAX`]
/// lines it keeps its first and last [`RANGE_KEEP`]; `None` when it keeps them all. The
/// excerpt's commented span and the CLI's unnumbered lines as commented share this rule.
pub fn hidden_span(len: usize) -> Option<std::ops::Range<usize>> {
    (len > RANGE_MAX).then(|| RANGE_KEEP..len - RANGE_KEEP)
}

/// `removed` or `added`.
fn kind_word(kind: LineKind) -> &'static str {
    if kind == LineKind::Added {
        "added"
    } else {
        "removed"
    }
}

/// `text` cut at [`LINE_MAX`] characters, the last of them `…` when anything was cut.
pub fn cut(text: &str) -> std::borrow::Cow<'_, str> {
    if text.chars().count() <= LINE_MAX {
        return text.into();
    }
    let mut kept: String = text.chars().take(LINE_MAX - 1).collect();
    kept.push('…');
    kept.into()
}

/// Whether `line` starts a scope by git's default hunk-header rule: its first character is an
/// ASCII letter, `_` or `$`.
fn starts_scope(line: &str) -> bool {
    line.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
}
