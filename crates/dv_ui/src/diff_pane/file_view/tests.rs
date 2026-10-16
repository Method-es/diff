//! The file view's text (PLAN §18.1): a diff cut to one side keeps that side's every line, in
//! order, numbered on it alone, with its kind and intraline words.

use std::ops::Range;

use dv_core::diff::{DiffLine, LineKind, TextDiff};
use dv_core::review::{AnchorSide, side_lines};
use pretty_assertions::assert_eq;

use super::side_only;

fn line(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
    DiffLine {
        kind,
        old_no: old,
        new_no: new,
        text: text.to_string(),
        words: if kind == LineKind::Context {
            Vec::new()
        } else {
            vec![Range { start: 0, end: 1 }]
        },
    }
}

fn diff() -> TextDiff {
    TextDiff {
        lines: vec![
            line(LineKind::Context, Some(1), Some(1), "one"),
            line(LineKind::Removed, Some(2), None, "two"),
            line(LineKind::Added, None, Some(2), "2"),
            line(LineKind::Context, Some(3), Some(3), "three"),
            line(LineKind::Added, None, Some(4), "four"),
        ],
        old_eof_newline: true,
        new_eof_newline: false,
        crlf: false,
    }
}

#[test]
fn the_new_side_keeps_its_lines_and_their_kinds() {
    let text = diff();
    let new = side_only(&text, AnchorSide::New);
    assert_eq!(
        side_lines(&new, AnchorSide::New),
        side_lines(&text, AnchorSide::New)
    );
    let shape: Vec<(LineKind, Option<u32>, Option<u32>)> = new
        .lines
        .iter()
        .map(|line| (line.kind, line.old_no, line.new_no))
        .collect();
    assert_eq!(
        shape,
        [
            (LineKind::Context, None, Some(1)),
            (LineKind::Added, None, Some(2)),
            (LineKind::Context, None, Some(3)),
            (LineKind::Added, None, Some(4)),
        ]
    );
    assert_eq!(new.lines[1].words, [Range { start: 0, end: 1 }]);
    assert!(!new.new_eof_newline);
}

#[test]
fn the_old_side_of_a_deleted_file_keeps_its_removed_lines() {
    let text = diff();
    let old = side_only(&text, AnchorSide::Old);
    assert_eq!(side_lines(&old, AnchorSide::Old), ["one", "two", "three"]);
    assert!(old.lines.iter().all(|line| line.new_no.is_none()));
    assert_eq!(old.lines[1].kind, LineKind::Removed);
    assert!(old.old_eof_newline && old.new_eof_newline);
}

#[test]
fn a_read_over_the_caps_shows_as_too_large_until_loaded_anyway() {
    use dv_core::diff::{FileDiff, MAX_FILE_LINES};
    let lines = (1..=MAX_FILE_LINES as u32 + 1)
        .map(|n| line(LineKind::Context, Some(n), Some(n), "x"))
        .collect();
    let read = FileDiff::Text(TextDiff {
        lines,
        old_eof_newline: true,
        new_eof_newline: true,
        crlf: false,
    });
    assert!(matches!(
        super::read_as_shown(&read, AnchorSide::New, false),
        FileDiff::TooLarge {
            old_len: None,
            new_len: Some(_)
        }
    ));
    let FileDiff::Text(shown) = super::read_as_shown(&read, AnchorSide::New, true) else {
        panic!("loaded anyway, it is text");
    };
    assert_eq!(shown.lines.len(), MAX_FILE_LINES + 1);
    // The old side of a file the review deletes shows its lines removed.
    let small = FileDiff::Text(diff());
    let FileDiff::Text(old) = super::read_as_shown(&small, AnchorSide::Old, false) else {
        panic!("text");
    };
    assert!(old.lines.iter().all(|line| line.kind == LineKind::Removed));
}
