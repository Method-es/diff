//! Long lines wrapping (DESIGN §4.1 "Wrap", `wrap`): the setting turns it on and off, a row
//! grows to its longer side's visual lines, the pointer selects text on any of them, and the
//! sideways scroll stays put while it is on.

use gpui::{Modifiers, MouseButton, Point, point, px};

use pretty_assertions::assert_eq;

use super::*;
use crate::diff_pane::rows::heights;

/// `wrap.txt`: its second line, `old`, replaced by a long one (`tail` at its end), which split
/// view pairs with the short removed line.
fn wrap_fixture() -> FixtureRepo {
    let repo = FixtureRepo::new();
    repo.write("wrap.txt", "first\nold\nlast\n");
    repo.commit("base");
    let words: Vec<String> = (1..=60).map(|i| format!("word{i}")).collect();
    let long = format!("start {} tail", words.join(" "));
    repo.write("wrap.txt", format!("first\n{long}\nlast\n"));
    repo
}

fn set_wrap(cx: &mut VisualTestContext, wrap: bool) {
    let settings = cx.update(|_, cx| cx.services().settings.clone());
    settings.update(cx, |settings, cx| settings.set_wrap(wrap, cx));
    draw(cx);
    draw(cx);
}

/// The window point in the middle of character `column` (0-based in its visual line) of the
/// visual line `segment` of `path`'s line (an index) `line` on `side`, as last drawn.
fn segment_point(
    pane: &Entity<DiffPane>,
    key: (&str, AnchorSide, usize),
    segment: usize,
    column: usize,
    cx: &mut VisualTestContext,
) -> Point<Pixels> {
    pane.read_with(cx, |pane, _| {
        let geometry = pane.geometry.borrow();
        let drawn = geometry
            .get(&(key.0.to_string(), key.1, key.2))
            .expect("the line drew");
        let slice = &drawn.segments[segment];
        let char_width = pane.char_width.expect("measured");
        point(
            slice.origin_x + px((column as f32 + 0.5) * char_width),
            slice.bounds.center().y,
        )
    })
}

/// The index of `path`'s line `number` on the new side.
fn new_index(
    pane: &Entity<DiffPane>,
    path: &str,
    number: u32,
    cx: &mut VisualTestContext,
) -> usize {
    pane.read_with(cx, |pane, _| {
        select::line_index(pane.lines(path).unwrap(), AnchorSide::New, number).unwrap()
    })
}

#[gpui::test]
fn the_setting_wraps_a_long_line_into_a_taller_row_and_back(cx: &mut TestAppContext) {
    let repo = wrap_fixture();
    let (_dir, shell, cx) = open_target(&repo, Target::Working, 1, cx);
    let pane = pane(&shell, cx);
    let long = new_index(&pane, "wrap.txt", 2, cx);
    let height = |cx: &mut VisualTestContext| {
        f32::from(
            bounds(cx, "code/wrap.txt/new/2")
                .expect("the long line drew")
                .size
                .height,
        )
    };
    assert_eq!(height(cx), heights::CODE);
    // Scrolled sideways before, wrapping puts the code back at its start.
    pane.update(cx, |pane, cx| pane.scroll_sideways(200., cx));
    assert!(pane.read_with(cx, |pane, _| pane.h_offset) > 0.);

    set_wrap(cx, true);
    let segments = pane.read_with(cx, |pane, _| {
        pane.segments("wrap.txt", AnchorSide::New, long)
            .expect("lines wrap")
            .len()
    });
    assert!(segments >= 3, "{segments} visual lines");
    assert_eq!(height(cx), heights::CODE * segments as f32);
    assert_eq!(pane.read_with(cx, |pane, _| pane.h_offset), 0.);
    // Nothing scrolls sideways while lines wrap.
    pane.update(cx, |pane, cx| pane.scroll_sideways(200., cx));
    assert_eq!(pane.read_with(cx, |pane, _| pane.h_offset), 0.);
    // Every visual line drew, each within the column, and together they hold the whole line.
    pane.read_with(cx, |pane, _| {
        let geometry = pane.geometry.borrow();
        let drawn = geometry
            .get(&("wrap.txt".to_string(), AnchorSide::New, long))
            .unwrap();
        assert_eq!(drawn.segments.len(), segments);
        let text = &pane.line("wrap.txt", long).unwrap().text;
        assert_eq!(drawn.segments.last().unwrap().bytes.end, text.len());
        for slice in &drawn.segments {
            let right = slice.x_of(slice.bytes.end);
            assert!(
                right <= slice.bounds.right(),
                "a visual line runs past its cell"
            );
        }
    });
    // In split view the short removed line beside it takes the row's height, its text on the
    // first visual line; the unchanged line before them stays one line high.
    let old = bounds(cx, "code/wrap.txt/old/2").expect("the removed line drew");
    assert_eq!(f32::from(old.size.height), heights::CODE * segments as f32);
    let first = bounds(cx, "code/wrap.txt/new/1").expect("the first line drew");
    assert_eq!(f32::from(first.size.height), heights::CODE);

    set_wrap(cx, false);
    assert_eq!(height(cx), heights::CODE);
}

#[gpui::test]
fn the_pointer_selects_text_across_visual_lines(cx: &mut TestAppContext) {
    let repo = wrap_fixture();
    let (_dir, shell, cx) = open_target(&repo, Target::Working, 1, cx);
    let pane = pane(&shell, cx);
    set_wrap(cx, true);
    let long = new_index(&pane, "wrap.txt", 2, cx);
    let key = ("wrap.txt", AnchorSide::New, long);
    let (text, second) = pane.read_with(cx, |pane, _| {
        let segments = pane.segments("wrap.txt", AnchorSide::New, long).unwrap();
        (
            pane.line("wrap.txt", long).unwrap().text.clone(),
            segments[1].bytes.clone(),
        )
    });

    // A click on the second visual line's third character puts the caret there.
    let at = segment_point(&pane, key, 1, 2, cx);
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    draw(cx);
    let caret = pane
        .read_with(cx, |pane, _| pane.caret.clone())
        .expect("a caret");
    assert_eq!(caret.offset, second.start + 2);

    // A drag from the first visual line's start to the second's fifth character selects
    // across the break.
    let from = segment_point(&pane, key, 0, 0, cx);
    let to = segment_point(&pane, key, 1, 4, cx);
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    draw(cx);
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    draw(cx);
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    draw(cx);
    let selected = pane
        .read_with(cx, |pane, _| pane.text_selection().map(|s| s.text))
        .expect("a selection");
    assert_eq!(selected, text[..second.start + 4]);
}
