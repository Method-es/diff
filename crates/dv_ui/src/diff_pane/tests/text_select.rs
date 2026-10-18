//! Text in code with the mouse (PLAN §18.1, DESIGN §4.1): a drag, a double- and a
//! triple-click, a shift-click, the caret, `Ctrl C`, and the selection keeping its text through
//! scrolling, a gap opening and the view mode.

use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Point, point, px};

use pretty_assertions::assert_eq;

use super::*;
use crate::diff_pane::render::code::TEXT_INSET;
use crate::system::SystemRequest;

/// `a.txt` with a tab, a multi-byte character and a gap (lines 6–25 unchanged).
fn text_fixture() -> FixtureRepo {
    let repo = FixtureRepo::new();
    let mut base = String::from("alpha beta\n\tgamma_delta = 1;\nzeta é\nomega\n");
    for i in 5..=30 {
        base.push_str(&format!("filler {i}\n"));
    }
    repo.write("a.txt", &base);
    repo.write("b.txt", "one\n");
    repo.commit("base");
    let edited = base
        .replace("\tgamma_delta = 1;", "\tgamma_delta = 2;")
        .replace("filler 30\n", "filler thirty\n");
    repo.write("a.txt", &edited);
    repo.write("b.txt", "one\ntwo\n");
    repo
}

fn open_text<'a>(
    repo: &FixtureRepo,
    cx: &'a mut TestAppContext,
) -> (tempfile::TempDir, Entity<Shell>, &'a mut VisualTestContext) {
    super::open_target(repo, Target::Working, 2, cx)
}

/// Where the middle of character `column` (1-based, painted columns: a tab counts four) of
/// `path`'s line `number` on `side` drew, after a fresh frame.
fn char_point(
    pane: &Entity<DiffPane>,
    path: &str,
    side: &str,
    number: u32,
    column: usize,
    cx: &mut VisualTestContext,
) -> Point<Pixels> {
    let name = format!("code/{path}/{side}/{number}");
    let cell = bounds(cx, Box::leak(name.into_boxed_str())).expect("the line's text drew");
    let (char_width, h_offset) = pane.read_with(cx, |pane, _| {
        (pane.char_width.expect("measured"), pane.h_offset)
    });
    let x = f32::from(cell.origin.x) + TEXT_INSET + (column as f32 - 0.5) * char_width - h_offset;
    point(px(x), cell.center().y)
}

fn press_at(cx: &mut VisualTestContext, at: Point<Pixels>, clicks: usize, shift: bool) {
    let modifiers = Modifiers {
        shift,
        ..Modifiers::none()
    };
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: at,
        modifiers,
        click_count: clicks,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: at,
        modifiers,
        click_count: clicks,
    });
    cx.run_until_parked();
}

fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    draw(cx);
    let steps = 4;
    for step in 1..=steps {
        let t = step as f32 / steps as f32;
        let at = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
        cx.simulate_mouse_move(at, MouseButton::Left, Modifiers::none());
        draw(cx);
    }
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    draw(cx);
}

fn selected_text(pane: &Entity<DiffPane>, cx: &mut VisualTestContext) -> Option<String> {
    pane.read_with(cx, |pane, _| pane.text_selection().map(|s| s.text))
}

fn clipboard(cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|_, cx| {
        crate::system::requests(cx)
            .0
            .into_iter()
            .filter_map(|request| match request {
                SystemRequest::Clipboard(text) => Some(text),
                _ => None,
            })
            .collect()
    })
}

#[gpui::test]
fn a_drag_selects_characters_across_lines_and_ctrl_c_copies_the_source_text(
    cx: &mut TestAppContext,
) {
    let repo = text_fixture();
    let (_dir, shell, cx) = open_text(&repo, cx);
    let pane = pane(&shell, cx);
    // From `beta` on line 1 to before ` = 2` on line 2 (the tab paints columns 1–4).
    let from = char_point(&pane, "a.txt", "new", 1, 7, cx);
    let to = char_point(&pane, "a.txt", "new", 2, 16, cx);
    drag(cx, from, to);
    let selection = pane
        .read_with(cx, |pane, _| pane.text_selection())
        .expect("text is selected");
    assert_eq!(selection.text, "beta\n\tgamma_delta");
    assert_eq!(selection.side, Some(AnchorSide::New));
    assert_eq!((selection.from.line, selection.from.column), (1, 7));
    // The place after the last selected character: column 13 of the source line.
    assert_eq!((selection.to.line, selection.to.column), (2, 13));
    // The cursor and the caret follow the drag's end.
    assert_eq!(cursor(&pane, cx), new_line("a.txt", 2));
    let caret = pane.read_with(cx, |pane, _| pane.caret()).unwrap();
    assert_eq!((caret.1.line, caret.1.column), (2, 13));
    press(cx, &shell, "secondary-c");
    assert_eq!(clipboard(cx), ["beta\n\tgamma_delta"]);
    // Scrolled sideways and with a gap opened, it keeps its text.
    pane.update(cx, |pane, cx| {
        pane.view_columns = 200;
        pane.scroll_sideways(30., cx);
        pane.expand("a.txt", 0, Expand::All, cx);
    });
    draw(cx);
    assert_eq!(
        selected_text(&pane, cx).as_deref(),
        Some("beta\n\tgamma_delta")
    );
    // Esc clears it; the caret stays.
    press(cx, &shell, "escape");
    assert_eq!(selected_text(&pane, cx), None);
    assert!(pane.read_with(cx, |pane, _| pane.caret()).is_some());
}

#[gpui::test]
fn a_double_click_takes_a_word_a_triple_click_the_line_and_shift_extends(cx: &mut TestAppContext) {
    let repo = text_fixture();
    let (_dir, shell, cx) = open_text(&repo, cx);
    let pane = pane(&shell, cx);
    // `gamma_delta` is one word (painted columns 5–15).
    let at = char_point(&pane, "a.txt", "new", 2, 8, cx);
    press_at(cx, at, 1, false);
    press_at(cx, at, 2, false);
    assert_eq!(selected_text(&pane, cx).as_deref(), Some("gamma_delta"));
    press_at(cx, at, 3, false);
    assert_eq!(
        selected_text(&pane, cx).as_deref(),
        Some("\tgamma_delta = 2;")
    );
    // A click puts the caret, before the character under the pointer; `é` is one column.
    let zeta = char_point(&pane, "a.txt", "new", 3, 6, cx);
    press_at(cx, zeta, 1, false);
    assert_eq!(selected_text(&pane, cx), None);
    let caret = pane.read_with(cx, |pane, _| pane.caret()).unwrap();
    assert_eq!(caret.0, "a.txt");
    assert_eq!(
        (caret.1.side, caret.1.line, caret.1.column),
        (AnchorSide::New, 3, 6)
    );
    assert_eq!(cursor(&pane, cx), new_line("a.txt", 3));
    // Shift-click extends from the caret.
    let omega = char_point(&pane, "a.txt", "new", 4, 3, cx);
    press_at(cx, omega, 1, true);
    assert_eq!(selected_text(&pane, cx).as_deref(), Some("é\nom"));
    // A plain click elsewhere clears it; a keyboard move clears the caret.
    let one = char_point(&pane, "a.txt", "new", 1, 1, cx);
    press_at(cx, one, 1, false);
    assert_eq!(selected_text(&pane, cx), None);
    press(cx, &shell, "down");
    assert_eq!(pane.read_with(cx, |pane, _| pane.caret()), None);
}

#[gpui::test]
fn dragging_past_the_file_clamps_to_its_last_line(cx: &mut TestAppContext) {
    let repo = text_fixture();
    let (_dir, shell, cx) = open_text(&repo, cx);
    let pane = pane(&shell, cx);
    let from = char_point(&pane, "a.txt", "new", 30, 8, cx);
    // Down into the next file's rows: the selection stops at the end of a.txt's last line.
    let below = char_point(&pane, "b.txt", "new", 2, 2, cx);
    drag(cx, from, below);
    let selection = pane.read_with(cx, |pane, _| pane.text_selection()).unwrap();
    assert_eq!(selection.path, "a.txt");
    assert!(selection.text.starts_with("thirty"), "{:?}", selection.text);
    assert!(!selection.text.contains("two"));
    // A drag on the old side keeps to the old side.
    let old = char_point(&pane, "a.txt", "old", 2, 5, cx);
    let old_to = char_point(&pane, "a.txt", "old", 3, 3, cx);
    drag(cx, old, old_to);
    let selection = pane.read_with(cx, |pane, _| pane.text_selection()).unwrap();
    assert_eq!(selection.side, Some(AnchorSide::Old));
    assert_eq!(selection.text, "gamma_delta = 1;\nze");
}

#[gpui::test]
fn unified_view_copies_removed_and_added_rows_in_order(cx: &mut TestAppContext) {
    let repo = text_fixture();
    let (_dir, shell, cx) = open_text(&repo, cx);
    let pane = pane(&shell, cx);
    press(cx, &shell, "s");
    assert_eq!(
        pane.read_with(cx, |pane, _| pane.view_mode()),
        ViewMode::Unified
    );
    // From line 1 (new) through the removed old line 2 to the added new line 2.
    let from = char_point(&pane, "a.txt", "new", 1, 1, cx);
    let to = char_point(&pane, "a.txt", "new", 2, 6, cx);
    drag(cx, from, to);
    let selection = pane.read_with(cx, |pane, _| pane.text_selection()).unwrap();
    assert_eq!(selection.side, None);
    assert_eq!(selection.text, "alpha beta\n\tgamma_delta = 1;\n\tg");
    // Switching back to split view clears it.
    press(cx, &shell, "s");
    assert_eq!(selected_text(&pane, cx), None);
}

#[gpui::test]
fn ctrl_c_copies_a_line_selection_and_nothing_without_one(cx: &mut TestAppContext) {
    let repo = text_fixture();
    let (_dir, shell, cx) = open_text(&repo, cx);
    let pane = pane(&shell, cx);
    press(cx, &shell, "secondary-c");
    assert!(clipboard(cx).is_empty());
    pane.update(cx, |pane, cx| {
        let at = |line| LinePos {
            line,
            side: AnchorSide::New,
        };
        // Index 2 is new line 2 (the removed old line 2 comes first), index 3 new line 3.
        pane.click_line("a.txt", at(2), false, cx);
        pane.click_line("a.txt", at(3), true, cx);
    });
    press(cx, &shell, "secondary-c");
    assert_eq!(clipboard(cx), ["\tgamma_delta = 2;\nzeta é"]);
}

#[gpui::test]
fn the_menus_copy_copies_the_streams_selection_too(cx: &mut TestAppContext) {
    // macOS's Edit › Copy sends the fields' copy wherever the keyboard is (`crate::app_menu`):
    // from the stream it copies the stream's selection, as `Ctrl C` there does.
    let repo = text_fixture();
    let (_dir, shell, cx) = open_text(&repo, cx);
    let pane = pane(&shell, cx);
    cx.dispatch_action(crate::actions::field::Copy);
    assert!(clipboard(cx).is_empty());
    pane.update(cx, |pane, cx| {
        let at = |line| LinePos {
            line,
            side: AnchorSide::New,
        };
        pane.click_line("a.txt", at(2), false, cx);
        pane.click_line("a.txt", at(3), true, cx);
    });
    cx.dispatch_action(crate::actions::field::Copy);
    assert_eq!(clipboard(cx), ["\tgamma_delta = 2;\nzeta é"]);
    // A drag's text the same way.
    let from = char_point(&pane, "a.txt", "new", 1, 1, cx);
    let to = char_point(&pane, "a.txt", "new", 2, 6, cx);
    drag(cx, from, to);
    let dragged = selected_text(&pane, cx).unwrap();
    cx.dispatch_action(crate::actions::field::Copy);
    assert_eq!(clipboard(cx).last(), Some(&dragged));
}

#[gpui::test]
fn a_drag_after_a_double_click_extends_by_words_and_after_a_triple_click_by_lines(
    cx: &mut TestAppContext,
) {
    let repo = text_fixture();
    let (_dir, shell, cx) = open_text(&repo, cx);
    let pane = pane(&shell, cx);
    // Double-click `beta`, then drag into `gamma_delta` on the next line: whole words.
    let beta = char_point(&pane, "a.txt", "new", 1, 8, cx);
    let gamma = char_point(&pane, "a.txt", "new", 2, 7, cx);
    press_at(cx, beta, 1, false);
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: beta,
        modifiers: Modifiers::none(),
        click_count: 2,
        first_mouse: false,
    });
    draw(cx);
    cx.simulate_mouse_move(gamma, MouseButton::Left, Modifiers::none());
    draw(cx);
    assert_eq!(
        selected_text(&pane, cx).as_deref(),
        Some("beta\n\tgamma_delta")
    );
    // Back before the word it started on: from that word's end.
    let alpha = char_point(&pane, "a.txt", "new", 1, 2, cx);
    cx.simulate_mouse_move(alpha, MouseButton::Left, Modifiers::none());
    draw(cx);
    assert_eq!(selected_text(&pane, cx).as_deref(), Some("alpha beta"));
    cx.simulate_mouse_up(alpha, MouseButton::Left, Modifiers::none());
    draw(cx);
    // Triple-click line 2, then drag to line 3: whole lines.
    let zeta = char_point(&pane, "a.txt", "new", 3, 2, cx);
    press_at(cx, gamma, 1, false);
    press_at(cx, gamma, 2, false);
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: gamma,
        modifiers: Modifiers::none(),
        click_count: 3,
        first_mouse: false,
    });
    draw(cx);
    cx.simulate_mouse_move(zeta, MouseButton::Left, Modifiers::none());
    draw(cx);
    assert_eq!(
        selected_text(&pane, cx).as_deref(),
        Some("\tgamma_delta = 2;\nzeta é")
    );
    cx.simulate_mouse_up(zeta, MouseButton::Left, Modifiers::none());
    draw(cx);
    // A press and release in one frame ends the drag all the same.
    press_at(cx, zeta, 1, false);
    assert!(pane.read_with(cx, |pane, _| pane.text_drag.is_none()));
}

#[gpui::test]
fn the_agent_editing_the_file_clears_its_selection_and_caret(cx: &mut TestAppContext) {
    let repo = text_fixture();
    let (_dir, shell, cx) = open_text(&repo, cx);
    let pane = pane(&shell, cx);
    let at = char_point(&pane, "a.txt", "new", 2, 8, cx);
    press_at(cx, at, 1, false);
    press_at(cx, at, 2, false);
    assert_eq!(selected_text(&pane, cx).as_deref(), Some("gamma_delta"));
    let session = shell.read_with(cx, |shell, _| shell.session().cloned().unwrap());
    // The same lines again (a rescan, the agent staging): the selection stays.
    session.update(cx, |session, cx| session.reload_file("a.txt", cx));
    settle_drawn(cx, &shell, "the reload", |_, _| true);
    assert_eq!(selected_text(&pane, cx).as_deref(), Some("gamma_delta"));
    // Other lines: the line numbers it named are other text now.
    repo.write("a.txt", "alpha beta\n\tgamma = 3;\nzeta é\nomega\n");
    session.update(cx, |session, cx| session.reload_file("a.txt", cx));
    settle_drawn(cx, &shell, "the edit", |_, _| true);
    assert_eq!(selected_text(&pane, cx), None);
    assert_eq!(pane.read_with(cx, |pane, _| pane.caret()), None);
}

#[gpui::test]
fn copy_and_cut_in_a_text_field_go_through_the_clipboard_seam(cx: &mut TestAppContext) {
    let repo = text_fixture();
    let (_dir, shell, cx) = open_text(&repo, cx);
    // The composer's field.
    press(cx, &shell, "down c");
    cx.simulate_input("a draft");
    press(cx, &shell, "secondary-a secondary-c");
    assert_eq!(clipboard(cx), ["a draft"]);
    press(cx, &shell, "secondary-a secondary-x");
    assert_eq!(clipboard(cx), ["a draft", "a draft"]);
    let pane = pane(&shell, cx);
    assert_eq!(
        pane.read_with(cx, |pane, cx| pane.composer_text(cx))
            .as_deref(),
        Some("")
    );
    // The find field's too.
    press(cx, &shell, "escape secondary-f");
    cx.simulate_input("omega");
    press(cx, &shell, "secondary-a secondary-c");
    assert_eq!(clipboard(cx), ["a draft", "a draft", "omega"]);
}

#[gpui::test]
fn esc_in_the_code_keeps_a_draft_and_only_esc_in_the_composer_discards_it(cx: &mut TestAppContext) {
    let repo = text_fixture();
    let (_dir, shell, cx) = open_text(&repo, cx);
    let pane = pane(&shell, cx);
    let draft = |cx: &mut VisualTestContext| pane.read_with(cx, |pane, cx| pane.composer_text(cx));
    // A comment on the cursor's line, then a press in the code to quote some of it.
    press(cx, &shell, "c");
    cx.simulate_input("a draft");
    let from = char_point(&pane, "a.txt", "new", 1, 1, cx);
    let to = char_point(&pane, "a.txt", "new", 1, 5, cx);
    drag(cx, from, to);
    press(cx, &shell, "secondary-c");
    assert_eq!(clipboard(cx), ["alph"]);
    // Esc in the stream clears the selection, then does nothing more: the draft stays.
    press(cx, &shell, "escape");
    assert_eq!(selected_text(&pane, cx), None);
    assert_eq!(draft(cx).as_deref(), Some("a draft"));
    press(cx, &shell, "escape");
    assert_eq!(draft(cx).as_deref(), Some("a draft"));
    // A language server's details over the composer close first, the draft staying.
    // (`C` with a draft open gives it the keyboard.)
    press(cx, &shell, "c");
    shell.update(cx, |shell, cx| {
        shell.toggle_details(
            &crate::shell::intel::ServerDetails::Missing("typescript"),
            cx,
        )
    });
    press(cx, &shell, "escape");
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.server_details().cloned()),
        None
    );
    assert_eq!(draft(cx).as_deref(), Some("a draft"));
    // Esc in the composer itself discards it.
    press(cx, &shell, "escape");
    assert_eq!(draft(cx), None);
    // An empty composer still closes from the stream.
    press(cx, &shell, "c");
    pane.update_in(cx, |pane, window, cx| {
        pane.focus_handle(cx).focus(window, cx)
    });
    press(cx, &shell, "escape");
    assert_eq!(draft(cx), None);
}
