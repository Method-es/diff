//! Code rows (DESIGN §4.1 "Diff"): 20 high in JetBrains Mono 12 (DESIGN §1.4), a 44-wide line
//! number and a 16-wide marker per side (both numbers then the marker in unified view), the
//! text clipped with a fade where it runs on. Over the text: find matches (`findMatch`, the
//! current one `findCurrent`), the selected text (`textSel`) and the caret, a 2px accent bar
//! while the stream has the keyboard (PLAN §18.1, `select`, `find`).
//!
//! The marks and the caret are placed with the painted slice shaped as it paints
//! ([`marks_canvas`]): a wide character takes two cells, a combining mark none, a tab four.
//! Under automation each character of a line in view is a target,
//! `code/<path>/<old|new>/<line>/<column>` (the column 1-based in characters of the source line,
//! the target the room its glyph takes), recorded only while a capture is armed
//! ([`crate::automation::record_target`]).

use std::cell::Cell;
use std::rc::Rc;

use dv_core::diff::{DiffLine, LineKind};
use dv_core::review::AnchorSide;
use gpui::{
    AnyElement, BorderStyle, Bounds, Context, Div, Font, FontFeatures, FontWeight, HighlightStyle,
    Hsla, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    PaintQuad, ParentElement as _, Pixels, Styled as _, StyledText, TextRun, Window, canvas, div,
    fill, linear_color_stop, linear_gradient, point, prelude::FluentBuilder as _, px, quad, size,
};

use super::super::DiffPane;
use super::super::rows::{self, LinePos, Row, heights, side_number};
use super::super::select::{Geometry, GeometryKey, LineGeometry, Span};
use super::super::text::{Visible, visible};
use super::side_key;
use crate::automation::AutomationTargetExt as _;
use crate::components::icon;
use crate::icons::DvIcon;
use crate::services::ViewMode;
use crate::session::Shown;
use crate::syntax::FileHighlights;
use crate::theme::{DvTheme, StyledText as _};

/// A line number's column (the board's `numStyle`).
pub(in crate::diff_pane) const NUMBER_WIDTH: f32 = 44.;
/// The `+` / `−` marker's column.
pub(in crate::diff_pane) const MARKER_WIDTH: f32 = 16.;
/// Room before a line's text.
pub(in crate::diff_pane) const TEXT_INSET: f32 = 4.;
/// The fade over text that runs on past the right edge.
const FADE_WIDTH: f32 = 28.;
/// The comment `+`'s square, over the marker column.
const COMMENT_BUTTON: f32 = 18.;
/// The caret: a bar this wide, this far in from the row's top and bottom.
const CARET_WIDTH: f32 = 2.;
const CARET_INSET: f32 = 2.;
/// The hover groups a comment `+` shows in: a unified row, a split row's half.
const ROW_GROUP: &str = "code-row";
const HALF_GROUP: &str = "code-half";

/// What code rows paint over their text this frame, worked out once a frame (`render`).
#[derive(Clone, Debug, Default)]
pub(in crate::diff_pane) struct FrameMarks {
    /// The selected text's file and span.
    pub selection: Option<(String, Span)>,
    /// The caret while the stream has the keyboard: its file, side, line (an index into the
    /// file's lines) and byte offset.
    pub caret: Option<(String, AnchorSide, usize, usize)>,
    /// Whether find matches paint (the bar is open with a query).
    pub matches: bool,
}

/// What every code row of one file paints with.
struct CodeCx<'a> {
    theme: &'a DvTheme,
    path: &'a str,
    lines: &'a [DiffLine],
    highlights: Option<&'a FileHighlights>,
    /// The first column in view, the pixels of it scrolled off, and how many columns show.
    first: usize,
    shift: f32,
    count: usize,
    char_width: f32,
    /// Where each line this frame paints keeps its shaped text, for the pointer.
    geometry: Geometry,
    cursor: Option<LinePos>,
    /// The selected lines (indexes into `lines`) on a side.
    selection: Option<(AnchorSide, usize, usize)>,
    /// The selected text, when it is in this file.
    span: Option<Span>,
    /// The caret, when it is in this file: its side, line and byte offset.
    caret: Option<(AnchorSide, usize, usize)>,
    matches: bool,
    /// Whether a comment can start on the old / the new side: the review isn't finished, and
    /// an interdiff's old column (a round's snapshot, which no anchor names) takes none.
    commentable: [bool; 2],
    /// The file view: one line number, its side's, and its own automation names
    /// (`file_view/line/<n>`, `file_view/comment/<n>`, `file_view/code/<n>/<column>`).
    file_view: bool,
}

impl CodeCx<'_> {
    /// Line `number` on `side`'s automation name for `kind` (`line`, `comment`, `code`): the
    /// stream's `stream/line/<path>/<side>/<n>`, `stream/comment/…` and `code/<path>/<side>/<n>`,
    /// or the file view's `file_view/<kind>/<n>`.
    fn target(&self, kind: &str, side: AnchorSide, number: u32) -> String {
        match (self.file_view, kind) {
            (true, kind) => format!("file_view/{kind}/{number}"),
            (false, "code") => format!("code/{}/{}/{number}", self.path, side_key(side)),
            (false, kind) => format!("stream/{kind}/{}/{}/{number}", self.path, side_key(side)),
        }
    }
}

impl CodeCx<'_> {
    fn is_cursor(&self, line: usize, side: AnchorSide, mode: ViewMode) -> bool {
        self.cursor
            .is_some_and(|c| c.line == line && (mode == ViewMode::Unified || c.side == side))
    }

    fn can_comment(&self, side: AnchorSide) -> bool {
        match side {
            AnchorSide::Old => self.commentable[0],
            AnchorSide::New => self.commentable[1],
        }
    }

    fn is_selected(&self, line: usize, side: AnchorSide) -> bool {
        self.selection.is_some_and(|(selected, from, to)| {
            selected == side
                && (from..=to).contains(&line)
                && rows::on_side(&self.lines[line], side)
        })
    }
}

/// What paints over one line's text: background marks and the caret's byte.
#[derive(Clone, Default)]
struct LineMarks {
    spans: Vec<Mark>,
    caret: Option<usize>,
}

/// A mark behind a line's text: a byte range of the source line, whether it goes on past the
/// line's end (a column further: the selection reaching the next line), its fill and outline.
#[derive(Clone)]
struct Mark {
    range: std::ops::Range<usize>,
    past_end: bool,
    fill: Hsla,
    outline: Option<Hsla>,
}

impl DiffPane {
    /// This frame's [`FrameMarks`].
    pub(in crate::diff_pane) fn frame_marks(&self, window: &Window) -> FrameMarks {
        let caret = self
            .caret
            .as_ref()
            .filter(|_| self.focus_handle.is_focused(window))
            .and_then(|caret| {
                let lines = self.lines(&caret.path)?;
                let line = super::super::select::line_index(lines, caret.side, caret.line)?;
                Some((caret.path.clone(), caret.side, line, caret.offset))
            });
        FrameMarks {
            selection: self.painted_span(),
            caret,
            matches: self.paints_matches(),
        }
    }

    pub(in crate::diff_pane) fn render_code(
        &self,
        block: usize,
        row: &Row,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = self.blocks[block].path.clone();
        // The diff the rows were built from: the loaded one, or while it loads again the last
        // (the file view's text, cut to its side).
        let shown = self.shown_for(&path, cx);
        let (Some(Shown { diff, highlights }), Some(char_width)) = (shown, self.char_width) else {
            return div().h(px(heights::CODE)).into_any_element();
        };
        let Some(text) = diff.as_text() else {
            return div().h(px(heights::CODE)).into_any_element();
        };
        let first = (self.h_offset / char_width).floor() as usize;
        let cursor = self
            .cursor
            .as_ref()
            .filter(|cursor| cursor.path == path)
            .map(|cursor| cursor.pos);
        let selection = self.selection.as_ref().and_then(|anchor| {
            let cursor = cursor.filter(|_| anchor.path == path)?;
            Some((
                anchor.side,
                anchor.line.min(cursor.line),
                anchor.line.max(cursor.line),
            ))
        });
        let marks = &self.frame_marks;
        let writable = !self.read_only(cx);
        let code = CodeCx {
            theme,
            path: &path,
            lines: &text.lines,
            highlights: highlights.as_deref(),
            first,
            shift: self.h_offset - first as f32 * char_width,
            count: self.code_columns,
            char_width,
            geometry: self.geometry.clone(),
            cursor,
            selection,
            span: marks
                .selection
                .as_ref()
                .filter(|(at, _)| *at == path)
                .map(|(_, span)| *span),
            caret: marks
                .caret
                .as_ref()
                .filter(|(at, ..)| *at == path)
                .map(|&(_, side, line, offset)| (side, line, offset)),
            matches: marks.matches,
            commentable: [writable && !self.comparison(cx).is_interdiff(), writable],
            file_view: self.is_file_view(),
        };
        // The row's lines on each side, for a line drag passing over it.
        let (old, new) = match *row {
            Row::Line(line) => (Some(line), Some(line)),
            Row::Pair { old, new } => (old, new),
            _ => (None, None),
        };
        let drag_path = path.clone();
        // Code shows the characters as typed: no ligatures (`=>` stays two characters).
        let row_element = div()
            .relative()
            .group(ROW_GROUP)
            .flex()
            .h(px(heights::CODE))
            .w_full()
            .text_spec(&theme.text.code)
            .font_features(FontFeatures::disable_ligatures())
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                    if this.line_drag.is_none() {
                        return;
                    }
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.drag_over_row(&drag_path, old, new, cx);
                    } else {
                        // The button came up where the stream didn't hear it.
                        this.finish_line_drag(window, cx);
                    }
                }),
            );
        match *row {
            Row::Line(line) => row_element
                .children(self.unified_cells(&code, line, cx))
                .into_any_element(),
            Row::Pair { old, new } => row_element
                .child(self.split_cell(&code, old, AnchorSide::Old, cx))
                .child(self.split_cell(&code, new, AnchorSide::New, cx))
                .into_any_element(),
            _ => row_element.into_any_element(),
        }
    }

    /// A unified row: the old and the new line number, the marker, the text.
    fn unified_cells(&self, code: &CodeCx, line: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = &code.theme.colors;
        let Some(diff_line) = code.lines.get(line) else {
            return Vec::new();
        };
        let (bg, gutter) = tints(diff_line.kind, code.theme);
        let side = rows::natural_side(diff_line);
        let cursor = code.is_cursor(line, side, ViewMode::Unified);
        // The cursor's mark goes on its side's number, or the line's own side when it has
        // none there.
        let marked = code.cursor.filter(|_| cursor).map(|c| {
            if rows::on_side(diff_line, c.side) {
                c.side
            } else {
                side
            }
        });
        let selected = code.selection.is_some_and(|(selected, from, to)| {
            (from..=to).contains(&line) && rows::on_side(diff_line, selected)
        });
        let mut number = |side: AnchorSide| {
            self.number_cell(code, Some(line), side, gutter, marked == Some(side), cx)
        };
        // The file view numbers its one side only.
        let numbered: &[AnchorSide] = if code.file_view {
            std::slice::from_ref(&side)
        } else {
            &[AnchorSide::Old, AnchorSide::New]
        };
        let numbers = div()
            .relative()
            .flex()
            .flex_none()
            .bg(bg)
            .children(numbered.iter().map(|&side| number(side)))
            .child(marker(diff_line.kind, code.theme));
        let marks = self.line_marks(code, line, side);
        vec![
            numbers
                .children(self.comment_button(
                    code,
                    line,
                    side,
                    numbered.len() as f32 * NUMBER_WIDTH - 1.,
                    ROW_GROUP,
                    cx,
                ))
                .into_any_element(),
            self.pressable(
                text_cell(
                    code,
                    Some(line),
                    side,
                    bg,
                    cursor || selected,
                    c.code,
                    &marks,
                ),
                code.path,
                line,
                side,
                cx,
            ),
        ]
    }

    /// One side of a split row: its line number, marker and text, or an empty cell.
    fn split_cell(
        &self,
        code: &CodeCx,
        line: Option<usize>,
        side: AnchorSide,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &code.theme.colors;
        let half = div()
            .relative()
            .group(HALF_GROUP)
            .flex()
            .w_1_2()
            .min_w_0()
            .h_full()
            .when(side == AnchorSide::Old, |this| {
                this.border_r_1().border_color(c.line)
            });
        let Some(diff_line) = line.and_then(|line| code.lines.get(line)) else {
            return half.bg(c.recess).into_any_element();
        };
        let line = line.unwrap_or_default();
        let (bg, gutter) = tints(diff_line.kind, code.theme);
        let cursor = code.is_cursor(line, side, ViewMode::Split);
        let selected = code.is_selected(line, side);
        let marks = self.line_marks(code, line, side);
        half.bg(bg)
            .child(self.number_cell(code, Some(line), side, gutter, cursor, cx))
            .child(marker(diff_line.kind, code.theme))
            .child(self.pressable(
                text_cell(
                    code,
                    Some(line),
                    side,
                    bg,
                    cursor || selected,
                    c.code,
                    &marks,
                ),
                code.path,
                line,
                side,
                cx,
            ))
            .children(self.comment_button(code, line, side, NUMBER_WIDTH - 1., HALF_GROUP, cx))
            .into_any_element()
    }

    /// `cell` (line `line`'s text on `side`) taking the mouse: a press selects text and puts
    /// the caret (`select`).
    fn pressable(
        &self,
        cell: Div,
        path: &str,
        line: usize,
        side: AnchorSide,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = path.to_string();
        cell.cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.press_text(&path, line, side, event, cx);
                }),
            )
            .into_any_element()
    }

    /// What paints over line `line`'s text on `side`: its find matches, the selected part of
    /// it, and the caret.
    fn line_marks(&self, code: &CodeCx, line: usize, side: AnchorSide) -> LineMarks {
        let c = &code.theme.colors;
        let mut marks = LineMarks::default();
        if code.lines.get(line).is_none() {
            return marks;
        }
        if code.matches {
            for (range, current) in self.matches_on(code.path, line, side) {
                // An outline on every match, so one on an intraline word's fill still reads as
                // a match (DESIGN §4.1 "Find").
                let (fill, outline) = if current {
                    (c.find_current, Some(c.yellow))
                } else {
                    (c.find_match, Some(c.find_line))
                };
                marks.spans.push(Mark {
                    range,
                    past_end: false,
                    fill,
                    outline,
                });
            }
        }
        if let Some((from, to, more)) = code
            .span
            .and_then(|span| span.on_line(code.lines, line, side))
        {
            marks.spans.push(Mark {
                range: from..to,
                past_end: more,
                fill: c.text_sel,
                outline: None,
            });
        }
        if let Some((caret_side, caret_line, offset)) = code.caret
            && caret_line == line
            && caret_side == side
        {
            marks.caret = Some(offset);
        }
        marks
    }

    /// The `+` that starts a comment on `line`'s `side` with the mouse (DESIGN §4.1), `left` in
    /// from its row's (or half's) edge over the marker column, shown while the pointer is over
    /// `group`. A press puts the cursor on the line and starts the drag that picks the
    /// comment's lines; the release opens the composer on them. None where no comment can start.
    fn comment_button(
        &self,
        code: &CodeCx,
        line: usize,
        side: AnchorSide,
        left: f32,
        group: &'static str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !code.can_comment(side) {
            return None;
        }
        let number = code.lines.get(line).and_then(|l| side_number(l, side))?;
        let c = &code.theme.colors;
        let path = code.path.to_string();
        let name = code.target("comment", side, number);
        let selector = format!("comment/{}/{}/{number}", code.path, side_key(side));
        Some(
            div()
                .absolute()
                .top(px((heights::CODE - COMMENT_BUTTON) / 2.))
                .left(px(left))
                .size(px(COMMENT_BUTTON))
                .flex()
                .items_center()
                .justify_center()
                .rounded(code.theme.radii.small)
                .bg(c.accent)
                .cursor_pointer()
                .opacity(0.)
                .group_hover(group, |style| style.opacity(1.))
                .child(icon(DvIcon::Plus, px(12.), c.accent_fg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        // The line number under the button's edge doesn't hear the press.
                        cx.stop_propagation();
                        this.start_line_drag(&path, LinePos { line, side }, cx);
                    }),
                )
                .debug_selector(move || selector)
                .automation_target(name),
        )
    }

    /// A line number: `fg3`, or the accent at 600 on the cursor's line. A click puts the
    /// cursor on the line; with Shift, the selection reaches to it.
    fn number_cell(
        &self,
        code: &CodeCx,
        line: Option<usize>,
        side: AnchorSide,
        gutter: Hsla,
        cursor: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &code.theme.colors;
        let number = line
            .and_then(|line| code.lines.get(line))
            .and_then(|l| side_number(l, side));
        let cell = div()
            .relative()
            .flex_none()
            .w(px(NUMBER_WIDTH))
            .h_full()
            .pr(px(8.))
            .flex()
            .justify_end()
            .bg(gutter)
            .text_color(if cursor { c.accent_text } else { c.fg3 })
            .when(cursor, |this| {
                this.font_weight(FontWeight::SEMIBOLD).child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .w(px(2.))
                        .h_full()
                        .bg(c.accent),
                )
            });
        let (Some(number), Some(line)) = (number, line) else {
            return cell.into_any_element();
        };
        let path = code.path.to_string();
        let name = code.target("line", side, number);
        cell.cursor_pointer()
            .child(number.to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.click_line(&path, LinePos { line, side }, event.modifiers.shift, cx);
                }),
            )
            .automation_target(name)
    }
}

/// A row's background and line-number tint for its kind (DESIGN §1.1 diff tokens).
fn tints(kind: LineKind, theme: &DvTheme) -> (Hsla, Hsla) {
    let c = &theme.colors;
    match kind {
        LineKind::Added => (c.add_bg, c.add_gut),
        LineKind::Removed => (c.del_bg, c.del_gut),
        LineKind::Context => (gpui::transparent_black(), gpui::transparent_black()),
    }
}

/// The `+` / `−` marker.
fn marker(kind: LineKind, theme: &DvTheme) -> impl IntoElement {
    let c = &theme.colors;
    let (text, color) = match kind {
        LineKind::Added => ("+", c.green),
        LineKind::Removed => ("−", c.red),
        LineKind::Context => ("", c.fg3),
    };
    div()
        .flex_none()
        .w(px(MARKER_WIDTH))
        .h_full()
        .flex()
        .justify_center()
        .text_color(color)
        .child(text)
}

/// A line's text: the columns in view, coloured by token kind with intraline words on their
/// tint, shifted by the sideways scroll; `marks` behind it and the caret over it, a fade where it
/// runs on, and `sel` when its line is selected.
///
/// The marks, the caret and automation's character targets are placed with the painted slice
/// shaped as the text paints it (the code font, no ligatures), so a wide character or a
/// combining mark takes the room it is drawn in; the shaped line is also kept for the pointer
/// (`select`, [`LineGeometry`]).
fn text_cell(
    code: &CodeCx,
    line: Option<usize>,
    side: AnchorSide,
    bg: Hsla,
    selected: bool,
    surface: Hsla,
    marks: &LineMarks,
) -> Div {
    let c = &code.theme.colors;
    let cell = div()
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_hidden()
        .when(selected, |this| this.bg(c.sel));
    let Some((line, diff_line)) = line.and_then(|line| Some((line, code.lines.get(line)?))) else {
        return cell;
    };
    let number = side_number(diff_line, side);
    let tokens = code
        .highlights
        .and_then(|highlights| highlights.side(side))
        .zip(number)
        .map(|(side, number)| side.line(number))
        .unwrap_or_default();
    let shown = Rc::new(visible(
        &diff_line.text,
        tokens,
        &diff_line.words,
        code.first,
        code.count,
    ));
    let word = match diff_line.kind {
        LineKind::Added => c.add_word,
        LineKind::Removed => c.del_word,
        LineKind::Context => c.sel,
    };
    let syntax = c.syntax;
    let runs = shown.runs.iter().map(|(range, style)| {
        (
            range.clone(),
            HighlightStyle {
                color: Some(style.kind.color(&syntax)),
                background_color: style.word.then_some(word),
                ..Default::default()
            },
        )
    });
    let text = StyledText::new(shown.text.clone()).with_highlights(runs);
    let behind = {
        let base = surface.blend(bg);
        if selected { base.blend(c.sel) } else { base }
    };
    // The line's name for automation's character targets and tests' bounds, only made for them.
    let named = crate::automation::is_active() || cfg!(test);
    let name = number
        .filter(|_| named)
        .map(|number| code.target("code", side, number));
    let caret_x = Rc::new(Cell::new(None));
    let back = marks_canvas(MarksCx {
        key: (code.path.to_string(), side, line),
        geometry: code.geometry.clone(),
        source: diff_line.text.clone(),
        shown: shown.clone(),
        font: Font {
            features: FontFeatures::disable_ligatures(),
            weight: code.theme.text.code.weight,
            ..gpui::font(code.theme.fonts.mono.clone())
        },
        font_size: code.theme.text.code.size,
        inset: TEXT_INSET - code.shift,
        char_width: code.char_width,
        marks: marks.clone(),
        caret_x: caret_x.clone(),
        targets: name.clone().filter(|_| crate::automation::is_active()),
    });
    let accent = c.accent;
    let caret = marks.caret.map(|_| {
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                if let Some(x) = caret_x.get() {
                    let bar = Bounds {
                        origin: point(x - px(CARET_WIDTH / 2.), bounds.origin.y + px(CARET_INSET)),
                        size: size(px(CARET_WIDTH), bounds.size.height - px(2. * CARET_INSET)),
                    };
                    window.paint_quad(fill(bar, accent));
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    });
    cell.child(back)
        .child(
            div()
                .absolute()
                .top_0()
                .left(px(TEXT_INSET - code.shift))
                .h_full()
                .whitespace_nowrap()
                .text_color(syntax.plain)
                .child(text),
        )
        .children(caret)
        // Scrolled sideways, the text slides under a short fade at the left edge too.
        .when(code.first > 0 || code.shift > 0., |this| {
            this.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .h_full()
                    .w(px(FADE_WIDTH / 2.))
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(behind, 0.),
                        linear_color_stop(behind.opacity(0.), 1.),
                    )),
            )
        })
        .when(shown.clipped, |this| {
            this.child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .h_full()
                    .w(px(FADE_WIDTH))
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(behind.opacity(0.), 0.),
                        linear_color_stop(behind, 1.),
                    )),
            )
        })
        .when_some(name, |this, name| this.debug_selector(move || name))
}

/// What [`marks_canvas`] paints with.
struct MarksCx {
    key: GeometryKey,
    geometry: Geometry,
    source: String,
    shown: Rc<Visible>,
    font: Font,
    font_size: Pixels,
    /// The painted slice's left edge, from the cell's.
    inset: f32,
    char_width: f32,
    marks: LineMarks,
    /// Where the caret goes, for the canvas over the text.
    caret_x: Rc<Cell<Option<Pixels>>>,
    /// The line's automation name while automation runs: its characters are targets.
    targets: Option<String>,
}

/// The canvas behind a line's text: it shapes the painted slice, keeps the shaped line for the
/// pointer, places the marks, the caret and the character targets with it, and paints the
/// marks.
fn marks_canvas(cx: MarksCx) -> impl IntoElement {
    canvas(
        move |bounds: Bounds<Pixels>, window, _| {
            let MarksCx {
                key,
                geometry,
                source,
                shown,
                font,
                font_size,
                inset,
                char_width,
                marks,
                caret_x,
                targets,
            } = cx;
            let run = TextRun {
                len: shown.text.len(),
                font,
                color: gpui::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped =
                window
                    .text_system()
                    .shape_line(shown.text.clone().into(), font_size, &[run], None);
            let line = LineGeometry {
                bounds,
                origin_x: bounds.origin.x + px(inset),
                shaped,
                shown,
            };
            let top = bounds.origin.y;
            let quads: Vec<PaintQuad> = marks
                .spans
                .iter()
                .filter_map(|mark| {
                    let left = line.x_of(mark.range.start);
                    let mut right = line.x_of(mark.range.end);
                    if mark.past_end {
                        right += px(char_width);
                    }
                    (right > left).then(|| {
                        let area = Bounds {
                            origin: point(left, top + px(1.)),
                            size: size(right - left, bounds.size.height - px(2.)),
                        };
                        match mark.outline {
                            Some(outline) => quad(
                                area,
                                px(2.),
                                mark.fill,
                                px(1.),
                                outline,
                                BorderStyle::default(),
                            ),
                            None => fill(area, mark.fill),
                        }
                    })
                })
                .collect();
            caret_x.set(marks.caret.map(|offset| line.x_of(offset)));
            if let Some(name) = targets.filter(|_| crate::automation::is_capturing()) {
                for (index, (at, ch)) in source.char_indices().enumerate() {
                    let (left, right) = (line.x_of(at), line.x_of(at + ch.len_utf8()));
                    if right <= left || right <= bounds.left() || left >= bounds.right() {
                        continue;
                    }
                    let target = Bounds {
                        origin: point(left, top),
                        size: size(right - left, bounds.size.height),
                    };
                    crate::automation::record_target(
                        window,
                        || format!("{name}/{}", index + 1).into(),
                        target,
                    );
                }
            }
            geometry.borrow_mut().insert(key, line);
            quads
        },
        |_, quads, window, _| {
            for quad in quads {
                window.paint_quad(quad);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}
