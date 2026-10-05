use super::*;

/// Lines from a pattern, numbered as git numbers them: `.` an unchanged line (both sides), `-`
/// removed (old side), `+` added (new side).
fn lines(pattern: &str) -> Vec<DiffLine> {
    let (mut old, mut new) = (0, 0);
    pattern
        .chars()
        .map(|c| {
            let kind = match c {
                '.' => LineKind::Context,
                '-' => LineKind::Removed,
                '+' => LineKind::Added,
                other => panic!("bad pattern char {other:?}"),
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
                text: String::new(),
                words: Vec::new(),
            }
        })
        .collect()
}

/// Rows as `<old>|<new>` line indexes, `_` for an empty cell.
fn show(rows: &[SplitRow]) -> Vec<String> {
    let cell = |index: Option<usize>| index.map_or("_".to_string(), |i| i.to_string());
    rows.iter()
        .map(|row| format!("{}|{}", cell(row.old), cell(row.new)))
        .collect()
}

#[test]
fn split_rows_pair_the_ith_removed_line_with_the_ith_added_one() {
    // (pattern, range, rows)
    let cases: &[(&str, Range<usize>, &[&str])] = &[
        ("...", 0..3, &["0|0", "1|1", "2|2"]),
        // A change: two removed against two added.
        (".--++.", 0..6, &["0|0", "1|3", "2|4", "5|5"]),
        // More removed than added: the extra removed line faces an empty cell.
        (".---+.", 0..6, &["0|0", "1|4", "2|_", "3|_", "5|5"]),
        // More added than removed.
        ("-+++", 0..4, &["0|1", "_|2", "_|3"]),
        // Pure additions and pure removals.
        ("++", 0..2, &["_|0", "_|1"]),
        ("--.", 0..3, &["0|_", "1|_", "2|2"]),
        // Separate runs pair separately.
        ("-+.-+", 0..5, &["0|1", "2|2", "3|4"]),
        // Interleaved within one run still pairs by position on each side.
        ("-+-+", 0..4, &["0|1", "2|3"]),
        // Only the given range.
        ("..-+..", 2..4, &["2|3"]),
        ("..-+..", 4..6, &["4|4", "5|5"]),
        ("", 0..0, &[]),
    ];
    for (pattern, range, expected) in cases {
        assert_eq!(
            show(&split_rows(&lines(pattern), range.clone())),
            *expected,
            "{pattern:?} {range:?}"
        );
    }
}

#[test]
fn hunk_ranges_read_like_gits_headers() {
    // (pattern, range, header)
    let cases: &[(&str, Range<usize>, &str)] = &[
        // A run in the middle: lines 4-8 old, 4-9 new.
        ("...........", 3..8, "@@ -4,5 +4,5 @@"),
        ("...-++....", 1..7, "@@ -2,4 +2,5 @@"),
        // One line on a side drops its count.
        ("..-+..", 2..4, "@@ -3 +3 @@"),
        // Only additions: the old side starts at the line before them.
        ("...++..", 3..5, "@@ -3,0 +4,2 @@"),
        // Only removals: the new side starts at the line before them.
        ("..--..", 2..4, "@@ -3,2 +2,0 @@"),
        // A new file and a deleted file.
        ("+++", 0..3, "@@ -0,0 +1,3 @@"),
        ("---", 0..3, "@@ -1,3 +0,0 @@"),
        // Additions at the very top of an existing file.
        ("+...", 0..1, "@@ -0,0 +1 @@"),
    ];
    for (pattern, range, header) in cases {
        assert_eq!(
            hunk_range(&lines(pattern), range.clone()).to_string(),
            *header,
            "{pattern:?} {range:?}"
        );
    }
    assert_eq!(
        hunk_range(&lines("..-+.."), 1..5),
        HunkRange {
            old_start: 2,
            old_count: 3,
            new_start: 2,
            new_count: 3
        }
    );
}

#[test]
fn a_gap_rows_hunk_range_matches_git_for_a_real_diff() {
    // git's own header for a one-line change with 3 lines of context, in a 20-line file whose
    // line 10 changed: `@@ -7,7 +7,7 @@`. The same run collapsed out of the whole-file diff.
    let mut pattern = ".".repeat(9);
    pattern.push_str("-+");
    pattern.push_str(&".".repeat(10));
    let lines = lines(&pattern);
    let segments = crate::diff::segments(&lines, 3);
    let shown = segments
        .iter()
        .find_map(|segment| match segment {
            crate::diff::Segment::Lines(range) => Some(range.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(hunk_range(&lines, shown).to_string(), "@@ -7,7 +7,7 @@");
}
