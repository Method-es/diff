//! The comments section's layout (the board's "Open comments" block under the files: a header
//! with the count and "Copy as prompt", then `listBtn` rows with a dot, `path:line` in mono and
//! the comment's first line), and a row's right-click menu, in the file menu's style.

use dv_core::review::ThreadState;
use gpui::{
    AnyElement, Context, ElementId, FontWeight, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
    Window, anchored, deferred, div, prelude::FluentBuilder as _, px,
};

use super::{CommentRow, CommentsList};
use crate::automation::AutomationTargetExt as _;
use crate::components::{
    count_badge, icon, menu_item, menu_panel, small_button, small_icon_button, tooltip,
};
use crate::icons::DvIcon;
use crate::services::ActiveServices as _;
use crate::theme::{ActiveTheme as _, DvTheme, StyledText as _};

/// The tallest the list of rows grows before it scrolls.
const MAX_ROWS_HEIGHT: f32 = 240.;

/// The narrowest panel (DESIGN §2: 220–420) whose comments header has room for "Copy as
/// prompt" beside "Open comments" and a three-digit count.
const COPY_LABEL_FROM: f32 = 270.;

/// The row menu's paint order: over the panel and the stream, as the file menu's.
const MENU_PRIORITY: usize = 2;

impl Render for CommentsList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.dv_theme().clone();
        let c = &theme.colors;
        let Some(session) = self.session.clone() else {
            return div().into_any_element();
        };
        let all = super::rows(session.read(cx), true, true);
        let count = |state: ThreadState| all.iter().filter(|row| row.state == state).count();
        let waiting = all.iter().filter(|row| row.state.is_waiting()).count();
        let open = count(ThreadState::Open);
        let resolved = count(ThreadState::Resolved);
        let annotations = count(ThreadState::Annotation);
        let focused = self.diff_pane.read(cx).focused_thread().cloned();
        let (waiting_rows, rest): (Vec<&CommentRow>, Vec<&CommentRow>) =
            all.iter().partition(|row| row.state.is_waiting());
        let annotation_rows: Vec<&CommentRow> = rest
            .iter()
            .copied()
            .filter(|row| self.show_annotations && row.state == ThreadState::Annotation)
            .collect();
        let resolved_rows: Vec<&CommentRow> = rest
            .iter()
            .copied()
            .filter(|row| self.show_resolved && row.state == ThreadState::Resolved)
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
        let mut rows: Vec<AnyElement> = waiting_rows
            .iter()
            .map(|row| self.render_row(row, focused.as_ref() == Some(&row.id), &theme, cx))
            .collect();
        if waiting == 0 {
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
        if annotations > 0 {
            let hover = c.control_hover;
            let show = self.show_annotations;
            rows.push(
                div()
                    .id("comments-annotations")
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
                    .child(annotation_marker(&theme))
                    .child(format!(
                        "{annotations} {} · {}",
                        if annotations == 1 {
                            "annotation"
                        } else {
                            "annotations"
                        },
                        if show { "hide" } else { "show" }
                    ))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.set_show_annotations(!show, cx)),
                    )
                    .automation_target("comments_list/annotations")
                    .into_any_element(),
            );
            rows.extend(
                annotation_rows
                    .iter()
                    .map(|row| self.render_row(row, focused.as_ref() == Some(&row.id), &theme, cx)),
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
            rows.extend(
                resolved_rows
                    .iter()
                    .map(|row| self.render_row(row, focused.as_ref() == Some(&row.id), &theme, cx)),
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
            .children(self.render_menu(&theme, cx))
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
            ThreadState::Annotation => annotation_marker(theme),
        };
        // `path` and, quieter after it, `:line` or a comment on the whole file's ` (file)`.
        let split = if row.file_level {
            row.location
                .rfind(" (file")
                .map(|at| row.location.split_at(at))
        } else {
            row.location.rfind(':').map(|at| row.location.split_at(at))
        };
        let (path, line) = match split {
            Some((path, line)) => (path.to_string(), line.to_string()),
            None => (row.location.clone(), String::new()),
        };
        let flag = |text: &'static str, color, weight| {
            div()
                .flex_none()
                .text_size(px(11.))
                .font_weight(weight)
                .text_color(color)
                .child(text)
        };
        // At the row's end: outdated or addressed, and a quiet "all changes" for a thread the
        // interdiff doesn't show (DESIGN §4.1).
        let mut flags = Vec::new();
        if row.outdated {
            flags.push(flag("outdated", c.yellow, FontWeight::MEDIUM));
        } else if row.state == ThreadState::Addressed {
            flags.push(flag("addressed", c.blue, FontWeight::MEDIUM));
        } else if row.state == ThreadState::Annotation && row.unlisted {
            flags.push(flag("unchanged file", c.fg3, FontWeight::NORMAL));
        }
        if row.not_in_comparison {
            flags.push(flag("all changes", c.fg3, FontWeight::NORMAL));
        }
        let id = row.id.clone();
        // The row whose menu is open keeps its hover tint while the pointer is on the menu.
        let menu_open = self.menu_thread() == Some(&row.id);
        // An annotation the stream never draws has no menu: there is no card to act on.
        let has_menu = row.drawable;
        // Its whole text: the explanation and the agent's additions to it.
        let expanded = (self.expanded.as_ref() == Some(&row.id)
            && row.state == ThreadState::Annotation
            && !row.drawable)
            .then(|| {
                self.session
                    .as_ref()
                    .and_then(|session| session.read(cx).review().thread(&row.id))
                    .map(|thread| {
                        let mut text = thread.body.clone();
                        for reply in thread.replies.iter().filter(|reply| !reply.is_deleted()) {
                            text.push_str("\n\n");
                            text.push_str(&reply.body);
                        }
                        text
                    })
            })
            .flatten();
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
                } else if menu_open {
                    this.bg(hover)
                } else {
                    this.hover(move |style| style.bg(hover))
                }
            })
            .on_click({
                let id = id.clone();
                cx.listener(move |this, _, _, cx| this.reveal(&id, cx))
            })
            .when(has_menu, |this| {
                this.on_mouse_down(MouseButton::Right, {
                    let id = id.clone();
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.open_menu(id.clone(), event.position, cx);
                        cx.stop_propagation();
                    })
                })
            })
            .debug_selector(move || format!("comment-row-{id}"))
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
                    // Not placed yet: the line it was written on, quietly (module docs).
                    .text_color(if row.pending { c.fg3 } else { c.fg2 })
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
                    .when(!flags.is_empty(), |this| {
                        this.child(
                            div()
                                .flex_none()
                                .ml_auto()
                                .flex()
                                .items_center()
                                .gap(theme.spacing.sm)
                                .children(flags),
                        )
                    }),
            )
            .child(
                div()
                    .pl(px(13.))
                    .truncate()
                    .text_spec(&theme.text.body)
                    .text_color(match row.state {
                        ThreadState::Resolved => c.fg3,
                        ThreadState::Annotation => c.fg2,
                        ThreadState::Open | ThreadState::Addressed => c.fg,
                    })
                    .child(row.summary.clone()),
            )
            .children(expanded.map(|body| {
                div()
                    .pl(px(13.))
                    .pt(px(2.))
                    .child(
                        crate::markdown::view(
                            ElementId::Name(format!("comment-row-{}/body", row.id).into()),
                            &body,
                            cx,
                        )
                        .text_spec(&theme.text.secondary)
                        .text_color(c.fg2),
                    )
                    .automation_target(format!("comments_list/{}/body", row.id))
            }))
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

    /// The open row menu, at the pointer where it was opened (module docs): the card's actions
    /// for the thread's state, Reply, and Copy as prompt.
    fn render_menu(&self, theme: &DvTheme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.clone()?;
        let found = self.session.as_ref().and_then(|session| {
            let session = session.read(cx);
            let thread = session
                .review()
                .thread(&menu.thread)
                .filter(|thread| !thread.is_deleted())?;
            Some((thread.state, session.is_read_only()))
        });
        // A thread gone meanwhile shows no menu; the session's change closes it
        // (`CommentsList::close_stale_menu`).
        let (state, read_only) = found?;
        let c = &theme.colors;
        let writable = !read_only;
        if state == ThreadState::Annotation {
            return Some(self.render_annotation_menu(menu, writable, theme, cx));
        }
        let states: &[(&'static str, &'static str, DvIcon, ThreadState)] = match state {
            ThreadState::Annotation => &[],
            ThreadState::Open => &[(
                "resolve",
                "Resolve",
                DvIcon::CircleCheck,
                ThreadState::Resolved,
            )],
            ThreadState::Addressed => &[
                (
                    "resolve",
                    "Resolve",
                    DvIcon::CircleCheck,
                    ThreadState::Resolved,
                ),
                ("reopen", "Reopen", DvIcon::RotateCw, ThreadState::Open),
            ],
            ThreadState::Resolved => &[("reopen", "Reopen", DvIcon::RotateCw, ThreadState::Open)],
        };
        let item = |key: &'static str, label: &'static str, glyph: DvIcon, enabled: bool| {
            menu_item(
                ElementId::Name(format!("comments-menu-{key}").into()),
                label,
                glyph,
                enabled,
                theme,
            )
            .debug_selector(move || format!("comments-menu-{key}"))
        };
        let mut items: Vec<AnyElement> = states
            .iter()
            .map(|&(key, label, glyph, state)| {
                let thread = menu.thread.clone();
                item(key, label, glyph, writable)
                    .when(writable, |this| {
                        this.on_click(cx.listener(move |this, _, _, cx| {
                            this.set_state(&thread, state, cx);
                        }))
                    })
                    .automation_target(format!("comments_list/menu/{key}"))
            })
            .collect();
        let thread = menu.thread.clone();
        items.push(
            item("reply", "Reply", DvIcon::MessageSquare, writable)
                .when(writable, |this| {
                    this.on_click(cx.listener(move |this, _, window, cx| {
                        this.reply(&thread, window, cx);
                    }))
                })
                .automation_target("comments_list/menu/reply"),
        );
        items.push(
            div()
                .h(px(1.))
                .my(theme.spacing.xs)
                .bg(c.line)
                .into_any_element(),
        );
        let thread = menu.thread.clone();
        items.push(
            item("copy", "Copy as prompt", DvIcon::Copy, true)
                .on_click(cx.listener(move |this, _, _, cx| this.copy_thread(&thread, cx)))
                .automation_target("comments_list/menu/copy"),
        );
        let panel = menu_panel("comments-menu", theme)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menu(cx)))
            .children(items)
            .automation_target("comments_list/menu");
        Some(
            deferred(
                anchored()
                    .position(menu.position)
                    .snap_to_window_with_margin(px(8.))
                    .child(panel),
            )
            .with_priority(MENU_PRIORITY)
            .into_any_element(),
        )
    }

    /// An annotation's row menu (PLAN §19.3): Ask alone, disabled on a finished review; no
    /// Resolve (nothing waits on it) and no Copy as prompt.
    fn render_annotation_menu(
        &self,
        menu: super::RowMenu,
        writable: bool,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let thread = menu.thread.clone();
        let ask = menu_item(
            ElementId::Name("comments-menu-ask".into()),
            "Ask",
            DvIcon::MessageSquare,
            writable,
            theme,
        )
        .debug_selector(|| "comments-menu-ask".into())
        .when(writable, |this| {
            this.on_click(cx.listener(move |this, _, window, cx| {
                this.reply(&thread, window, cx);
            }))
        })
        .automation_target("comments_list/menu/ask");
        let panel = menu_panel("comments-menu", theme)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menu(cx)))
            .child(ask)
            .automation_target("comments_list/menu");
        deferred(
            anchored()
                .position(menu.position)
                .snap_to_window_with_margin(px(8.))
                .child(panel),
        )
        .with_priority(MENU_PRIORITY)
        .into_any_element()
    }
}

/// An annotation's mark in the list: a 6-square in `blue` (DESIGN §4.1).
fn annotation_marker(theme: &DvTheme) -> AnyElement {
    div()
        .flex_none()
        .size(px(6.))
        .rounded(px(1.5))
        .bg(theme.colors.blue)
        .into_any_element()
}
