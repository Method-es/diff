//! The stream's layout (DESIGN §4.1 "Diff", the board's `GlassReview` main column): the list of
//! rows over the `code` surface, the pinned header above it, and the file menu. Code rows are
//! 20 high in JetBrains Mono 12 (DESIGN §1.4): a 44-wide line number and a 16-wide marker per
//! side, the text clipped with a fade where it runs on.

mod header;
mod thread;

use dv_core::diff::{DiffLine, EXPAND_STEP, Expand, FileEntry, LineKind};
use dv_core::review::AnchorSide;
use gpui::{
    AnyElement, Context, FontFeatures, FontWeight, HighlightStyle, Hsla, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Render, ScrollWheelEvent, StatefulInteractiveElement as _, Styled as _, StyledText, Window,
    div, linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px,
};
use std::rc::Rc;

use super::rows::{Gap, InsertItem, LinePos, Notice, Row, heights, side_number};
use super::text::visible;
use super::{DiffPane, rows};
use crate::automation::AutomationTargetExt as _;
use crate::banner::{Banner, BannerAction};
use crate::components::{icon, skeleton, small_button};
use crate::icons::DvIcon;
use crate::keymap;
use crate::services::ViewMode;
use crate::session::{FileState, Shown};
use crate::syntax::FileHighlights;
use crate::theme::{ActiveTheme as _, DvTheme, StyledText as _};

/// A line number's column (the board's `numStyle`).
const NUMBER_WIDTH: f32 = 44.;
/// The `+` / `−` marker's column.
const MARKER_WIDTH: f32 = 16.;
/// Room before a line's text.
const TEXT_INSET: f32 = 4.;
/// The fade over text that runs on past the right edge.
const FADE_WIDTH: f32 = 28.;
/// The comment `+`'s square, over the marker column.
const COMMENT_BUTTON: f32 = 18.;
/// The hover groups a comment `+` shows in: a unified row, a split row's half.
const ROW_GROUP: &str = "code-row";
const HALF_GROUP: &str = "code-half";

impl Render for DiffPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Rc::new(cx.dv_theme().clone());
        if self.char_width.is_none() {
            let text = cx.text_system();
            let font = text.resolve_font(&gpui::font(theme.fonts.mono.clone()));
            self.char_width = text
                .ch_advance(font, theme.text.code.size)
                .ok()
                .map(f32::from);
        }
        self.code_columns = self.columns_in_view();
        self.release_orphaned_focus(window, cx);
        self.focus_waiting_composer(window, cx);
        // What the viewport shows is worked out again after this frame's layout, when it moved.
        if self.viewport_key != Some(self.current_viewport_key()) && !self.viewport_pending {
            self.viewport_pending = true;
            cx.defer_in(window, |this, _, cx| {
                this.viewport_pending = false;
                this.viewport_changed(cx);
            });
        }
        let view = cx.entity();
        let rows_theme = theme.clone();
        let list = gpui::list(self.list.clone(), move |item, window, cx| {
            let theme = rows_theme.clone();
            view.update(cx, |this, cx| this.render_item(item, &theme, window, cx))
        })
        .size_full();
        let pinned = self.pinned().map(|(block, shift)| {
            div()
                .absolute()
                .top(px(shift))
                .left_0()
                .right_0()
                // Clicks stay with the header; the wheel still scrolls the rows under it.
                .block_mouse_except_scroll()
                .child(self.render_header(block, true, &theme, cx))
        });
        let menu = self.render_menu(&theme, window, cx);
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .text_color(theme.colors.fg)
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let delta = event.delta.pixel_delta(px(heights::CODE));
                if delta.x != px(0.) {
                    this.scroll_sideways(-f32::from(delta.x), cx);
                }
            }))
            // A line drag (a press on a comment `+`) ends wherever the button comes up: over
            // the stream, before anything in it hears the release, or outside it.
            .capture_any_mouse_up(cx.listener(|this, event: &MouseUpEvent, window, cx| {
                if event.button == MouseButton::Left {
                    this.finish_line_drag(window, cx);
                }
            }))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.finish_line_drag(window, cx)),
            )
            .child(list)
            .children(pinned)
            .children(menu)
            .automation_target("stream")
    }
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
    cursor: Option<LinePos>,
    /// The selected lines (indexes into `lines`) on a side.
    selection: Option<(AnchorSide, usize, usize)>,
    /// Whether a comment can start on the old / the new side: the review isn't finished, and
    /// an interdiff's old column (a round's snapshot, which no anchor names) takes none.
    commentable: [bool; 2],
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

impl DiffPane {
    /// How many columns of code a cell shows at the list's width.
    fn columns_in_view(&self) -> usize {
        let width = f32::from(self.list.viewport_bounds().size.width);
        let width = if width > 0. { width } else { 1160. };
        let cell = match self.mode {
            ViewMode::Unified => width - 2. * NUMBER_WIDTH - MARKER_WIDTH - TEXT_INSET,
            ViewMode::Split => width / 2. - NUMBER_WIDTH - MARKER_WIDTH - TEXT_INSET - 1.,
        };
        let char_width = self.char_width.unwrap_or(7.2).max(1.);
        (cell.max(0.) / char_width).ceil() as usize + 1
    }

    /// List item `item`. Runs while the list lays out, so it never touches the list's state.
    fn render_item(
        &mut self,
        item: usize,
        theme: &DvTheme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some((block, row)) = self.locate(item) else {
            return div().w_full().h(px(self.spacer_height)).into_any_element();
        };
        let row = self.blocks[block].rows[row].clone();
        let element = match &row {
            Row::Header { .. } => self.render_header(block, false, theme, cx),
            Row::Line(_) | Row::Pair { .. } => self.render_code(block, &row, theme, cx),
            Row::Gap(gap) => self.render_gap(block, gap, theme, cx),
            Row::Notice(notice) => self.render_notice(block, notice, theme, cx),
            Row::Footer => self.render_footer(block, theme, cx),
            Row::Insert(insert) => match &insert.item {
                InsertItem::Thread(thread) => {
                    self.render_thread(block, thread, Some(insert), theme, cx)
                }
                InsertItem::Composer => self.render_composer(block, insert, theme, cx),
            },
            Row::Outdated(thread) => self.render_thread(block, thread, None, theme, cx),
        };
        div().w_full().child(element).into_any_element()
    }

    /// Block `block`'s session entry and loaded state.
    fn entry_and_state(
        &self,
        block: usize,
        cx: &gpui::App,
    ) -> Option<(FileEntry, Option<FileState>)> {
        let session = self.session.as_ref()?.read(cx);
        let entry = session.files().get(block)?.clone();
        let state = session.file_state(&entry.path).cloned();
        Some((entry, state))
    }

    fn render_code(
        &self,
        block: usize,
        row: &Row,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = self.blocks[block].path.clone();
        // The diff the rows were built from: the loaded one, or while it loads again the last.
        let shown = self
            .session
            .as_ref()
            .and_then(|session| session.read(cx).shown(&path));
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
        let writable = !self.read_only(cx);
        let code = CodeCx {
            theme,
            path: &path,
            lines: &text.lines,
            highlights: highlights.as_deref(),
            first,
            shift: self.h_offset - first as f32 * char_width,
            count: self.code_columns,
            cursor,
            selection,
            commentable: [writable && !self.comparison(cx).is_interdiff(), writable],
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
        let numbers = div()
            .relative()
            .flex()
            .flex_none()
            .bg(bg)
            .child(number(AnchorSide::Old))
            .child(number(AnchorSide::New))
            .child(marker(diff_line.kind, code.theme));
        vec![
            numbers
                .children(self.comment_button(
                    code,
                    line,
                    side,
                    2. * NUMBER_WIDTH - 1.,
                    ROW_GROUP,
                    cx,
                ))
                .into_any_element(),
            text_cell(code, Some(line), side, bg, cursor || selected, c.code).into_any_element(),
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
        half.bg(bg)
            .child(self.number_cell(code, Some(line), side, gutter, cursor, cx))
            .child(marker(diff_line.kind, code.theme))
            .child(text_cell(
                code,
                Some(line),
                side,
                bg,
                cursor || selected,
                c.code,
            ))
            .children(self.comment_button(code, line, side, NUMBER_WIDTH - 1., HALF_GROUP, cx))
            .into_any_element()
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
        let side_key = side_key(side);
        let name = format!("stream/comment/{}/{side_key}/{number}", code.path);
        let selector = format!("comment/{}/{side_key}/{number}", code.path);
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
        let side_key = side_key(side);
        cell.cursor_pointer()
            .child(number.to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.click_line(&path, LinePos { line, side }, event.modifiers.shift, cx);
                }),
            )
            .automation_target(format!("stream/line/{}/{side_key}/{number}", code.path))
    }

    /// A gap row (DESIGN §4.1): the next hunk's `@@ -a,b +c,d @@`, how many lines it hides, and
    /// "Expand 20 · Expand all", on the `hunk` tint.
    fn render_gap(
        &self,
        block: usize,
        gap: &Gap,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let path = self.blocks[block].path.clone();
        let name = format!("stream/gap/{path}/{}", gap.ordinal);
        let hidden = gap.lines.len();
        let ordinal = gap.ordinal;
        let step = if gap.below {
            Expand::FromBelow
        } else {
            Expand::FromAbove
        };
        let button = |key: &'static str, label: String, expand: Expand| {
            let path = path.clone();
            ghost_small(
                gpui::ElementId::NamedInteger(key.into(), (block * 10_000 + ordinal) as u64),
                theme,
            )
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.expand(&path, ordinal, expand, cx);
            }))
            .automation_target(format!("{name}/{key}"))
        };
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(heights::GAP))
            .pl(theme.spacing.xl)
            .pr(theme.spacing.xl)
            .bg(c.hunk)
            .border_t_1()
            .border_b_1()
            .border_color(c.line)
            .text_spec(&theme.text.secondary)
            .text_color(c.fg3)
            .child(icon(DvIcon::ChevronsUpDown, px(13.), c.fg3))
            .children(gap.hunk.map(|hunk| {
                div()
                    .font_family(theme.fonts.mono.clone())
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c.blue)
                    .child(hunk.to_string())
            }))
            .child(format!(
                "··· {hidden} unchanged {}",
                if hidden == 1 { "line" } else { "lines" }
            ))
            .when(hidden > EXPAND_STEP, |this| {
                this.child(button("expand", format!("Expand {EXPAND_STEP}"), step))
                    .child("·")
            })
            .child(button("expand_all", "Expand all".to_string(), Expand::All))
            .automation_target(name)
    }

    /// What a file shows in place of its lines.
    fn render_notice(
        &self,
        block: usize,
        notice: &Notice,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let path = self.blocks[block].path.clone();
        let name = format!("stream/notice/{path}");
        let Some((entry, state)) = self.entry_and_state(block, cx) else {
            return div().into_any_element();
        };
        match notice {
            Notice::Loading => div()
                .flex()
                .flex_col()
                .gap(theme.spacing.md)
                .h(px(heights::LOADING))
                .px(theme.spacing.xl)
                .py(theme.spacing.lg)
                .children(
                    [420., 300., 360.]
                        .into_iter()
                        .map(|width| skeleton(px(width), px(10.), theme)),
                )
                .automation_target(name),
            Notice::Failed => {
                let failure = match state {
                    Some(FileState::Failed(failure)) => failure,
                    _ => crate::Failure::new("The file's changes didn't load"),
                };
                let open = self.files.get(&path).is_some_and(|ui| ui.banner_open);
                let this = cx.weak_entity();
                let toggle_path = path.clone();
                let retry_path = path.clone();
                let session = self.session.clone();
                Banner::new(name, failure)
                    .disclosure(open, move |_, cx| {
                        this.update(cx, |this, cx| {
                            if let Some(ui) = this.files.get_mut(&toggle_path) {
                                ui.banner_open = !ui.banner_open;
                            }
                            if let Some(&index) = this.index.get(&toggle_path) {
                                let item = this.starts[index] + 1;
                                this.remeasure(item);
                            }
                            cx.notify();
                        })
                        .ok();
                    })
                    .action(BannerAction::new("retry", "Retry", move |_, cx| {
                        if let Some(session) = &session {
                            session.update(cx, |session, cx| session.reload_file(&retry_path, cx));
                        }
                    }))
                    .into_any_element()
            }
            notice => {
                let (glyph, title, detail) = notice_text(notice, &entry);
                let load_anyway = matches!(notice, Notice::TooLarge { .. }).then(|| {
                    let path = path.clone();
                    let session = self.session.clone();
                    small_button(
                        gpui::ElementId::NamedInteger("load-anyway".into(), block as u64),
                        "Load anyway",
                        theme,
                    )
                    .on_click(move |_, _, cx| {
                        if let Some(session) = &session {
                            session.update(cx, |session, cx| session.load_anyway(&path, cx));
                        }
                    })
                    .automation_target(format!("{name}/load_anyway"))
                });
                let dirty = matches!(notice, Notice::Submodule { dirty: true, .. }).then(|| {
                    crate::components::chip(c.yellow, c.yellow_soft, theme).child("dirty")
                });
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(heights::NOTICE))
                    .px(theme.spacing.xl)
                    .text_spec(&theme.text.body)
                    .child(icon(glyph, px(15.), c.fg3))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(c.fg2)
                            .child(title),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_spec(&theme.text.secondary)
                            .text_color(c.fg3)
                            .child(detail),
                    )
                    .children(dirty)
                    .children(load_anyway)
                    .automation_target(name)
            }
        }
    }

    /// The next-file footer: "Next: `path`" and "Mark viewed and go next `V` `J`".
    fn render_footer(&self, block: usize, theme: &DvTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let path = &self.blocks[block].path;
        let name = format!("stream/footer/{path}");
        let next = self.blocks.get(block + 1).map(|next| next.path.clone());
        let viewed = self.is_viewed(path, cx);
        let read_only = self
            .session
            .as_ref()
            .is_some_and(|session| session.read(cx).is_read_only());
        let (label, keys): (Option<&str>, &[&str]) = match (&next, viewed || read_only) {
            (Some(_), false) => (Some("Mark viewed and go next"), &["v", "j"]),
            (Some(_), true) => (Some("Go to next file"), &["j"]),
            (None, false) => (Some("Mark viewed"), &["v"]),
            (None, true) => (None, &[]),
        };
        // The next file's path truncates at its start, so its name stays (DESIGN §4.1).
        let note = match next {
            Some(next) => div()
                .flex()
                .gap(px(4.))
                .min_w_0()
                .child(div().flex_none().child("Next:"))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis_start()
                        .child(next),
                ),
            None => div().child("Last file"),
        };
        let button = label.map(|label| {
            let keys: Vec<_> = keys
                .iter()
                .map(|key| crate::components::kbd(keymap::label(key), theme))
                .collect();
            crate::components::button(
                gpui::ElementId::NamedInteger("footer-next".into(), block as u64),
                theme,
            )
            .child(label)
            .children(keys)
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.blocks.get(block + 1).is_some() {
                    this.mark_viewed_and_next(block, cx);
                } else if let Some(path) = this.blocks.get(block).map(|b| b.path.clone()) {
                    this.toggle_viewed(&path, cx);
                }
            }))
            .automation_target(format!("{name}/next"))
        });
        div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(10.))
            .h(px(heights::FOOTER))
            .px(theme.spacing.xl)
            .child(
                note.min_w_0()
                    .text_spec(&theme.text.secondary)
                    .text_color(c.fg3),
            )
            .children(button)
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
/// tint, shifted by the sideways scroll; a fade where it runs on, and `sel` when selected.
fn text_cell(
    code: &CodeCx,
    line: Option<usize>,
    side: AnchorSide,
    bg: Hsla,
    selected: bool,
    surface: Hsla,
) -> impl IntoElement {
    let c = &code.theme.colors;
    let cell = div()
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_hidden()
        .when(selected, |this| this.bg(c.sel));
    let Some(diff_line) = line.and_then(|line| code.lines.get(line)) else {
        return cell;
    };
    let number = side_number(diff_line, side);
    let tokens = code
        .highlights
        .and_then(|highlights| highlights.side(side))
        .zip(number)
        .map(|(side, number)| side.line(number))
        .unwrap_or_default();
    let shown = visible(
        &diff_line.text,
        tokens,
        &diff_line.words,
        code.first,
        code.count,
    );
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
    cell.child(
        div()
            .absolute()
            .top_0()
            .left(px(TEXT_INSET - code.shift))
            .h_full()
            .whitespace_nowrap()
            .text_color(syntax.plain)
            .child(text),
    )
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
}

/// The board's `ghostSm` button: 22 high, no fill until hovered, 12/500 in `fg2`.
fn ghost_small(id: gpui::ElementId, theme: &DvTheme) -> gpui::Stateful<gpui::Div> {
    let c = &theme.colors;
    let hover = c.control_hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(5.))
        .h(theme.sizes.control_xsmall_height)
        .px(theme.spacing.md)
        .rounded(theme.radii.control)
        .text_spec(&theme.text.secondary)
        .font_weight(FontWeight::MEDIUM)
        .text_color(c.fg2)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
}

/// A notice's glyph, title and detail.
fn notice_text(notice: &Notice, entry: &FileEntry) -> (DvIcon, &'static str, String) {
    match notice {
        Notice::Binary { old_len, new_len } => {
            (DvIcon::Binary, "Binary file", sizes(*old_len, *new_len))
        }
        Notice::TooLarge { old_len, new_len } => (
            DvIcon::FileDiff,
            "Too large to show",
            format!("{} — over 2 MB or 20,000 lines", sizes(*old_len, *new_len)),
        ),
        Notice::Submodule {
            old_oid, new_oid, ..
        } => {
            let short = |oid: &Option<dv_core::Oid>| {
                oid.as_ref()
                    .map_or("none".to_string(), |oid| oid.as_str()[..7].to_string())
            };
            (
                DvIcon::Package,
                "Submodule",
                format!("{} → {}", short(old_oid), short(new_oid)),
            )
        }
        Notice::Unmerged => (
            DvIcon::TriangleAlert,
            "Unresolved conflict",
            "the index holds no single version of this file".to_string(),
        ),
        Notice::NoContentChange => (
            DvIcon::FileDiff,
            "No content change",
            match (entry.old_mode, entry.new_mode, &entry.old_path) {
                (_, _, Some(old)) => format!("renamed from {old}"),
                (Some(old), Some(new), _) if old != new => {
                    format!("mode {} → {}", mode_text(old), mode_text(new))
                }
                _ => "an empty file".to_string(),
            },
        ),
        Notice::Loading | Notice::Failed => (DvIcon::FileDiff, "", String::new()),
    }
}

fn mode_text(mode: dv_core::diff::FileMode) -> &'static str {
    use dv_core::diff::FileMode;
    match mode {
        FileMode::Regular => "100644",
        FileMode::Executable => "100755",
        FileMode::Symlink => "120000",
        FileMode::Submodule => "160000",
    }
}

/// "12.4 KB → 13.1 KB", "added · 3 KB", "deleted · 3 KB".
fn sizes(old: Option<u64>, new: Option<u64>) -> String {
    match (old, new) {
        (Some(old), Some(new)) => format!("{} → {}", byte_size(old), byte_size(new)),
        (None, Some(new)) => format!("added · {}", byte_size(new)),
        (Some(old), None) => format!("deleted · {}", byte_size(old)),
        (None, None) => "size unknown".to_string(),
    }
}

/// A size as people read it: bytes, KB or MB (1024s), one decimal above bytes.
pub fn byte_size(bytes: u64) -> String {
    const KB: f64 = 1024.;
    let value = bytes as f64;
    if value < KB {
        format!("{bytes} B")
    } else if value < KB * KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{:.1} MB", value / (KB * KB))
    }
}

/// A side as automation targets name it.
fn side_key(side: AnchorSide) -> &'static str {
    match side {
        AnchorSide::Old => "old",
        AnchorSide::New => "new",
    }
}
