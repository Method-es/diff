//! `Ctrl F` in the stream (PLAN §18.1, DESIGN §4.1): matches over both sides and every file,
//! a file loaded for the search, a match inside a gap opening it, `Enter` / `Shift Enter`
//! wrapping, smart case, files that can't be searched, and `Esc`.

use pretty_assertions::assert_eq;

use super::*;
use crate::diff_pane::FindSummary;
use gpui::Focusable as _;

/// `a.txt` with `needle` in a gap (line 20) on both sides and in a change; seven more files of
/// 20 changed lines so the last ones are well below the viewport; `z.txt` with `needle` in an
/// addition; `bin.dat` binary.
fn find_fixture() -> FixtureRepo {
    let repo = FixtureRepo::new();
    let mut a: String = (1..=40).map(|i| format!("a line {i}\n")).collect();
    a = a.replace("a line 20\n", "a needle in the gap\n");
    repo.write("a.txt", &a);
    for n in 1..=7 {
        let text: String = (1..=20).map(|i| format!("m{n} {i}\n")).collect();
        repo.write(format!("m{n}.txt"), &text);
    }
    repo.write("bin.dat", "\0\x01binary");
    repo.commit("base");
    repo.write("a.txt", a.replace("a line 2\n", "a Needle changed\n"));
    for n in 1..=7 {
        let text: String = (1..=20).map(|i| format!("m{n} {i} edited\n")).collect();
        repo.write(format!("m{n}.txt"), &text);
    }
    repo.write("bin.dat", "\0\x02binary");
    repo.write("z.txt", "last\nthe needle at the end\n");
    repo
}

/// The review of `repo`, opened and settled with `files` files in the stream.
fn open_files<'a>(
    repo: &FixtureRepo,
    files: usize,
    cx: &'a mut TestAppContext,
) -> (tempfile::TempDir, Entity<Shell>, &'a mut VisualTestContext) {
    let (dir, shell, cx) = open_shell(cx);
    add_repo(repo, &shell, cx);
    let task = cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open(
                Location::Local(repo.path().to_path_buf()),
                OpenRequest::Target(Target::Working),
                window,
                cx,
            )
        })
    });
    cx.run_until_parked();
    let pane = shell.read_with(cx, |shell, _| shell.diff_pane().clone());
    settle_drawn(cx, &shell, "the stream", |_, cx| {
        pane.read(cx).blocks.len() == files
    });
    cx.foreground_executor()
        .block_test(task)
        .expect("the review opens");
    (dir, shell, cx)
}

fn find(pane: &Entity<DiffPane>, cx: &mut VisualTestContext) -> FindSummary {
    pane.read_with(cx, |pane, _| pane.find_summary().expect("a review is open"))
}

fn state(shell: &Entity<Shell>, path: &str, cx: &mut VisualTestContext) -> &'static str {
    shell.read_with(cx, |shell, cx| {
        shell
            .session()
            .and_then(|session| session.read(cx).file_state(path).map(FileState::key))
            .unwrap_or("none")
    })
}

/// Types `text` into the focused field and lets the search run.
fn type_query(shell: &Entity<Shell>, text: &str, cx: &mut VisualTestContext) {
    cx.simulate_input(text);
    settle_drawn(cx, shell, "the search", |_, _| true);
}

fn find_focused(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> bool {
    let pane = pane(shell, cx);
    cx.update(|window, cx| pane.read(cx).is_find_focused(window, cx))
}

#[gpui::test]
fn ctrl_f_searches_every_file_both_sides_and_enter_goes_through_the_matches(
    cx: &mut TestAppContext,
) {
    let repo = find_fixture();
    let (_dir, shell, cx) = open_files(&repo, 10, cx);
    let pane = pane(&shell, cx);
    // The last file is far below the viewport: not loaded until something asks for it.
    assert_eq!(state(&shell, "z.txt", cx), "unloaded");
    press(cx, &shell, "ctrl-f");
    assert!(find_focused(&shell, cx));
    assert!(find(&pane, cx).open);
    type_query(&shell, "needle", cx);
    let found = find(&pane, cx);
    // a.txt: the gap's line (once, on the new column: a context line matches once in split
    // view too), the changed line's new side (smart case: the query has no capital, so
    // `Needle` matches); z.txt's addition. bin.dat can't be searched.
    assert_eq!(found.count, 3, "{found:?}");
    assert!(found.complete);
    assert_eq!(found.skipped, 1);
    assert_eq!(state(&shell, "z.txt", cx), "loaded");
    // The first match from the top of the stream: a.txt's changed line, new side.
    assert_eq!(found.current, Some(1));
    assert_eq!(cursor(&pane, cx), new_line("a.txt", 2));
    // The keyboard stays in the field: Enter goes on, into the gap, which opens around it.
    assert_eq!(summary(&pane, "a.txt", cx).gaps.len(), 1);
    press(cx, &shell, "enter");
    assert_eq!(find(&pane, cx).current, Some(2));
    assert_eq!(cursor(&pane, cx), new_line("a.txt", 20));
    let gaps = summary(&pane, "a.txt", cx).gaps;
    assert_eq!(gaps, [11, 17], "line 20 and 3 lines either side shown");
    press(cx, &shell, "enter");
    assert_eq!(find(&pane, cx).current, Some(3));
    assert_eq!(cursor(&pane, cx), new_line("z.txt", 2));
    // Wrapping both ways.
    press(cx, &shell, "enter");
    assert_eq!(find(&pane, cx).current, Some(1));
    press(cx, &shell, "shift-enter");
    assert_eq!(find(&pane, cx).current, Some(3));
    assert_eq!(cursor(&pane, cx), new_line("z.txt", 2));
    // The caret sits at the match's start, for what language features do there.
    let caret = pane.read_with(cx, |pane, _| pane.caret()).unwrap();
    assert_eq!((caret.1.line, caret.1.column), (2, 5));
    // Esc closes the bar and gives the keyboard back to the stream, the cursor on the match.
    press(cx, &shell, "escape");
    assert!(!find(&pane, cx).open);
    assert!(!find_focused(&shell, cx));
    let stream = shell.update_in(cx, |shell, window, cx| {
        shell.focus_handle(cx).is_focused(window)
    });
    assert!(stream);
    assert_eq!(cursor(&pane, cx), new_line("z.txt", 2));
}

#[gpui::test]
fn a_capital_makes_the_search_case_sensitive_and_unified_counts_a_line_once(
    cx: &mut TestAppContext,
) {
    let repo = find_fixture();
    let (_dir, shell, cx) = open_files(&repo, 10, cx);
    let pane = pane(&shell, cx);
    press(cx, &shell, "ctrl-f");
    type_query(&shell, "Needle", cx);
    assert_eq!(find(&pane, cx).count, 1);
    press(cx, &shell, "escape");
    // In unified view the gap's context line is one row: one match, not two.
    press(cx, &shell, "s ctrl-f");
    type_query(&shell, "needle", cx);
    assert_eq!(find(&pane, cx).count, 3);
    // Nothing matches: no current one.
    press(cx, &shell, "ctrl-a");
    type_query(&shell, "haystack", cx);
    let found = find(&pane, cx);
    assert_eq!((found.count, found.current), (0, None));
    assert_eq!(found.label().as_deref(), Some("No results"));
}

#[gpui::test]
fn ctrl_f_takes_a_one_line_selection_as_its_query(cx: &mut TestAppContext) {
    let repo = find_fixture();
    let (_dir, shell, cx) = open_files(&repo, 10, cx);
    let pane = pane(&shell, cx);
    pane.update(cx, |pane, cx| {
        pane.text_selection = Some(select::TextSelection {
            anchor: TextPoint {
                path: "a.txt".to_string(),
                side: AnchorSide::New,
                line: 2,
                offset: 2,
            },
            head: TextPoint {
                path: "a.txt".to_string(),
                side: AnchorSide::New,
                line: 2,
                offset: 8,
            },
            side: Some(AnchorSide::New),
        });
        cx.notify();
    });
    press(cx, &shell, "ctrl-f");
    let found = find(&pane, cx);
    assert_eq!(found.query, "Needle");
    assert_eq!(found.count, 1);
    // Ctrl F again keeps the bar and its query.
    press(cx, &shell, "ctrl-f");
    assert!(find_focused(&shell, cx));
    assert_eq!(find(&pane, cx).query, "Needle");
}

#[gpui::test]
fn the_first_match_is_the_next_one_in_the_rows_order_and_enter_searches_typing_first(
    cx: &mut TestAppContext,
) {
    // Split view pairs `old two` with `new two` on the row after the cursor's (`new one`),
    // although `old two` comes before `new one` among the diff's lines.
    let repo = FixtureRepo::new();
    repo.write("a.txt", "ctx\nold foo\nold two\nend\n");
    repo.commit("base");
    repo.write("a.txt", "ctx\nnew one\nnew two\nend\n");
    let (_dir, shell, cx) = open_files(&repo, 1, cx);
    let pane = pane(&shell, cx);
    pane.update(cx, |pane, cx| {
        let pos = LinePos {
            line: 3,
            side: AnchorSide::New,
        };
        pane.click_line("a.txt", pos, false, cx);
    });
    assert_eq!(cursor(&pane, cx), new_line("a.txt", 2));
    press(cx, &shell, "ctrl-f");
    type_query(&shell, "two", cx);
    assert_eq!(find(&pane, cx).current, Some(1));
    assert_eq!(
        cursor(&pane, cx),
        Some(("a.txt".to_string(), AnchorSide::Old, 3))
    );
    // `Enter` right after typing, before the pause: the new query is searched and its first
    // match shown, not the old query's next.
    press(cx, &shell, "ctrl-a");
    cx.simulate_input("end");
    cx.simulate_keystrokes("enter");
    let found = find(&pane, cx);
    assert_eq!(found.query, "end");
    assert_eq!(found.current, Some(1));
    settle_drawn(cx, &shell, "the search", |_, _| true);
    // A context line: its new column's.
    assert_eq!(cursor(&pane, cx), new_line("a.txt", 4));
}
