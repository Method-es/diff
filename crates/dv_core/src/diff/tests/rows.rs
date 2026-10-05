//! The row helpers against git's own output (PLAN §6 step 5): a gap row's hunk range is the
//! header git prints for the same lines.

use super::*;

#[test]
fn gap_rows_show_the_hunk_headers_git_prints() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", numbered(60));
    repo.commit("base");
    // A changed line, a replaced pair, an insertion and a deletion near the end, each further
    // than two contexts apart, so git prints one hunk per change as the stream shows one run.
    // (A single unchanged line between runs, or before the first, stays shown, so there the
    // stream's run is one line longer than git's hunk: `MIN_GAP`.)
    let mut lines: Vec<String> = (1..=60).map(|n| format!("line {n}")).collect();
    lines[5] = "line 6 changed".to_string();
    lines[29] = "line 30 changed".to_string();
    lines[30] = "line 31 changed".to_string();
    lines.insert(45, "inserted after 45".to_string());
    lines.insert(46, "inserted again".to_string());
    lines.remove(58);
    repo.write("a.txt", lines.join("\n") + "\n");

    let listed = Listed::of(&repo, Target::Working);
    let text = listed.text("a.txt");
    let ours: Vec<String> = text
        .segments(3)
        .iter()
        .filter_map(|segment| match segment {
            Segment::Lines(range) => Some(hunk_range(&text.lines, range.clone()).to_string()),
            Segment::Gap { .. } => None,
        })
        .collect();
    let gits: Vec<String> = repo
        .git(&["diff", "-U3", "--no-color", "HEAD", "--", "a.txt"])
        .lines()
        .filter(|line| line.starts_with("@@ "))
        // git adds the enclosing "function" after the second `@@`.
        .map(|line| {
            let end = line[3..].find(" @@").unwrap() + 6;
            line[..end].to_string()
        })
        .collect();
    assert_eq!(gits.len(), 4, "{gits:?}");
    assert_eq!(ours, gits);
}
