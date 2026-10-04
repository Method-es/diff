//! The shell's layout (DESIGN §2): title bar, then the panel and the main area side by side, then
//! the status bar, each on its surface token (DESIGN §3).

use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use super::{EmptyState, Shell, title_bar};
use crate::automation::AutomationTargetExt as _;
use crate::components::{inline_code, kbd};
use crate::theme::{ActiveTheme as _, DvTheme, StyledText as _};

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.dv_theme().clone();
        let c = &theme.colors;
        div()
            .id("shell")
            .key_context("Shell")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(c.window_background)
            .text_color(c.fg)
            .text_spec(&theme.text.body)
            // First, so its capture listener runs before any other.
            .child(crate::automation::pointer_guard())
            .child(title_bar::title_bar(&theme))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .when(self.panel_visible(), |this| this.child(panel(&theme)))
                    .child(main(self.empty_state(), &theme)),
            )
            .child(status_bar(&theme))
    }
}

/// The left panel (DESIGN §2, §4.1): "Changes" over the file list; with no review open, only
/// the header and a quiet line.
fn panel(theme: &DvTheme) -> AnyElement {
    let c = &theme.colors;
    div()
        .flex_none()
        .flex()
        .flex_col()
        .w(theme.sizes.panel_width)
        .h_full()
        .bg(c.panel)
        .border_r_1()
        .border_color(c.line)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(theme.spacing.xs)
                .pt(theme.spacing.lg)
                .px(theme.spacing.lg)
                .pb(px(10.))
                .child(
                    div().flex().items_center().gap(theme.spacing.md).child(
                        div()
                            .text_spec(&theme.text.title)
                            .text_color(c.fg)
                            .child("Changes"),
                    ),
                )
                .child(
                    div()
                        .text_spec(&theme.text.secondary)
                        .text_color(c.fg3)
                        .child("No review open"),
                )
                .automation_target("shell/panel/header"),
        )
        .automation_target("shell/panel")
}

/// The main area on the `code` surface, with DESIGN §4.4's empty state.
fn main(empty: Option<EmptyState>, theme: &DvTheme) -> AnyElement {
    let c = &theme.colors;
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(c.code)
        .children(empty.map(|empty| match empty {
            EmptyState::NoReview => no_review(theme),
        }))
        .automation_target("shell/main")
}

/// "Pick a repo to start a review", and how.
fn no_review(theme: &DvTheme) -> AnyElement {
    let c = &theme.colors;
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(theme.spacing.md)
        .child(
            div()
                .text_spec(&theme.text.heading)
                .text_color(c.fg)
                .child("Pick a repo to start a review"),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.spacing.sm)
                .text_color(c.fg2)
                .child("Open a folder with")
                // The comma hugs the keycap.
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(kbd("Ctrl O", theme))
                        .child(","),
                )
                .child("or run")
                .child(inline_code("dv", theme))
                .child("inside a repo"),
        )
        .automation_target("shell/main/empty")
}

/// The status bar (DESIGN §2, §4.1): the repo and its location on the left, the material on the
/// right.
fn status_bar(theme: &DvTheme) -> AnyElement {
    let c = &theme.colors;
    div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(14.))
        .h(theme.sizes.status_bar_height)
        .px(theme.spacing.lg)
        .bg(c.chrome)
        .border_t_1()
        .border_color(c.line)
        .text_spec(&theme.text.secondary)
        .text_color(c.fg2)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.spacing.sm)
                .child(div().size(px(7.)).rounded_full().bg(c.fg3))
                .child("No repo")
                .automation_target("statusbar/repo"),
        )
        .child(div().flex_1())
        .child(
            div()
                .child(theme.material.label())
                .automation_target("statusbar/material"),
        )
        .automation_target("statusbar")
}
