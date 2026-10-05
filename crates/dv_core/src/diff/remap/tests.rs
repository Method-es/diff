use super::*;
use crate::diff::LineKind;

/// A diff's lines from `(kind, text)` pairs (`' '` unchanged, `-` removed, `+` added), numbered
/// on each side in order.
fn diff(lines: &[(char, &str)]) -> Vec<DiffLine> {
    let (mut old, mut new) = (0, 0);
    lines
        .iter()
        .map(|&(kind, text)| {
            let kind = match kind {
                ' ' => LineKind::Context,
                '-' => LineKind::Removed,
                '+' => LineKind::Added,
                other => panic!("bad kind {other:?}"),
            };
            let old_no = (kind != LineKind::Added).then(|| {
                old += 1;
                old
            });
            let new_no = (kind != LineKind::Removed).then(|| {
                new += 1;
                new
            });
            DiffLine {
                kind,
                old_no,
                new_no,
                text: text.to_string(),
                words: Vec::new(),
            }
        })
        .collect()
}

const READ: &[(char, &str)] = &[
    (' ', "a"),
    (' ', "b"),
    ('-', "c"),
    ('+', "C"),
    (' ', "d"),
    ('+', "e1"),
    ('+', "e2"),
    (' ', "f"),
];

#[test]
fn a_line_the_agent_inserts_above_moves_every_line_down_one() {
    // The base holds still while the agent edits the working tree: lines keep their old
    // numbers, added ones their place after the line before them.
    let mut edited = vec![('+', "z")];
    edited.extend_from_slice(READ);
    assert_eq!(
        remap_lines(&diff(READ), &diff(&edited)),
        [1, 2, 3, 4, 5, 6, 7, 8]
    );
    // The same diff again maps every line onto itself.
    assert_eq!(
        remap_lines(&diff(READ), &diff(READ)),
        [0, 1, 2, 3, 4, 5, 6, 7]
    );
}

#[test]
fn an_added_run_the_agent_shortens_keeps_its_lines_on_its_last() {
    let shorter = [
        (' ', "a"),
        (' ', "b"),
        ('-', "c"),
        ('+', "C"),
        (' ', "d"),
        ('+', "e"),
        (' ', "f"),
    ];
    assert_eq!(
        remap_lines(&diff(READ), &diff(&shorter)),
        [0, 1, 2, 3, 4, 5, 5, 6]
    );
    // The change reverted: its added line goes to the line before it.
    let reverted = [(' ', "a"), (' ', "b"), (' ', "c"), (' ', "d"), (' ', "f")];
    assert_eq!(
        remap_lines(&diff(READ), &diff(&reverted)),
        [0, 1, 2, 2, 3, 3, 3, 4]
    );
}

#[test]
fn a_commit_of_part_of_the_change_keeps_the_working_trees_numbers() {
    // The agent commits `C`: the base moved, the working tree didn't.
    let committed = [
        (' ', "a"),
        (' ', "b"),
        (' ', "C"),
        (' ', "d"),
        ('+', "e1"),
        ('+', "e2"),
        (' ', "f"),
    ];
    // `c` (removed, on the old side only) goes to the line before it.
    assert_eq!(
        remap_lines(&diff(READ), &diff(&committed)),
        [0, 1, 1, 2, 3, 4, 5, 6]
    );
}

#[test]
fn when_both_sides_changed_lines_map_by_their_own_number() {
    let other = [(' ', "x"), ('-', "y"), ('+', "Y"), (' ', "w")];
    // a, b: new 1 and 2; c: old 3 (`w`); C: new 3 (`w`); d, e1, e2, f: new 4 to 7, past the
    // new side's end, so its last line.
    assert_eq!(
        remap_lines(&diff(READ), &diff(&other)),
        [0, 2, 3, 3, 3, 3, 3, 3]
    );
}

#[test]
fn nothing_maps_into_an_empty_diff() {
    assert!(remap_lines(&diff(READ), &[]).is_empty());
    assert!(remap_lines(&[], &diff(READ)).is_empty());
}
