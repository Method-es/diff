//! A thread's excerpt (PLAN §10.3): the window around its lines, the other side's rows between,
//! long runs and spans cut to their ends, the scope row, and every number the new side's.

use crate::diff::{DiffLine, LineKind, TextDiff};
use crate::review::excerpt::{LINE_MAX, excerpt, excerpt_lines};

use super::*;

/// A text diff from rows of `(mark, text)`: `' '` unchanged, `+` added, `-` removed, numbered
/// as git numbers them.
fn diff(rows: &[(char, &str)]) -> TextDiff {
    let (mut old, mut new) = (0u32, 0u32);
    let lines = rows
        .iter()
        .map(|&(mark, text)| {
            let kind = match mark {
                '+' => LineKind::Added,
                '-' => LineKind::Removed,
                _ => LineKind::Context,
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
        .collect();
    TextDiff {
        lines,
        ..TextDiff::empty()
    }
}

/// The excerpt of lines `start..=end` of `side`, as printed.
fn shown(text: &TextDiff, side: AnchorSide, start: u32, end: u32) -> Vec<String> {
    let rows = excerpt(text, side, start, end).expect("the lines are on the side");
    excerpt_lines(text, &rows)
}

/// A function whose two lines were rewritten: two removed, two added, one kept.
fn rewritten() -> TextDiff {
    diff(&[
        (' ', "export function Dialog() {"),
        (' ', "  const a = 1;"),
        (' ', "  const b = 2;"),
        (' ', "  const c = 3;"),
        (' ', ""),
        ('-', "  async function onSubmit() {"),
        ('-', "    await save();"),
        ('+', "  function handleSubmit() {"),
        ('+', "    mutation.mutate();"),
        (' ', "  }"),
        (' ', ""),
        (' ', "  if (!open) return null;"),
        (' ', ""),
        (' ', "  return form;"),
    ])
}

/// A request helper with one line removed from its middle.
fn removed_line() -> TextDiff {
    diff(&[
        (' ', "async function request() {"),
        (' ', "  return fetch(url, {"),
        (' ', "    ...init,"),
        ('-', "    credentials: \"include\","),
        (' ', "    headers: {},"),
        (' ', "  });"),
        (' ', "}"),
        (' ', ""),
    ])
}

#[test]
fn a_new_side_range_shows_the_lines_it_replaced_and_its_scope() {
    assert_eq!(
        shown(&rewritten(), AnchorSide::New, 6, 8),
        [
            "  1   | export function Dialog() {",
            "  ⋮",
            "  3   |   const b = 2;",
            "  4   |   const c = 3;",
            "  5   |",
            "    - |   async function onSubmit() {",
            "    - |     await save();",
            "> 6 + |   function handleSubmit() {",
            "> 7 + |     mutation.mutate();",
            "> 8   |   }",
            "  9   |",
            " 10   |   if (!open) return null;",
            " 11   |",
        ]
    );
}

#[test]
fn an_old_side_removed_line_is_marked_without_a_number() {
    assert_eq!(
        shown(&removed_line(), AnchorSide::Old, 4, 4),
        [
            " 1   | async function request() {",
            " 2   |   return fetch(url, {",
            " 3   |     ...init,",
            ">  - |     credentials: \"include\",",
            " 4   |     headers: {},",
            " 5   |   });",
            " 6   | }",
        ],
        "the scope line stands in the window already"
    );
}

#[test]
fn an_old_side_unchanged_line_is_numbered_as_the_file_is_now() {
    // Old line 6 is new line 5; the removed line is on the old side, so it counts as context.
    assert_eq!(
        shown(&removed_line(), AnchorSide::Old, 6, 6),
        [
            " 1   | async function request() {",
            " ⋮",
            " 3   |     ...init,",
            "   - |     credentials: \"include\",",
            " 4   |     headers: {},",
            ">5   |   });",
            " 6   | }",
            " 7   |",
        ]
    );
}

#[test]
fn an_added_file_is_all_added_and_a_deleted_one_unnumbered() {
    let added = diff(&[
        ('+', "export const a = 1;"),
        ('+', "export const b = 2;"),
        ('+', ""),
        ('+', "export function f() {"),
        ('+', "  return a + b;"),
        ('+', "}"),
    ]);
    assert_eq!(
        shown(&added, AnchorSide::New, 5, 5),
        [
            " 2 + | export const b = 2;",
            " 3 + |",
            " 4 + | export function f() {",
            ">5 + |   return a + b;",
            " 6 + | }",
        ]
    );
    let deleted = diff(&[('-', "fn a() {}"), ('-', "fn b() {}"), ('-', "fn c() {}")]);
    assert_eq!(
        shown(&deleted, AnchorSide::Old, 2, 2),
        ["   - | fn a() {}", ">  - | fn b() {}", "   - | fn c() {}"]
    );
    assert_eq!(
        excerpt(&deleted, AnchorSide::New, 1, 1),
        None,
        "no new side"
    );
}

#[test]
fn the_first_and_the_last_line() {
    let text = diff(&[
        ('-', "// old header"),
        ('-', "// more"),
        ('+', "import x;"),
        (' ', "import y;"),
        (' ', ""),
        (' ', "run();"),
        (' ', "stop();"),
    ]);
    assert_eq!(
        shown(&text, AnchorSide::New, 1, 1),
        [
            "   - | // old header",
            "   - | // more",
            ">1 + | import x;",
            " 2   | import y;",
            " 3   |",
            " 4   | run();",
        ],
        "the removed lines above line 1 come along; nothing above them to scope it"
    );
    assert_eq!(
        shown(&text, AnchorSide::New, 5, 5),
        [
            " 2   | import y;",
            " 3   |",
            " 4   | run();",
            ">5   | stop();"
        ]
    );
}

#[test]
fn no_scope_row_when_no_line_above_starts_one() {
    let text = diff(&[
        (' ', "  a"),
        (' ', "  b"),
        (' ', "  c"),
        (' ', "  d"),
        (' ', "  e"),
        (' ', "  f"),
    ]);
    assert_eq!(
        shown(&text, AnchorSide::New, 6, 6),
        [" 3   |   c", " 4   |   d", " 5   |   e", ">6   |   f"]
    );
    // `_` and `$` start a scope too; a scope right above the window needs no gap.
    let text = diff(&[
        (' ', "$handler = function () {"),
        (' ', "  a"),
        (' ', "  b"),
        (' ', "  c"),
        (' ', "  d"),
    ]);
    assert_eq!(
        shown(&text, AnchorSide::New, 5, 5),
        [
            " 1   | $handler = function () {",
            " 2   |   a",
            " 3   |   b",
            " 4   |   c",
            ">5   |   d",
        ]
    );
    let text = diff(&[
        (' ', "_private:"),
        (' ', "  a"),
        (' ', "  b"),
        (' ', "  c"),
        (' ', "  d"),
        (' ', "  e"),
    ]);
    assert_eq!(
        shown(&text, AnchorSide::New, 6, 6)[..2],
        [" 1   | _private:", " ⋮"]
    );
}

#[test]
fn a_long_run_of_removed_lines_keeps_its_ends() {
    let removed: Vec<String> = (1..=40).map(|n| format!("  old {n}")).collect();
    let mut rows = vec![(' ', "fn main() {")];
    rows.extend(removed.iter().map(|line| ('-', line.as_str())));
    rows.extend([('+', "  new();"), (' ', "}")]);
    assert_eq!(
        shown(&diff(&rows), AnchorSide::New, 2, 2),
        [
            " 1   | fn main() {",
            "   - |   old 1",
            "   - |   old 2",
            "   - |   old 3",
            " ⋮ 34 more removed lines",
            "   - |   old 38",
            "   - |   old 39",
            "   - |   old 40",
            ">2 + |   new();",
            " 3   | }",
        ]
    );
    // An old-side thread's run is of added lines.
    let added: Vec<String> = (1..=9).map(|n| format!("  new {n}")).collect();
    let mut rows = vec![(' ', "fn main() {"), ('-', "  old();")];
    rows.extend(added.iter().map(|line| ('+', line.as_str())));
    rows.push((' ', "}"));
    let shown = shown(&diff(&rows), AnchorSide::Old, 2, 2);
    assert_eq!(shown[1], ">   - |   old();");
    assert_eq!(shown[5], "  ⋮ 3 more added lines");
}

#[test]
fn a_long_commented_span_keeps_its_ends() {
    let lines: Vec<String> = (1..=70).map(|n| format!("x{n}")).collect();
    let rows: Vec<(char, &str)> = lines.iter().map(|line| ('+', line.as_str())).collect();
    let shown = shown(&diff(&rows), AnchorSide::New, 5, 64);
    assert_eq!(shown.len(), 3 + 10 + 1 + 10 + 3, "{shown:#?}");
    assert_eq!(shown[0], "  2 + | x2");
    assert_eq!(shown[3], "> 5 + | x5");
    assert_eq!(shown[12], ">14 + | x14");
    assert_eq!(shown[13], "  ⋮ 40 lines");
    assert_eq!(shown[14], ">55 + | x55");
    assert_eq!(shown[23], ">64 + | x64");
    assert_eq!(shown[26], " 67 + | x67");
    // 24 rows are shown whole.
    let whole = excerpt(&diff(&rows), AnchorSide::New, 5, 28).unwrap();
    assert_eq!(whole.len(), 3 + 24 + 3);
}

#[test]
fn a_long_line_is_cut() {
    let long = "a".repeat(1000);
    let shown = shown(&diff(&[('+', long.as_str())]), AnchorSide::New, 1, 1);
    let code = shown[0].strip_prefix(">1 + | ").unwrap();
    assert_eq!(code.chars().count(), LINE_MAX);
    assert!(code.ends_with("a…"), "{code}");
    let exact = "b".repeat(LINE_MAX);
    let shown = excerpt_lines(
        &diff(&[(' ', exact.as_str())]),
        &excerpt(&diff(&[(' ', exact.as_str())]), AnchorSide::New, 1, 1).unwrap(),
    );
    assert_eq!(
        shown[0],
        format!(">1   | {exact}"),
        "{LINE_MAX} characters fit"
    );
}

#[test]
fn numbers_align_to_the_widest() {
    let lines: Vec<String> = (1..=12).map(|n| format!("  l{n}")).collect();
    let rows: Vec<(char, &str)> = lines.iter().map(|line| (' ', line.as_str())).collect();
    let text = diff(&rows);
    assert_eq!(
        shown(&text, AnchorSide::New, 6, 6),
        [
            " 3   |   l3",
            " 4   |   l4",
            " 5   |   l5",
            ">6   |   l6",
            " 7   |   l7",
            " 8   |   l8",
            " 9   |   l9",
        ]
    );
    assert_eq!(
        shown(&text, AnchorSide::New, 7, 7),
        [
            "  4   |   l4",
            "  5   |   l5",
            "  6   |   l6",
            "> 7   |   l7",
            "  8   |   l8",
            "  9   |   l9",
            " 10   |   l10",
        ]
    );
}

#[test]
fn no_row_ends_in_whitespace() {
    // A line of only whitespace in the context and in the commented range, and a line ending
    // in spaces.
    let text = diff(&[
        (' ', "fn main() {"),
        (' ', "    "),
        ('+', "  let a = 1;   "),
        ('+', "\t"),
        (' ', "}"),
    ]);
    assert_eq!(
        shown(&text, AnchorSide::New, 3, 4),
        [
            " 1   | fn main() {",
            " 2   |",
            ">3 + |   let a = 1;",
            ">4 + |",
            " 5   | }",
        ]
    );
}

#[test]
fn a_span_counts_the_threads_own_lines() {
    // Twenty new lines with ten removed rows among them: 30 rows, but 20 lines, kept whole.
    let mut rows = vec![(' ', "fn main() {")];
    let added: Vec<String> = (1..=20).map(|n| format!("  new {n}")).collect();
    let removed: Vec<String> = (1..=10).map(|n| format!("  old {n}")).collect();
    for (n, line) in added.iter().enumerate() {
        rows.push(('+', line.as_str()));
        if n < 10 {
            rows.push(('-', removed[n].as_str()));
        }
    }
    rows.push((' ', "}"));
    let text = diff(&rows);
    let whole = shown(&text, AnchorSide::New, 2, 21);
    assert_eq!(whole.len(), 1 + 30 + 1, "{whole:#?}");
    assert!(!whole.iter().any(|row| row.contains('⋮')), "{whole:#?}");

    // Thirty lines with removed rows among them: the gap counts both.
    let added: Vec<String> = (1..=30).map(|n| format!("  new {n}")).collect();
    let mut rows = vec![(' ', "fn main() {")];
    for (n, line) in added.iter().enumerate() {
        rows.push(('+', line.as_str()));
        if n % 3 == 0 {
            rows.push(('-', "  old"));
        }
    }
    rows.push((' ', "}"));
    let shown = shown(&diff(&rows), AnchorSide::New, 2, 31);
    let gap = shown.iter().find(|row| row.contains('⋮')).unwrap();
    // `new 11` to `new 20` hidden (the last kept above is `new 10`, line 11; the first kept
    // below `new 21`, line 22), with the removed rows after `new 10`, `13`, `16` and `19`.
    assert_eq!(gap, "  ⋮ 10 lines, 4 removed");
    let at = shown.iter().position(|row| row == gap).unwrap();
    assert_eq!(shown[at - 1], ">11 + |   new 10");
    assert_eq!(shown[at + 1], ">22 + |   new 21");
}
