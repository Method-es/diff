//! The stream in a real window over real fixture repos: the files and their rows, the keys
//! (files, change blocks, the cursor and selection, sides, gaps, viewed, split / unified,
//! pages), the sideways scroll, and keys staying out of text fields.

use dv_core::review::AnchorSide;
use dv_core::{Location, Target};
use dv_test_support::FixtureRepo;
use gpui::{Bounds, Entity, Pixels, TestAppContext, VisualTestContext};
use pretty_assertions::assert_eq;

mod file_comment;
mod file_view;
mod find;
mod rounds;
mod section;
mod text_select;
mod threads;

use super::*;
use crate::services::reviews::OpenRequest;
use crate::shell::Shell;
use crate::test_support::{add_repo, bounds, open_shell, settle};

/// `a/long.txt` (80 lines, two lines edited far apart), `b.txt` (a change block and an
/// addition) and `c.txt` (deleted).
fn stream_fixture(lines: usize) -> FixtureRepo {
    let repo = FixtureRepo::new();
    let long: String = (1..=lines).map(|i| format!("line {i}\n")).collect();
    repo.write("a/long.txt", &long);
    repo.write("b.txt", "one\ntwo\nthree\n");
    repo.write("c.txt", "keep\n");
    repo.commit("base");
    let edited = long
        .replace("line 10\n", "line ten\n")
        .replace("line 70\n", "line seventy\n");
    repo.write("a/long.txt", &edited);
    repo.write("b.txt", "one\n2\nthree\nfour\n");
    repo.remove("c.txt");
    repo
}

/// A shell showing the working review of `repo`, drawn and settled.
fn open<'a>(
    repo: &FixtureRepo,
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
    settle_drawn(cx, &shell, "the files to load", |_, cx| {
        pane.read(cx).blocks.len() == 3 && pane.read(cx).wanted.len() == 3
    });
    cx.foreground_executor()
        .block_test(task)
        .expect("the review opens");
    (dir, shell, cx)
}

/// Draws a frame and settles until `done` holds, drawing in between, so the list lays out.
fn settle_drawn(
    cx: &mut VisualTestContext,
    shell: &Entity<Shell>,
    what: &str,
    done: impl Fn(&Shell, &gpui::App) -> bool,
) {
    draw(cx);
    settle(cx, shell, what, |shell, cx| done(shell, cx));
    draw(cx);
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn pane(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<DiffPane> {
    shell.read_with(cx, |shell, _| shell.diff_pane().clone())
}

/// Presses `keys` and lets everything they started land.
fn press(cx: &mut VisualTestContext, shell: &Entity<Shell>, keys: &str) {
    cx.simulate_keystrokes(keys);
    settle_drawn(cx, shell, keys, |_, _| true);
}

fn cursor(
    pane: &Entity<DiffPane>,
    cx: &mut VisualTestContext,
) -> Option<(String, AnchorSide, u32)> {
    pane.read_with(cx, |pane, _| {
        pane.cursor()
            .map(|cursor| (cursor.path, cursor.side, cursor.line))
    })
}

fn current(pane: &Entity<DiffPane>, cx: &mut VisualTestContext) -> Option<String> {
    pane.read_with(cx, |pane, _| pane.current_file().map(str::to_string))
}

fn summary(pane: &Entity<DiffPane>, path: &str, cx: &mut VisualTestContext) -> FileSummary {
    pane.read_with(cx, |pane, _| {
        pane.file_summaries()
            .into_iter()
            .find(|file| file.path == path)
            .expect("the file is in the stream")
    })
}

/// Where the comment `+` of `path`'s line `number` on `side` (`"old"`, `"new"`) drew, after a
/// fresh frame; `None` when it didn't.
fn plus(path: &str, side: &str, number: u32, cx: &mut VisualTestContext) -> Option<Bounds<Pixels>> {
    let name = format!("comment/{path}/{side}/{number}");
    bounds(cx, Box::leak(name.into_boxed_str()))
}

fn new_line(path: &str, line: u32) -> Option<(String, AnchorSide, u32)> {
    Some((path.to_string(), AnchorSide::New, line))
}

#[gpui::test]
fn every_file_is_a_block_with_its_rows_once_its_diff_lands(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    // Lines 10 and 70 changed: a gap above, between and below the two runs.
    let long = summary(&pane, "a/long.txt", cx);
    assert_eq!(long.gaps, [6, 53, 7]);
    assert!(!long.collapsed);
    assert_eq!(long.notice, None);
    // Header, 3 gaps, 7 + 7 pairs and the footer.
    assert_eq!(long.rows, 1 + 3 + 14 + 1);
    // The deleted file shows its one line on the old side.
    assert_eq!(summary(&pane, "c.txt", cx).rows, 3);
    assert_eq!(current(&pane, cx).as_deref(), Some("a/long.txt"));
    pane.read_with(cx, |pane, _| {
        assert_eq!(pane.view_mode(), ViewMode::Split);
        assert_eq!(pane.top_file(), Some("a/long.txt"));
        assert_eq!(pane.pinned_file(), None);
        assert_eq!(pane.cursor(), None);
    });
}

#[gpui::test]
fn j_and_k_show_the_next_and_previous_file_with_the_cursor_on_its_first_change(
    cx: &mut TestAppContext,
) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    press(cx, &shell, "j");
    assert_eq!(current(&pane, cx).as_deref(), Some("b.txt"));
    // b.txt's first change is the pair `two` | `2`: the cursor takes its new line.
    assert_eq!(cursor(&pane, cx), new_line("b.txt", 2));
    pane.read_with(cx, |pane, _| assert_eq!(pane.top_file(), Some("b.txt")));
    // The panel follows.
    let list_current = shell.read_with(cx, |shell, cx| {
        shell
            .diff_pane()
            .read(cx)
            .current_file()
            .map(str::to_string)
    });
    assert_eq!(list_current.as_deref(), Some("b.txt"));
    // A deleted file has only an old side.
    press(cx, &shell, "j");
    assert_eq!(
        cursor(&pane, cx),
        Some(("c.txt".to_string(), AnchorSide::Old, 1))
    );
    // The last file stays put; K goes back.
    press(cx, &shell, "j");
    assert_eq!(current(&pane, cx).as_deref(), Some("c.txt"));
    press(cx, &shell, "k k");
    assert_eq!(current(&pane, cx).as_deref(), Some("a/long.txt"));
    assert_eq!(cursor(&pane, cx), new_line("a/long.txt", 10));
}

#[gpui::test]
fn arrows_move_the_cursor_and_shift_extends_a_selection_on_one_side(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    // Without a cursor, ↓ starts on the first line in view (split view prefers the new side).
    press(cx, &shell, "down");
    assert_eq!(cursor(&pane, cx), new_line("a/long.txt", 7));
    press(cx, &shell, "down down down");
    assert_eq!(cursor(&pane, cx), new_line("a/long.txt", 10));
    // ← moves to the old side of the same row: the removed `line 10`.
    press(cx, &shell, "left");
    assert_eq!(
        cursor(&pane, cx),
        Some(("a/long.txt".to_string(), AnchorSide::Old, 10))
    );
    press(cx, &shell, "right");
    // Shift ↓ selects from the cursor; the selection keeps its side.
    press(cx, &shell, "shift-down shift-down");
    let selection = pane.read_with(cx, |pane, _| pane.selection());
    assert_eq!(
        selection,
        Some(SelectionRef {
            path: "a/long.txt".to_string(),
            side: AnchorSide::New,
            start_line: 10,
            end_line: 12,
        })
    );
    // Shift ↑ shrinks it again, past the anchor it grows the other way.
    press(cx, &shell, "shift-up shift-up shift-up");
    let selection = pane.read_with(cx, |pane, _| pane.selection()).unwrap();
    assert_eq!((selection.start_line, selection.end_line), (9, 10));
    // Esc clears the selection and keeps the cursor; a plain arrow does too.
    press(cx, &shell, "escape");
    assert_eq!(pane.read_with(cx, |pane, _| pane.selection()), None);
    assert_eq!(cursor(&pane, cx), new_line("a/long.txt", 9));
    // The cursor crosses into the next file after this one's last shown line (new 73).
    pane.update(cx, |pane, cx| {
        let pos = LinePos {
            line: 74,
            side: AnchorSide::New,
        };
        pane.click_line("a/long.txt", pos, false, cx);
    });
    assert_eq!(cursor(&pane, cx), new_line("a/long.txt", 73));
    press(cx, &shell, "down");
    assert_eq!(cursor(&pane, cx), new_line("b.txt", 1));
    assert_eq!(current(&pane, cx).as_deref(), Some("b.txt"));
}

#[gpui::test]
fn the_cursor_keeps_its_side_through_a_file_without_lines_on_it(cx: &mut TestAppContext) {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a1\na2\n");
    repo.write("b.txt", "gone\n");
    repo.write("c.txt", "c1\nc2\n");
    repo.write("e.txt", "e1\n");
    repo.commit("base");
    repo.write("a.txt", "a1\nA2\n");
    repo.remove("b.txt");
    repo.write("c.txt", "c1\nC2\n");
    repo.write("d.txt", "added\n");
    repo.write("e.txt", "E1\n");
    let (_dir, shell, cx) = open_target(&repo, Target::Working, 5, cx);
    let pane = pane(&shell, cx);
    let at = |path: &str, side, line| Some((path.to_string(), side, line));
    press(cx, &shell, "down down");
    assert_eq!(cursor(&pane, cx), at("a.txt", AnchorSide::New, 2));
    // The deleted file has only an old side, so the cursor goes there while in it...
    press(cx, &shell, "down");
    assert_eq!(cursor(&pane, cx), at("b.txt", AnchorSide::Old, 1));
    // ...and back to the new side in the next file that has one.
    press(cx, &shell, "down");
    assert_eq!(cursor(&pane, cx), at("c.txt", AnchorSide::New, 1));
    // The same the other way: chosen with ←, the old side survives an added file.
    press(cx, &shell, "left down");
    assert_eq!(cursor(&pane, cx), at("c.txt", AnchorSide::Old, 2));
    press(cx, &shell, "down");
    assert_eq!(cursor(&pane, cx), at("d.txt", AnchorSide::New, 1));
    press(cx, &shell, "down");
    assert_eq!(cursor(&pane, cx), at("e.txt", AnchorSide::Old, 1));
    // Going back up keeps it too.
    press(cx, &shell, "up");
    assert_eq!(cursor(&pane, cx), at("d.txt", AnchorSide::New, 1));
    press(cx, &shell, "up");
    assert_eq!(cursor(&pane, cx), at("c.txt", AnchorSide::Old, 2));
}

#[gpui::test]
fn a_shift_click_on_a_line_number_extends_the_selection(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    pane.update(cx, |pane, cx| {
        let at = |line| LinePos {
            line,
            side: AnchorSide::New,
        };
        pane.click_line("a/long.txt", at(7), false, cx);
        pane.click_line("a/long.txt", at(12), true, cx);
    });
    let selection = pane.read_with(cx, |pane, _| pane.selection()).unwrap();
    // Indexes 7 and 12 are new lines 8 and 12 (the removed line has no new number).
    assert_eq!((selection.start_line, selection.end_line), (8, 12));
    // A click on the other side starts over.
    pane.update(cx, |pane, cx| {
        let pos = LinePos {
            line: 9,
            side: AnchorSide::Old,
        };
        pane.click_line("a/long.txt", pos, true, cx);
    });
    assert_eq!(pane.read_with(cx, |pane, _| pane.selection()), None);
}

#[gpui::test]
fn n_and_p_move_between_change_blocks_across_files(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    press(cx, &shell, "n");
    assert_eq!(cursor(&pane, cx), new_line("a/long.txt", 10));
    press(cx, &shell, "n");
    assert_eq!(cursor(&pane, cx), new_line("a/long.txt", 70));
    press(cx, &shell, "n");
    assert_eq!(cursor(&pane, cx), new_line("b.txt", 2));
    // b.txt's addition after `three` is a block of its own.
    press(cx, &shell, "n");
    assert_eq!(cursor(&pane, cx), new_line("b.txt", 4));
    press(cx, &shell, "p p");
    assert_eq!(cursor(&pane, cx), new_line("a/long.txt", 70));
}

#[gpui::test]
fn e_expands_the_gap_nearest_the_cursor_and_shift_e_all_of_it(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    press(cx, &shell, "n");
    // From `line 10` the 53-line gap below is nearer than the 6 above: one step from its top.
    press(cx, &shell, "e");
    assert_eq!(summary(&pane, "a/long.txt", cx).gaps, [6, 33, 7]);
    assert_eq!(cursor(&pane, cx), new_line("a/long.txt", 10));
    // Now the 6-line gap above is the nearest: all of it.
    press(cx, &shell, "shift-e");
    assert_eq!(summary(&pane, "a/long.txt", cx).gaps, [33, 7]);
    // A click on a gap's "Expand all" reveals that gap.
    pane.update(cx, |pane, cx| pane.expand("a/long.txt", 0, Expand::All, cx));
    assert_eq!(summary(&pane, "a/long.txt", cx).gaps, [7]);
}

#[gpui::test]
fn s_switches_split_and_unified_and_the_review_remembers(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    press(cx, &shell, "n");
    let split_rows = summary(&pane, "a/long.txt", cx).rows;
    press(cx, &shell, "s");
    pane.read_with(cx, |pane, _| {
        assert_eq!(pane.view_mode(), ViewMode::Unified)
    });
    // Unified shows the removed and the added line apart: two rows more.
    assert_eq!(summary(&pane, "a/long.txt", cx).rows, split_rows + 2);
    // The cursor stays on its line.
    assert_eq!(cursor(&pane, cx), new_line("a/long.txt", 10));
    let (session, review) = shell.read_with(cx, |shell, cx| {
        let session = shell.session().cloned().unwrap();
        let review = session.read(cx).review_id().clone();
        (session, review)
    });
    let remembered = cx.update(|_, cx| cx.services().review_ui.read(cx).view_mode(&review));
    assert_eq!(remembered, ViewMode::Unified);
    // Shown again, the review opens unified.
    pane.update(cx, |pane, cx| pane.set_session(Some(session), cx));
    draw(cx);
    pane.read_with(cx, |pane, _| {
        assert_eq!(pane.view_mode(), ViewMode::Unified)
    });
    press(cx, &shell, "s");
    pane.read_with(cx, |pane, _| assert_eq!(pane.view_mode(), ViewMode::Split));
}

#[gpui::test]
fn v_marks_the_current_file_viewed_and_it_collapses_until_opened(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    let session = shell.read_with(cx, |shell, _| shell.session().cloned().unwrap());
    press(cx, &shell, "j v");
    settle_drawn(cx, &shell, "the viewed mark", |_, cx| {
        session.read(cx).is_viewed("b.txt")
    });
    let b = summary(&pane, "b.txt", cx);
    assert!(b.collapsed);
    assert_eq!(b.rows, 1);
    // The collapsed header stays at the top and stays current, so `J` goes on from it.
    assert_eq!(current(&pane, cx).as_deref(), Some("b.txt"));
    press(cx, &shell, "j");
    assert_eq!(current(&pane, cx).as_deref(), Some("c.txt"));
    // The chevron opens a viewed file again; `V` again un-views it.
    pane.update(cx, |pane, cx| pane.set_open("b.txt", true, cx));
    assert!(!summary(&pane, "b.txt", cx).collapsed);
    pane.update(cx, |pane, cx| pane.toggle_viewed("b.txt", cx));
    settle_drawn(cx, &shell, "the mark to go", |_, cx| {
        !session.read(cx).is_viewed("b.txt")
    });
    assert!(!summary(&pane, "b.txt", cx).collapsed);
}

#[gpui::test]
fn page_down_scrolls_a_page_and_carries_the_cursor(cx: &mut TestAppContext) {
    let repo = stream_fixture(300);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    // Every line of the long file shown: well over a page.
    press(cx, &shell, "n shift-e");
    pane.update(cx, |pane, cx| {
        pane.expand("a/long.txt", 0, Expand::All, cx);
        pane.expand("a/long.txt", 0, Expand::All, cx);
    });
    draw(cx);
    assert!(summary(&pane, "a/long.txt", cx).gaps.is_empty());
    let before = cursor(&pane, cx).unwrap().2;
    press(cx, &shell, "pagedown");
    let after = cursor(&pane, cx).unwrap().2;
    // A page is the viewport less the pinned header and a line: about 37 lines at 832px.
    assert!((30..=45).contains(&(after - before)), "{before} → {after}");
    assert_eq!(
        pane.read_with(cx, |pane, _| pane.pinned_file().map(str::to_string)),
        Some("a/long.txt".to_string())
    );
    press(cx, &shell, "pageup");
    assert_eq!(cursor(&pane, cx).unwrap().2, before);
}

#[gpui::test]
fn single_letter_keys_stay_out_of_text_fields(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    let list = shell.read_with(cx, |shell, _| shell.file_list().clone());
    let field = list.read_with(cx, |list, cx| list.filter_focus(cx));
    cx.update(|window, cx| field.focus(window, cx));
    cx.simulate_input("j");
    settle_drawn(cx, &shell, "the filter", |_, _| true);
    assert_eq!(current(&pane, cx).as_deref(), Some("a/long.txt"));
    list.read_with(cx, |list, _| assert_eq!(list.query(), "j"));
    // Esc hands the keyboard back to the stream, whose keys then work.
    press(cx, &shell, "escape j");
    assert_eq!(current(&pane, cx).as_deref(), Some("b.txt"));
}

#[gpui::test]
fn the_sideways_scroll_stops_at_the_widest_line_in_view(cx: &mut TestAppContext) {
    let repo = FixtureRepo::new();
    repo.write("wide.txt", "short\n");
    repo.commit("base");
    repo.write("wide.txt", format!("short\n{}\n", "x".repeat(400)));
    repo.write("narrow.txt", "a\n");
    let (_dir, shell, cx) = open_two(&repo, cx);
    let pane = pane(&shell, cx);
    pane.update(cx, |pane, cx| pane.scroll_sideways(100_000., cx));
    let (offset, width, columns) = pane.read_with(cx, |pane, _| {
        (pane.h_offset(), pane.char_width.unwrap(), pane.code_columns)
    });
    let max = (400. - columns as f32 + 2.) * width;
    assert!((offset - max).abs() < 0.5, "{offset} vs {max}");
    pane.update(cx, |pane, cx| pane.scroll_sideways(-1_000_000., cx));
    pane.read_with(cx, |pane, _| assert_eq!(pane.h_offset(), 0.));
}

/// [`open`] for a repo with two changed files.
fn open_two<'a>(
    repo: &FixtureRepo,
    cx: &'a mut TestAppContext,
) -> (tempfile::TempDir, Entity<Shell>, &'a mut VisualTestContext) {
    open_target(repo, Target::Working, 2, cx)
}

/// A shell showing `target`'s review of `repo`, with `files` changed files, drawn and settled.
fn open_target<'a>(
    repo: &FixtureRepo,
    target: Target,
    files: usize,
    cx: &'a mut TestAppContext,
) -> (tempfile::TempDir, Entity<Shell>, &'a mut VisualTestContext) {
    let (dir, shell, cx) = open_shell(cx);
    add_repo(repo, &shell, cx);
    let task = cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open(
                Location::Local(repo.path().to_path_buf()),
                OpenRequest::Target(target),
                window,
                cx,
            )
        })
    });
    cx.run_until_parked();
    let pane = shell.read_with(cx, |shell, _| shell.diff_pane().clone());
    settle_drawn(cx, &shell, "the files to load", |_, cx| {
        pane.read(cx).blocks.len() == files && pane.read(cx).wanted.len() == files
    });
    cx.foreground_executor()
        .block_test(task)
        .expect("the review opens");
    (dir, shell, cx)
}

#[gpui::test]
fn submodules_and_conflicts_show_their_notices(cx: &mut TestAppContext) {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "base\n");
    repo.commit("base");
    repo.branch("other");
    repo.write("a.txt", "main side\n");
    repo.commit("main");
    repo.checkout("other");
    repo.write("a.txt", "other side\n");
    repo.commit("other");
    repo.checkout("main");
    // The merge stops on the conflict, leaving `a.txt` unmerged in the index.
    let merged = repo
        .git_command()
        .args(["merge", "-q", "other"])
        .output()
        .expect("git runs");
    assert!(!merged.status.success(), "the merge conflicts");
    repo.stage(
        "160000",
        "1111111111111111111111111111111111111111",
        "vendor/lib",
    );
    let (_dir, shell, cx) = open_target(&repo, Target::Staged, 2, cx);
    let pane = pane(&shell, cx);
    assert_eq!(summary(&pane, "a.txt", cx).notice, Some("unmerged"));
    assert_eq!(summary(&pane, "vendor/lib", cx).notice, Some("submodule"));
    // Without lines there are no side labels, so the header is the bar alone.
    pane.read_with(cx, |pane, _| {
        assert_eq!(
            pane.blocks[0].rows[0],
            Row::Header {
                collapsed: false,
                labels: false
            }
        );
    });
    // `J` steps over them without a cursor.
    press(cx, &shell, "j");
    assert_eq!(current(&pane, cx).as_deref(), Some("vendor/lib"));
    assert_eq!(cursor(&pane, cx), None);
}

#[gpui::test]
fn shift_arrows_open_a_gap_rather_than_select_across_it(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    let at = |line| LinePos {
        line,
        side: AnchorSide::New,
    };
    // The cursor on new 13, the last line shown above the 53-line gap.
    pane.update(cx, |pane, cx| {
        pane.click_line("a/long.txt", at(13), false, cx)
    });
    press(cx, &shell, "shift-down");
    let selection = pane.read_with(cx, |pane, _| pane.selection()).unwrap();
    assert_eq!(
        (selection.start_line, selection.end_line),
        (13, 14),
        "the selection takes in only the line the gap opened onto"
    );
    assert_eq!(summary(&pane, "a/long.txt", cx).gaps, [6, 33, 7]);
    // Upwards from new 7, the first line below the file's leading gap: it opens from its
    // bottom (all of its 6 lines are within a step).
    pane.update(cx, |pane, cx| {
        pane.click_line("a/long.txt", at(6), false, cx)
    });
    press(cx, &shell, "shift-up");
    let selection = pane.read_with(cx, |pane, _| pane.selection()).unwrap();
    assert_eq!((selection.start_line, selection.end_line), (6, 7));
    assert_eq!(summary(&pane, "a/long.txt", cx).gaps, [33, 7]);
    // A plain arrow still steps over a gap.
    pane.update(cx, |pane, cx| {
        pane.click_line("a/long.txt", at(33), false, cx)
    });
    press(cx, &shell, "down");
    assert_eq!(cursor(&pane, cx), new_line("a/long.txt", 67));
}

#[gpui::test]
fn a_file_loaded_again_with_the_same_lines_keeps_its_gaps_and_the_place(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    // Every gap of a/long.txt open, and the reader well inside it.
    press(cx, &shell, "n shift-e shift-e shift-e");
    assert!(summary(&pane, "a/long.txt", cx).gaps.is_empty());
    press(cx, &shell, "pagedown");
    let before = (
        cursor(&pane, cx),
        pane.read_with(cx, |pane, _| pane.list.logical_scroll_top()),
    );
    assert_ne!(before.1.item_ix, 0, "scrolled into the file");
    // The agent stages it (or a rescan): the same lines load again.
    let session = shell.read_with(cx, |shell, _| shell.session().cloned().unwrap());
    session.update(cx, |session, cx| session.reload_file("a/long.txt", cx));
    settle_drawn(cx, &shell, "the reload", |_, _| true);
    assert!(summary(&pane, "a/long.txt", cx).gaps.is_empty());
    let after = (
        cursor(&pane, cx),
        pane.read_with(cx, |pane, _| pane.list.logical_scroll_top()),
    );
    assert_eq!(after.0, before.0);
    assert_eq!(after.1.item_ix, before.1.item_ix);
}

#[gpui::test]
fn a_file_arriving_above_the_start_shows_when_the_reader_hadnt_moved(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    let blocks = |count: usize, cx: &mut VisualTestContext| {
        let watched = pane.clone();
        settle_drawn(cx, &shell, "the new file", move |_, cx| {
            watched.read(cx).blocks.len() == count
        });
    };
    // The stream at its start, untouched: a file the agent adds that sorts first (as a later
    // batch of its edit lists it) is what the reader sees first.
    pane.read_with(cx, |pane, _| {
        assert_eq!(pane.top_file(), Some("a/long.txt"));
        assert_eq!(pane.cursor(), None);
    });
    repo.write("0.txt", "new\n");
    blocks(4, cx);
    pane.read_with(cx, |pane, _| {
        assert_eq!(pane.top_file(), Some("0.txt"));
        assert_eq!(pane.list.logical_scroll_top().item_ix, 0);
    });

    // A reader who moved keeps their place: J to b.txt, then another file above it.
    press(cx, &shell, "j j");
    pane.read_with(cx, |pane, _| assert_eq!(pane.top_file(), Some("b.txt")));
    repo.write("00.txt", "newer\n");
    blocks(5, cx);
    pane.read_with(cx, |pane, _| assert_eq!(pane.top_file(), Some("b.txt")));
}

/// The text of the line at the top of the viewport (its new side in split view).
fn top_line(pane: &Entity<DiffPane>, cx: &mut VisualTestContext) -> Option<String> {
    pane.read_with(cx, |pane, _| {
        let top = pane.list.logical_scroll_top();
        let (block, row) = pane.locate(top.item_ix)?;
        let path = &pane.blocks[block].path;
        let line = match pane.blocks[block].rows[row] {
            Row::Pair { old, new } => new.or(old)?,
            Row::Line(line) => line,
            _ => return None,
        };
        Some(pane.lines(path)?[line].text.clone())
    })
}

/// `stream_fixture(80)`'s edited `a/long.txt`, with `edit` applied to its lines.
fn long_file(edit: impl Fn(&mut Vec<String>)) -> String {
    let mut lines: Vec<String> = (1..=80)
        .map(|i| match i {
            10 => "line ten".to_string(),
            70 => "line seventy".to_string(),
            i => format!("line {i}"),
        })
        .collect();
    edit(&mut lines);
    lines.join("\n") + "\n"
}

#[gpui::test]
fn an_agent_edit_keeps_the_readers_lines_cursor_and_opened_context(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    // Every gap of a/long.txt open, and the reader well inside it.
    press(cx, &shell, "n shift-e shift-e shift-e pagedown");
    assert!(summary(&pane, "a/long.txt", cx).gaps.is_empty());
    let (_, _, line) = cursor(&pane, cx).expect("a cursor");
    let top = top_line(&pane, cx).expect("a line at the top");
    let wait_for = |text: &'static str, cx: &mut VisualTestContext| {
        let watched = pane.clone();
        settle_drawn(cx, &shell, text, move |_, cx| {
            watched
                .read(cx)
                .lines("a/long.txt")
                .is_some_and(|lines| lines.iter().any(|line| line.text == text))
        });
    };

    // The agent adds a line at the top: everything moves down one, and nothing the reviewer
    // opened closes again.
    repo.write(
        "a/long.txt",
        long_file(|lines| lines.insert(0, "inserted".to_string())),
    );
    wait_for("inserted", cx);
    assert!(summary(&pane, "a/long.txt", cx).gaps.is_empty());
    assert_eq!(cursor(&pane, cx), new_line("a/long.txt", line + 1));
    assert_eq!(top_line(&pane, cx).as_deref(), Some(top.as_str()));

    // It changes a word far above without changing the line count: nothing moves.
    repo.write(
        "a/long.txt",
        long_file(|lines| {
            lines.insert(0, "inserted".to_string());
            lines[2] = "line two".to_string();
        }),
    );
    wait_for("line two", cx);
    assert!(summary(&pane, "a/long.txt", cx).gaps.is_empty());
    assert_eq!(cursor(&pane, cx), new_line("a/long.txt", line + 1));
    assert_eq!(top_line(&pane, cx).as_deref(), Some(top.as_str()));
}

#[gpui::test]
fn an_agent_edit_above_keeps_the_cursor_on_its_line_without_opened_context(
    cx: &mut TestAppContext,
) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    // The second change of a/long.txt, nothing expanded; the cursor on it.
    press(cx, &shell, "n n");
    assert_eq!(cursor(&pane, cx), new_line("a/long.txt", 70));
    let top = top_line(&pane, cx);
    // A line inserted at the top: the cursor stays on `line seventy`, the view on its lines.
    repo.write(
        "a/long.txt",
        long_file(|lines| lines.insert(0, "inserted".to_string())),
    );
    let watched = pane.clone();
    settle_drawn(cx, &shell, "the reload", move |_, cx| {
        watched
            .read(cx)
            .lines("a/long.txt")
            .is_some_and(|lines| lines.iter().any(|line| line.text == "inserted"))
    });
    assert_eq!(cursor(&pane, cx), new_line("a/long.txt", 71));
    assert_eq!(top_line(&pane, cx), top);
}
