//! What the find bar matches (DESIGN §4.1 "Find"): plain text, case-insensitive unless the
//! query has a capital letter (smart case), over a file's text diff, in the order the stream
//! shows its lines.
//!
//! Matching is on the source text (a tab is a tab, whatever it paints as). Case folding keeps
//! every character's byte length, so a match's byte range is the same in the folded and the
//! source text: a character whose lowercase is longer or shorter (`İ`) is compared as it is.
//! Matches in a line don't overlap.
//!
//! **Order and sides.** In unified view a line is one row: a context line matches once, on the
//! new side, a removed line on the old side, an added one on the new. In split view a context
//! line shows in both columns but matches once too, on the new column (the count is the
//! review's matches, whichever view shows it); rows go top to bottom and, within a row, the old
//! column before the new, as `dv_core::diff::split_rows` pairs the lines.

use std::borrow::Cow;
use std::ops::Range;

use dv_core::diff::{DiffLine, LineKind, split_rows};
use dv_core::review::AnchorSide;

use crate::services::ViewMode;

/// A query as typed, ready to match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// The text to find, folded when the match ignores case.
    needle: String,
    insensitive: bool,
}

impl Query {
    /// `None` for an empty query, which matches nothing.
    pub fn new(text: &str) -> Option<Self> {
        if text.is_empty() {
            return None;
        }
        let insensitive = !text.chars().any(char::is_uppercase);
        Some(Self {
            needle: if insensitive {
                fold(text).into_owned()
            } else {
                text.to_string()
            },
            insensitive,
        })
    }

    pub fn is_case_sensitive(&self) -> bool {
        !self.insensitive
    }
}

/// One match: the row it shows in (its place among the file's rows in the stream's order, the
/// rows gaps hide included; [`row_of`]), a line (an index into the file's lines), the side it
/// shows on and its byte range in the line's text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Hit {
    pub row: usize,
    pub line: usize,
    pub side: AnchorSide,
    pub range: Range<usize>,
}

/// `text` in lowercase, each character's byte length kept (see the module docs).
pub fn fold(text: &str) -> Cow<'_, str> {
    if text.is_ascii() {
        return if text.bytes().any(|b| b.is_ascii_uppercase()) {
            Cow::Owned(text.to_ascii_lowercase())
        } else {
            Cow::Borrowed(text)
        };
    }
    if !text.chars().any(char::is_uppercase) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        let mut lower = ch.to_lowercase();
        match (lower.next(), lower.next()) {
            (Some(single), None) if single.len_utf8() == ch.len_utf8() => out.push(single),
            _ => out.push(ch),
        }
    }
    Cow::Owned(out)
}

/// Where `query` matches in `text`: byte ranges, in order, apart.
pub fn line_hits(text: &str, query: &Query) -> Vec<Range<usize>> {
    let mut hits = Vec::new();
    each_hit(text, query, &mut String::new(), |range| hits.push(range));
    hits
}

/// Calls `found` with each match of `query` in `text`, in order; `folded` is scratch room for
/// the folded text, reused from line to line.
fn each_hit(text: &str, query: &Query, folded: &mut String, mut found: impl FnMut(Range<usize>)) {
    let haystack = if query.insensitive {
        folded.clear();
        fold_into(text, folded);
        folded.as_str()
    } else {
        text
    };
    for (at, hit) in haystack.match_indices(query.needle.as_str()) {
        found(at..at + hit.len());
    }
}

/// Appends `text` folded ([`fold`]) to `out`.
fn fold_into(text: &str, out: &mut String) {
    if text.is_ascii() {
        out.push_str(text);
        out.make_ascii_lowercase();
        return;
    }
    out.push_str(&fold(text));
}

/// Every match of `query` in `lines` (a file's text diff), in the order the stream shows them
/// in `mode` (module docs). Each line is matched once; in split view a context line's matches
/// are its new column's.
pub fn file_hits(lines: &[DiffLine], query: &Query, mode: ViewMode) -> Vec<Hit> {
    let mut found: Vec<(usize, Range<usize>)> = Vec::new();
    let mut starts = Vec::with_capacity(lines.len() + 1);
    let mut folded = String::new();
    for (line, diff_line) in lines.iter().enumerate() {
        starts.push(found.len());
        each_hit(&diff_line.text, query, &mut folded, |range| {
            found.push((line, range))
        });
    }
    starts.push(found.len());
    let on = |row: usize, line: usize, side: AnchorSide| {
        found[starts[line]..starts[line + 1]]
            .iter()
            .map(move |(_, range)| Hit {
                row,
                line,
                side,
                range: range.clone(),
            })
    };
    let mut hits = Vec::with_capacity(found.len());
    match mode {
        ViewMode::Unified => {
            for (line, diff_line) in lines.iter().enumerate() {
                if starts[line] < starts[line + 1] {
                    hits.extend(on(line, line, unified_side(diff_line)));
                }
            }
        }
        ViewMode::Split if !found.is_empty() => {
            for (row, pair) in split_rows(lines, 0..lines.len()).into_iter().enumerate() {
                // A context line is in both columns: its matches are the new column's.
                if let Some(old) = pair.old.filter(|old| pair.new != Some(*old)) {
                    hits.extend(on(row, old, AnchorSide::Old));
                }
                if let Some(new) = pair.new {
                    hits.extend(on(row, new, AnchorSide::New));
                }
            }
        }
        ViewMode::Split => {}
    }
    hits
}

/// The side a unified row shows `line` on: a removed line's old, any other's new.
fn unified_side(line: &DiffLine) -> AnchorSide {
    match line.kind {
        LineKind::Removed => AnchorSide::Old,
        LineKind::Added | LineKind::Context => AnchorSide::New,
    }
}

/// The place in [`file_hits`]' order of the row that shows `line` (an index into `lines`) on
/// `side` in `mode`: what "at or after the cursor" compares; `None` for a line that isn't on
/// that side.
pub fn row_of(lines: &[DiffLine], mode: ViewMode, line: usize, side: AnchorSide) -> Option<usize> {
    match mode {
        ViewMode::Unified => (line < lines.len()).then_some(line),
        ViewMode::Split => split_rows(lines, 0..lines.len())
            .iter()
            .position(|pair| match side {
                AnchorSide::Old => pair.old == Some(line),
                AnchorSide::New => pair.new == Some(line),
            }),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn span(start: usize, end: usize) -> Range<usize> {
        Range { start, end }
    }

    fn query(text: &str) -> Query {
        Query::new(text).expect("a query")
    }

    fn line(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old_no: old,
            new_no: new,
            text: text.to_string(),
            words: Vec::new(),
        }
    }

    #[test]
    fn smart_case_ignores_case_until_the_query_has_a_capital() {
        assert_eq!(line_hits("Foo foo FOO", &query("foo")), [0..3, 4..7, 8..11]);
        assert_eq!(line_hits("Foo foo FOO", &query("Foo")), [span(0, 3)]);
        assert!(!query("foo").is_case_sensitive());
        assert!(query("fOo").is_case_sensitive());
        assert_eq!(Query::new(""), None);
    }

    #[test]
    fn folding_keeps_byte_offsets_for_any_text() {
        // `É` and `é` are both two bytes; `İ` lowercases to two characters and stays as is.
        let text = "Élan élan İx";
        assert_eq!(fold(text).len(), text.len());
        assert_eq!(line_hits(text, &query("élan")), [0..5, 6..11]);
        assert_eq!(line_hits(text, &query("x")), [span(14, 15)]);
        // Plain text: a regex character is itself, a tab a tab.
        assert_eq!(line_hits("a.b\ta*b", &query("a*")), [span(4, 6)]);
        assert_eq!(line_hits("\tx", &query("\t")), [span(0, 1)]);
        // Matches don't overlap.
        assert_eq!(line_hits("aaaa", &query("aa")), [0..2, 2..4]);
    }

    #[test]
    fn a_context_line_matches_once_in_either_view() {
        let lines = vec![
            line(LineKind::Context, Some(1), Some(1), "let x"),
            line(LineKind::Removed, Some(2), None, "let a"),
            line(LineKind::Removed, Some(3), None, "let b"),
            line(LineKind::Added, None, Some(2), "let c"),
        ];
        let unified = file_hits(&lines, &query("let"), ViewMode::Unified);
        let order = |hits: &[Hit]| -> Vec<(usize, AnchorSide)> {
            hits.iter().map(|hit| (hit.line, hit.side)).collect()
        };
        assert_eq!(
            order(&unified),
            [
                (0, AnchorSide::New),
                (1, AnchorSide::Old),
                (2, AnchorSide::Old),
                (3, AnchorSide::New),
            ]
        );
        // Split view: the context line on its new column only; then `let a` paired with
        // `let c` on one row, old column first, then the next row's `let b`.
        let split = file_hits(&lines, &query("let"), ViewMode::Split);
        assert_eq!(
            order(&split),
            [
                (0, AnchorSide::New),
                (1, AnchorSide::Old),
                (3, AnchorSide::New),
                (2, AnchorSide::Old),
            ]
        );
        assert_eq!(split.len(), unified.len());
        // Each hit knows its row; the cursor's row is found the same way.
        let rows: Vec<usize> = split.iter().map(|hit| hit.row).collect();
        assert_eq!(rows, [0, 1, 1, 2]);
        assert_eq!(row_of(&lines, ViewMode::Split, 3, AnchorSide::New), Some(1));
        assert_eq!(row_of(&lines, ViewMode::Split, 2, AnchorSide::Old), Some(2));
        assert_eq!(row_of(&lines, ViewMode::Split, 3, AnchorSide::Old), None);
        assert_eq!(
            row_of(&lines, ViewMode::Unified, 3, AnchorSide::New),
            Some(3)
        );
    }
}
