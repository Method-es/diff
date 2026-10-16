use super::*;
use crate::diff::LineKind;

/// Lines from a pattern: `.` an unchanged line, `-` removed, `+` added.
fn lines(pattern: &str) -> Vec<DiffLine> {
    pattern
        .chars()
        .map(|c| DiffLine {
            kind: match c {
                '.' => LineKind::Context,
                '-' => LineKind::Removed,
                '+' => LineKind::Added,
                other => panic!("bad pattern char {other:?}"),
            },
            old_no: None,
            new_no: None,
            text: String::new(),
            words: Vec::new(),
        })
        .collect()
}

/// Segments as `L<start>..<end>` and `G<start>..<end><above><below>` (`^`/`_` for expandable
/// or not).
fn show(segments: &[Segment]) -> Vec<String> {
    segments
        .iter()
        .map(|segment| match segment {
            Segment::Lines(range) => format!("L{}..{}", range.start, range.end),
            Segment::Gap {
                lines,
                expandable_above,
                expandable_below,
            } => format!(
                "G{}..{}{}{}",
                lines.start,
                lines.end,
                if *expandable_above { "^" } else { "_" },
                if *expandable_below { "^" } else { "_" }
            ),
        })
        .collect()
}

#[test]
fn unchanged_runs_beyond_the_context_become_gaps() {
    // 10 unchanged, a change, 10 unchanged, a change, 10 unchanged.
    let lines = lines(&format!(
        "{}-+{}-{}",
        ".".repeat(10),
        ".".repeat(10),
        ".".repeat(10)
    ));
    assert_eq!(
        show(&segments(&lines, 3)),
        ["G0..7_^", "L7..15", "G15..19^^", "L19..26", "G26..33^_"]
    );
    // Context large enough to bridge the middle run.
    assert_eq!(
        show(&segments(&lines, 5)),
        ["G0..5_^", "L5..28", "G28..33^_"]
    );
}

#[test]
fn a_run_shorter_than_min_gap_stays_shown() {
    // Seven unchanged lines between changes with context 3 leave one hidden line.
    let lines = lines("-.......+");
    assert_eq!(show(&segments(&lines, 3)), ["L0..9"]);
    // Eight leave two, which collapse.
    let lines = super::tests::lines("-........+");
    assert_eq!(show(&segments(&lines, 3)), ["L0..4", "G4..6^^", "L6..10"]);
}

#[test]
fn context_zero_shows_only_changes() {
    let lines = lines("..-..+..");
    assert_eq!(
        show(&segments(&lines, 0)),
        ["G0..2_^", "L2..3", "G3..5^^", "L5..6", "G6..8^_"]
    );
}

#[test]
fn diffs_without_changes() {
    assert!(segments(&[], 3).is_empty());
    assert_eq!(show(&segments(&lines("....."), 3)), ["G0..5__"]);
    assert_eq!(show(&segments(&lines("."), 3)), ["L0..1"]);
    // A file that is all changes is one shown run.
    assert_eq!(show(&segments(&lines("+++"), 3)), ["L0..3"]);
}

#[test]
fn lines_marked_shown_stay_out_of_gaps() {
    let lines = lines(&format!("-{}+", ".".repeat(60)));
    let mut also = vec![false; 62];
    // A run the reviewer had open in the middle of the gap, and one line next to the context.
    also[30..36].fill(true);
    also[4] = true;
    assert_eq!(
        show(&segments_showing(&lines, 3, &also)),
        ["L0..5", "G5..30^^", "L30..36", "G36..58^^", "L58..62"]
    );
    // A marked line that would leave a one-line gap takes that line along (MIN_GAP).
    also[28] = true;
    assert_eq!(
        show(&segments_showing(&lines, 3, &also)),
        ["L0..5", "G5..28^^", "L28..36", "G36..58^^", "L58..62"]
    );
    // Nothing marked, or a mask shorter than the lines: the context rule alone.
    assert_eq!(segments_showing(&lines, 3, &[]), segments(&lines, 3));
    assert_eq!(
        show(&segments_showing(
            &super::tests::lines("....."),
            3,
            &[false, true]
        )),
        ["L0..2", "G2..5^_"]
    );
}

#[test]
fn expanding_reveals_twenty_lines_from_either_edge() {
    let lines = lines(&format!("-{}+", ".".repeat(60)));
    let base = segments(&lines, 3);
    assert_eq!(show(&base), ["L0..4", "G4..58^^", "L58..62"]);

    let from_above = expand_gap(&base, 1, Expand::FromAbove);
    assert_eq!(show(&from_above), ["L0..24", "G24..58^^", "L58..62"]);
    let from_below = expand_gap(&base, 1, Expand::FromBelow);
    assert_eq!(show(&from_below), ["L0..4", "G4..38^^", "L38..62"]);
    let all = expand_gap(&base, 1, Expand::All);
    assert_eq!(show(&all), ["L0..62"]);

    // Repeated steps shrink the gap until it is gone; a remainder under MIN_GAP goes with the
    // last step (54 hidden: 20 + 20 leaves 14, then 14 is revealed whole).
    let twice = expand_gap(&from_above, 1, Expand::FromBelow);
    assert_eq!(show(&twice), ["L0..24", "G24..38^^", "L38..62"]);
    let thrice = expand_gap(&twice, 1, Expand::FromAbove);
    assert_eq!(show(&thrice), ["L0..62"]);
}

#[test]
fn expanding_leaves_no_one_line_gap() {
    // 21 hidden lines: one step would leave a single line, which is revealed as well.
    let lines = lines(&format!("-{}+", ".".repeat(27)));
    let base = segments(&lines, 3);
    assert_eq!(show(&base), ["L0..4", "G4..25^^", "L25..29"]);
    assert_eq!(show(&expand_gap(&base, 1, Expand::FromAbove)), ["L0..29"]);
    assert_eq!(show(&expand_gap(&base, 1, Expand::FromBelow)), ["L0..29"]);
}

#[test]
fn edge_gaps_expand_toward_the_shown_lines() {
    let lines = lines(&format!("{}-{}", ".".repeat(30), ".".repeat(30)));
    let base = segments(&lines, 3);
    assert_eq!(show(&base), ["G0..27_^", "L27..34", "G34..61^_"]);
    // The leading gap reveals its bottom lines, joining the shown run below.
    let leading = expand_gap(&base, 0, Expand::FromBelow);
    assert_eq!(show(&leading), ["G0..7_^", "L7..34", "G34..61^_"]);
    // The trailing gap reveals its top lines.
    let trailing = expand_gap(&leading, 2, Expand::FromAbove);
    assert_eq!(show(&trailing), ["G0..7_^", "L7..54", "G54..61^_"]);
    // Revealing from the side without shown lines still works; the gap's top lines appear
    // above it.
    let odd = expand_gap(&base, 0, Expand::FromAbove);
    assert_eq!(show(&odd), ["L0..20", "G20..27^^", "L27..34", "G34..61^_"]);
}

#[test]
fn expanding_anything_but_a_gap_changes_nothing() {
    let lines = lines(&format!("-{}+", ".".repeat(30)));
    let base = segments(&lines, 3);
    assert_eq!(expand_gap(&base, 0, Expand::All), base);
    assert_eq!(expand_gap(&base, 7, Expand::All), base);
}

#[test]
fn segments_cover_every_line_once_in_order() {
    for pattern in [
        "",
        "-",
        "..........-.....+..........",
        "+.-.+.-",
        "-..........",
    ] {
        let lines = lines(pattern);
        for context in [0, 1, 3, 20] {
            let mut segs = segments(&lines, context);
            for step in 0..6 {
                let mut next = 0;
                for segment in &segs {
                    let range = segment.range();
                    assert_eq!(range.start, next, "{pattern} {context} {segs:?}");
                    assert!(!range.is_empty());
                    next = range.end;
                }
                assert_eq!(next, lines.len());
                // Every change is shown.
                for segment in segs.iter().filter(|s| s.is_gap()) {
                    assert!(
                        segment.range().all(|i| !lines[i].kind.is_change()),
                        "{pattern} {segs:?}"
                    );
                }
                let Some(gap) = segs.iter().position(Segment::is_gap) else {
                    break;
                };
                let how = [Expand::FromAbove, Expand::FromBelow, Expand::All][step % 3];
                segs = expand_gap(&segs, gap, how);
            }
        }
    }
}

#[test]
fn revealing_inside_a_gap_opens_just_the_lines_asked_for() {
    // A change, 30 unchanged lines, a change.
    let lines = lines(&format!("-{}+", ".".repeat(30)));
    let base = segments(&lines, 3);
    assert_eq!(show(&base), ["L0..4", "G4..28^^", "L28..32"]);
    // Lines 14..21 out of the middle: two gaps left, each expandable towards the lines shown.
    assert_eq!(
        show(&reveal_in_gap(&base, 1, 14..21)),
        ["L0..4", "G4..14^^", "L14..21", "G21..28^^", "L28..32"]
    );
    // Clamped to the gap; what is left of it under MIN_GAP goes too.
    assert_eq!(
        show(&reveal_in_gap(&base, 1, 0..9)),
        ["L0..9", "G9..28^^", "L28..32"]
    );
    assert_eq!(show(&reveal_in_gap(&base, 1, 5..27)), ["L0..32"]);
    // Not a gap, or nothing of it: unchanged.
    assert_eq!(reveal_in_gap(&base, 0, 0..3), base);
    assert_eq!(reveal_in_gap(&base, 1, 28..30), base);
    assert_eq!(reveal_in_gap(&base, 9, 0..3), base);
}
