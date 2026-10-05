//! Anchors from a selection (PLAN §2.5): the anchored text and up to three lines of context
//! either side.

use crate::diff::{DiffLine, LineKind, TextDiff};
use crate::review::anchor::{AnchorError, AnchorPlace, anchor_selection, side_lines};

use super::*;

fn place(side: AnchorSide) -> AnchorPlace {
    AnchorPlace {
        round: 2,
        path: "src/new.rs".to_string(),
        old_path: Some("src/old.rs".to_string()),
        side,
    }
}

/// `count` lines `l1`..`l<count>`.
fn text(count: usize) -> Vec<String> {
    (1..=count).map(|n| format!("l{n}")).collect()
}

#[test]
fn a_selection_keeps_its_text_and_three_lines_of_context_either_side() {
    type Lines = &'static [&'static str];
    let lines = text(10);
    // (from, to, text, before, after)
    let cases: &[(u32, u32, Lines, Lines, Lines)] = &[
        (
            5,
            6,
            &["l5", "l6"],
            &["l2", "l3", "l4"],
            &["l7", "l8", "l9"],
        ),
        (5, 5, &["l5"], &["l2", "l3", "l4"], &["l6", "l7", "l8"]),
        // Extended upwards: the same range.
        (
            6,
            5,
            &["l5", "l6"],
            &["l2", "l3", "l4"],
            &["l7", "l8", "l9"],
        ),
        // Near the edges, the context is what there is.
        (1, 1, &["l1"], &[], &["l2", "l3", "l4"]),
        (2, 3, &["l2", "l3"], &["l1"], &["l4", "l5", "l6"]),
        (9, 10, &["l9", "l10"], &["l6", "l7", "l8"], &[]),
        (8, 8, &["l8"], &["l5", "l6", "l7"], &["l9", "l10"]),
        (
            1,
            10,
            &["l1", "l2", "l3", "l4", "l5", "l6", "l7", "l8", "l9", "l10"],
            &[],
            &[],
        ),
    ];
    for &(from, to, anchored, before, after) in cases {
        let anchor = anchor_selection(place(AnchorSide::New), &lines, from, to).unwrap();
        let (start, end) = (from.min(to), from.max(to));
        assert_eq!(
            anchor,
            Anchor {
                round: 2,
                path: "src/new.rs".to_string(),
                old_path: Some("src/old.rs".to_string()),
                side: AnchorSide::New,
                start_line: start,
                end_line: end,
                text: anchored.iter().map(|s| s.to_string()).collect(),
                context_before: before.iter().map(|s| s.to_string()).collect(),
                context_after: after.iter().map(|s| s.to_string()).collect(),
            },
            "{from}..{to}"
        );
        // What the fold checks an anchor against (PLAN §7).
        assert_eq!(crate::review::fold::check_anchor(&anchor, 2), Ok(()));
        assert!(anchor.matches(&lines));
    }
}

#[test]
fn a_selection_outside_the_side_is_refused() {
    let lines = text(3);
    assert_eq!(
        anchor_selection(place(AnchorSide::Old), &lines, 0, 2),
        Err(AnchorError::ZeroLine)
    );
    assert_eq!(
        anchor_selection(place(AnchorSide::Old), &lines, 3, 4),
        Err(AnchorError::PastEnd {
            line: 4,
            len: 3,
            side: AnchorSide::Old
        })
    );
    let empty: [&str; 0] = [];
    assert!(anchor_selection(place(AnchorSide::New), &empty, 1, 1).is_err());
}

#[test]
fn side_lines_are_one_sides_full_text_in_order() {
    let line = |kind, old_no, new_no, text: &str| DiffLine {
        kind,
        old_no,
        new_no,
        text: text.to_string(),
        words: Vec::new(),
    };
    let diff = TextDiff {
        lines: vec![
            line(LineKind::Context, Some(1), Some(1), "same"),
            line(LineKind::Removed, Some(2), None, "old two"),
            line(LineKind::Removed, Some(3), None, "old three"),
            line(LineKind::Added, None, Some(2), "new two"),
            line(LineKind::Context, Some(4), Some(3), "end"),
        ],
        ..TextDiff::empty()
    };
    assert_eq!(
        side_lines(&diff, AnchorSide::Old),
        ["same", "old two", "old three", "end"]
    );
    assert_eq!(
        side_lines(&diff, AnchorSide::New),
        ["same", "new two", "end"]
    );
    let lines = side_lines(&diff, AnchorSide::Old);
    let anchor = anchor_selection(place(AnchorSide::Old), &lines, 2, 3).unwrap();
    assert_eq!(anchor.text, ["old two", "old three"]);
    assert_eq!(anchor.context_before, ["same"]);
    assert_eq!(anchor.context_after, ["end"]);
}

#[test]
fn an_anchor_matches_only_its_text_at_its_recorded_lines() {
    let lines = text(10);
    let anchor = anchor_selection(place(AnchorSide::New), &lines, 4, 5).unwrap();
    assert!(anchor.matches(&lines));
    // A line inserted above moves the text off its recorded lines.
    let mut moved = lines.clone();
    moved.insert(0, "new first line".to_string());
    assert!(!anchor.matches(&moved));
    // An edited anchored line.
    let mut edited = lines.clone();
    edited[4] = "l5 edited".to_string();
    assert!(!anchor.matches(&edited));
    // Context changes don't matter; a shorter file does.
    let mut context = lines.clone();
    context[0] = "changed context".to_string();
    assert!(anchor.matches(&context));
    assert!(!anchor.matches(&lines[..4]));
    let broken = Anchor {
        start_line: 0,
        ..anchor.clone()
    };
    assert!(!broken.matches(&lines));
}
