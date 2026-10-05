//! The comments section's layout (the board's "Open comments" block under the files: a header
//! with the count and "Copy as prompt", then `listBtn` rows with a dot, `path:line` in mono and
//! the comment's first line).

use dv_core::review::ThreadState;
use gpui::{
    AnyElement, Context, ElementId, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use super::{CommentRow, CommentsList};
use crate::automation::AutomationTargetExt as _;
use crate::components::{count_badge, icon, small_button, small_icon_button, tooltip};
use crate::icons::DvIcon;
use crate::services::ActiveServices as _;
use crate::theme::{ActiveTheme as _, DvTheme, StyledText as _};

/// The tallest the list of rows grows before it scrolls.
const MAX_ROWS_HEIGHT: f32 = 240.;

/// The narrowest panel (DESIGN §2: 220–420) whose comments header has room for "Copy as
/// prompt" beside "Open comments" and a three-digit count.
const COPY_LABEL_FROM: f32 = 270.;

impl Render for CommentsList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.dv_theme().clone();
        let c = &theme.colors;
        let Some(session) = self.session.clone() else {
            return div().into_any_element();
        };
        let all = super::rows(session.read(cx), true);
        let waiting = all
            .iter()
            .filter(|row| row.state != ThreadState::Resolved)
            .count();
        let open = all
            .iter()
            .filter(|row| row.state == ThreadState::Open)
            .count();
        let resolved = all.len() - waiting;
        let focused = self.diff_pane.read(cx).focused_thread().cloned();
        let shown: Vec<&CommentRow> = all
            .iter()
            .filter(|row| self.show_resolved || row.state != ThreadState::Resolved)
            .collect();

        // Below `COPY_LABEL_FROM` the label doesn't fit beside the heading: the button keeps
        // its glyph, and its tooltip says what it does.
        let narrow = cx.services().settings.read(cx).panel_width() < COPY_LABEL_FROM;
        let copy = if narrow {
            small_icon_button("comments-copy", DvIcon::Copy, &theme)
                .tooltip(tooltip("Copy as prompt"))
        } else {
            small_button("comments-copy", "Copy as prompt", &theme)
        };
        let copy = copy
            .ml_auto()
            .debug_selector(|| "comments-copy".into())
            .when(open == 0, |this| this.opacity(0.5).cursor_default())
            .when(open > 0, |this| {
                this.on_click(cx.listener(|this, _, _, cx| this.copy_as_prompt(cx)))
            })
            .automation_target("comments_list/copy");
        // Nothing in the row paints past the panel: the heading gives way first.
        let header = div()
            .flex()
            .items_center()
            .min_w_0()
            .overflow_hidden()
            .gap(theme.spacing.md)
            .px(theme.spacing.md)
            .pb(theme.spacing.sm)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_spec(&theme.text.secondary)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c.fg2)
                    .child("Open comments"),
            )
            .child(count_badge(&theme).child(waiting.to_string()))
            .child(copy);
        let mut rows: Vec<AnyElement> = shown
            .iter()
            .map(|row| self.render_row(row, focused.as_ref() == Some(&row.id), &theme, cx))
            .collect();
        if waiting == 0 && !self.show_resolved {
            rows.push(
                div()
                    .px(theme.spacing.md)
                    .py(theme.spacing.xs)
                    .text_spec(&theme.text.secondary)
                    .text_color(c.fg3)
                    .child("No open comments")
                    .into_any_element(),
            );
        }
        if resolved > 0 {
            let hover = c.control_hover;
            let show = self.show_resolved;
            rows.push(
                div()
                    .id("comments-resolved")
                    .flex()
                    .items_center()
                    .gap(theme.spacing.sm)
                    .w_full()
                    .px(theme.spacing.md)
                    .py(px(5.))
                    .rounded(theme.radii.control)
                    .text_spec(&theme.text.secondary)
                    .text_color(c.fg2)
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .child(icon(DvIcon::CircleCheck, px(13.), c.green))
                    .child(format!(
                        "{resolved} resolved · {}",
                        if show { "hide" } else { "show" }
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_show_resolved(!show, cx)))
                    .automation_target("comments_list/resolved")
                    .into_any_element(),
            );
        }
        div()
            .flex_none()
            .flex()
            .flex_col()
            .min_w_0()
            .overflow_hidden()
            .pt(px(10.))
            .px(theme.spacing.sm)
            .pb(theme.spacing.md)
            .border_t_1()
            .border_color(c.line)
            .child(header)
            .child(
                div()
                    .id("comments-rows")
                    .max_h(px(MAX_ROWS_HEIGHT))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .automation_target("comments_list")
    }
}

impl CommentsList {
    fn render_row(
        &self,
        row: &CommentRow,
        focused: bool,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let hover = c.control_hover;
        let marker = match row.state {
            ThreadState::Open => div()
                .flex_none()
                .size(px(7.))
                .rounded_full()
                .border(px(1.5))
                .border_color(c.accent)
                .into_any_element(),
            ThreadState::Addressed => div()
                .flex_none()
                .size(px(7.))
                .rounded_full()
                .bg(c.blue)
                .into_any_element(),
            ThreadState::Resolved => icon(DvIcon::CircleCheck, px(11.), c.green).into_any_element(),
        };
        let (path, line) = match row.location.rsplit_once(':') {
            Some((path, line)) => (path.to_string(), format!(":{line}")),
            None => (row.location.clone(), String::new()),
        };
        let flag = |text: &'static str, color| {
            div()
                .flex_none()
                .ml_auto()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(text)
        };
        let id = row.id.clone();
        div()
            .id(ElementId::Name(format!("comment-row-{}", row.id).into()))
            .flex()
            .flex_col()
            .gap(px(2.))
            .w_full()
            .px(theme.spacing.md)
            .py(px(5.))
            .rounded(theme.radii.control)
            .cursor_pointer()
            .map(|this| {
                if focused {
                    this.bg(c.sel)
                } else {
                    this.hover(move |style| style.bg(hover))
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| this.reveal(&id, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.sm)
                    .min_w_0()
                    .font_family(theme.fonts.mono.clone())
                    .text_size(px(11.5))
                    .line_height(px(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c.fg2)
                    .child(marker)
                    // `path:line` reads as one: no gap between the two.
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            // At its start, so the file's name stays (DESIGN §4.1).
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis_start()
                                    // What truncation leaves over goes before the path, so
                                    // `:line` stays against it.
                                    .text_right()
                                    .child(path),
                            )
                            .child(div().flex_none().text_color(c.fg3).child(line)),
                    )
                    .when(row.outdated, |this| this.child(flag("outdated", c.yellow)))
                    .when(
                        row.state == ThreadState::Addressed && !row.outdated,
                        |this| this.child(flag("addressed", c.blue)),
                    ),
            )
            .child(
                div()
                    .pl(px(13.))
                    .truncate()
                    .text_spec(&theme.text.body)
                    .text_color(if row.state == ThreadState::Resolved {
                        c.fg3
                    } else {
                        c.fg
                    })
                    .child(row.summary.clone()),
            )
            .children(row.snippet.clone().map(|snippet| {
                div()
                    .pl(px(13.))
                    .truncate()
                    .font_family(theme.fonts.mono.clone())
                    .text_size(px(11.5))
                    .line_height(px(16.))
                    .text_color(c.fg3)
                    .child(snippet.replace('\t', "    "))
            }))
            .automation_target(format!("comments_list/{}", row.id))
            .into_any_element()
    }
}
