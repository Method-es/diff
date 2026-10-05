//! The shell's layout (DESIGN §2): title bar, then the panel and the main area side by side, then
//! the status bar, each on its surface token (DESIGN §3), and the scrim under an open overlay.

use gpui::{
    AnyElement, AppContext as _, Context, DragMoveEvent, InteractiveElement as _, IntoElement,
    MouseButton, MouseUpEvent, ParentElement as _, Render, StatefulInteractiveElement as _,
    Styled as _, Window, deferred, div, prelude::FluentBuilder as _, px,
};

use dv_core::review::ReviewStatus;

use super::{EmptyState, Shell};
use crate::automation::AutomationTargetExt as _;
use crate::banner::{Banner, BannerAction};
use crate::components::{icon, inline_code, kbd, skeleton, small_button, status_label};
use crate::diff_pane::DiffPane;
use crate::icons::DvIcon;
use crate::keymap;
use crate::services::ActiveServices as _;
use crate::services::reviews::OpenRequest;
use crate::services::settings::PANEL_WIDTH_DEFAULT;
use crate::theme::{ActiveTheme as _, DvTheme, StyledText as _};

/// Deferred paint order: the panel's resize handle over its neighbour, the scrim over the
/// window, the sheet over the scrim, and a menu (the switcher, the source picker) over
/// everything. A menu open over the sheet has the scrim over the sheet too.
pub(super) const HANDLE_PRIORITY: usize = 0;
pub(super) const SCRIM_PRIORITY: usize = 1;
pub(super) const SHEET_PRIORITY: usize = 2;
pub(super) const SCRIM_OVER_SHEET_PRIORITY: usize = 3;
pub(super) const MENU_PRIORITY: usize = 4;

/// The drag payload of the panel's resize handle (Zed's `DraggedDock` pattern).
#[derive(Clone, Copy, Debug)]
pub(super) struct DraggedPanelEdge;

impl Render for DraggedPanelEdge {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.dv_theme().clone();
        let c = &theme.colors;
        let overlay_open =
            self.switcher.is_some() || self.source_picker.is_some() || self.sheet.is_some();
        div()
            .id("shell")
            .key_context(keymap::SHELL)
            .on_action(cx.listener(Self::toggle_switcher))
            .on_action(cx.listener(Self::toggle_panel))
            .on_action(cx.listener(Self::finish_review))
            .on_drag_move(cx.listener(Self::resize_panel))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(c.window_background)
            .text_color(c.fg)
            .text_spec(&theme.text.body)
            // First, so its capture listener runs before any other.
            .child(crate::automation::pointer_guard())
            .child(self.render_title_bar(window, &theme, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .when(self.panel_visible, |this| {
                        this.child(self.render_panel(&theme, cx))
                    })
                    .child(self.render_main(&theme, cx)),
            )
            .child(self.render_status_bar(&theme, cx))
            .when(overlay_open, |this| {
                this.child(self.render_scrim(&theme, cx))
            })
            .children(self.render_sheet(window, &theme))
    }
}

impl Shell {
    /// The panel (DESIGN §2, §4.1): the file list, and the handle on its right edge.
    fn render_panel(&self, theme: &DvTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let width = cx.services().settings.read(cx).panel_width();
        div()
            .relative()
            .flex_none()
            .flex()
            .flex_col()
            .w(px(width))
            .h_full()
            .bg(c.panel)
            .border_r_1()
            .border_color(c.line)
            .debug_selector(|| "panel".into())
            .child(self.file_list.clone())
            .child(self.comments_list.clone())
            .child(self.render_resize_handle(theme, cx))
            .automation_target("shell/panel")
    }

    /// The panel's right edge: dragging it resizes the panel, a double click restores 280.
    fn render_resize_handle(&self, theme: &DvTheme, cx: &mut Context<Self>) -> AnyElement {
        let line = theme.colors.accent_ring;
        let id = "panel-resize-handle";
        deferred(
            div()
                .id(id)
                .debug_selector(move || id.into())
                .group(id)
                .absolute()
                .top_0()
                .right(px(-4.))
                .h_full()
                .w(px(8.))
                .flex()
                .justify_center()
                .cursor_col_resize()
                .occlude()
                .child(
                    div()
                        .w(px(2.))
                        .h_full()
                        .group_hover(id, move |style| style.bg(line)),
                )
                .on_drag(DraggedPanelEdge, |edge, _, _, cx| {
                    cx.stop_propagation();
                    cx.new(|_| *edge)
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|_, event: &MouseUpEvent, _, cx| {
                        if event.click_count == 2 {
                            let settings = cx.services().settings.clone();
                            settings.update(cx, |settings, cx| {
                                settings.set_panel_width(PANEL_WIDTH_DEFAULT, cx)
                            });
                            cx.stop_propagation();
                        }
                    }),
                )
                .automation_target("shell/panel/resize"),
        )
        .with_priority(HANDLE_PRIORITY)
        .into_any_element()
    }

    fn resize_panel(
        &mut self,
        event: &DragMoveEvent<DraggedPanelEdge>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let width = f32::from(event.event.position.x - event.bounds.left());
        let settings = cx.services().settings.clone();
        settings.update(cx, |settings, cx| settings.set_panel_width(width, cx));
    }

    /// The main area on the `code` surface: a banner when opening failed, then the stream, a
    /// loading state or DESIGN §4.4's empty states. It always renders and holds the keyboard
    /// whenever no overlay or text field does (the diff pane's focus handle and key context),
    /// so the stream's keys reach the pane whatever the area shows.
    fn render_main(&self, theme: &DvTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let banner = self.banner.as_ref().map(|banner| {
            let expanded = banner.expanded;
            let mut view = Banner::new("banner/main", banner.failure.clone()).disclosure(
                expanded,
                handler(cx, |this, _, cx| {
                    if let Some(banner) = &mut this.banner {
                        banner.expanded = !banner.expanded;
                        cx.notify();
                    }
                }),
            );
            if banner.retry.is_some() {
                view = view.action(BannerAction::new(
                    "retry",
                    "Retry",
                    handler(cx, |this, window, cx| this.retry(window, cx)),
                ));
            }
            view.action(BannerAction::new(
                "dismiss",
                "Dismiss",
                handler(cx, |this, _, cx| {
                    this.banner = None;
                    cx.notify();
                }),
            ))
        });
        let session_banner = self.session.as_ref().and_then(|session| {
            let failure = session.read(cx).error()?.clone();
            let session = session.clone();
            Some(
                Banner::new("banner/session", failure)
                    .disclosure(
                        self.session_banner_expanded,
                        handler(cx, |this, _, cx| {
                            this.session_banner_expanded = !this.session_banner_expanded;
                            cx.notify();
                        }),
                    )
                    .action(BannerAction::new("retry", "Retry", move |_, cx| {
                        session.update(cx, |session, cx| session.rescan(cx))
                    })),
            )
        });
        // A viewed toggle that didn't land: its checkbox can't say why (DESIGN §4.4).
        let viewed_banner = self.session.as_ref().and_then(|session| {
            let failure = session.read(cx).viewed_error()?.clone();
            let session = session.clone();
            Some(
                Banner::new("banner/viewed", failure).action(BannerAction::new(
                    "dismiss",
                    "Dismiss",
                    move |_, cx| session.update(cx, |session, cx| session.dismiss_viewed_error(cx)),
                )),
            )
        });
        let finished = self
            .session
            .as_ref()
            .map(|session| session.read(cx).review().status)
            .filter(|status| status.is_final())
            .map(|status| finished_banner(status, theme, cx));
        // The first listing only: a refresh keeps the stream (or the empty state) on screen, and
        // so does opening the review that is already open (it stays as it is).
        let reopening = match (&self.opening, &self.session) {
            (Some(opening), Some(session)) => {
                let session = session.read(cx);
                session.backend().repo().location == opening.location
                    && match &opening.request {
                        OpenRequest::Review(id) => session.review_id() == id,
                        OpenRequest::Target(target) => {
                            !session.review().status.is_final()
                                && &session.review().target == target
                        }
                    }
            }
            _ => false,
        };
        let loading = (self.opening.is_some() && !reopening)
            || self
                .session
                .as_ref()
                .is_some_and(|session| session.read(cx).is_first_load());
        let content: AnyElement = match self.empty_state(cx) {
            Some(empty) => empty_view(empty, theme),
            None if loading => loading_view(self.opening.is_some(), theme),
            None if self.session.is_some() => div()
                .flex_1()
                .min_h_0()
                .child(self.diff_pane.clone())
                .into_any_element(),
            None => div().flex_1().into_any_element(),
        };
        let main = div()
            .id("main")
            .key_context(keymap::DIFF_PANE)
            .track_focus(&self.focus_handle);
        DiffPane::bind_actions(main, &self.diff_pane)
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(c.code)
            .debug_selector(|| "main".into())
            .children(banner)
            .children(session_banner)
            .children(viewed_banner)
            .children(finished)
            .child(content)
            .automation_target("shell/main")
    }

    /// The "Finish round" sheet, centred over the scrim (DESIGN §4.2).
    fn render_sheet(&self, window: &Window, theme: &DvTheme) -> Option<AnyElement> {
        let sheet = self.sheet.as_ref()?;
        let top = crate::submit::sheet_top(window.viewport_size().height);
        Some(
            deferred(
                div()
                    .absolute()
                    .top(theme.sizes.title_bar_height)
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(top)
                    .child(sheet.view.clone()),
            )
            .with_priority(SHEET_PRIORITY)
            .into_any_element(),
        )
    }

    /// The scrim under the top overlay (DESIGN §3): below the title bar, taking the mouse from
    /// everything under it (a sheet with a menu open over it too); a click on it closes that
    /// overlay.
    fn render_scrim(&self, theme: &DvTheme, cx: &mut Context<Self>) -> AnyElement {
        let menu_open = self.switcher.is_some() || self.source_picker.is_some();
        let priority = if menu_open && self.sheet.is_some() {
            SCRIM_OVER_SHEET_PRIORITY
        } else {
            SCRIM_PRIORITY
        };
        deferred(
            div()
                .id("scrim")
                .debug_selector(|| "scrim".into())
                .absolute()
                .top(theme.sizes.title_bar_height)
                .left_0()
                .right_0()
                .bottom_0()
                .bg(theme.colors.scrim)
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.close_top_overlay(window, cx);
                        cx.stop_propagation();
                    }),
                )
                .automation_target("shell/scrim"),
        )
        .with_priority(priority)
        .into_any_element()
    }
}

/// A handler for a stateless element (a banner's buttons) that runs `f` on the shell.
fn handler(
    cx: &Context<Shell>,
    f: impl Fn(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
) -> impl Fn(&mut Window, &mut gpui::App) + 'static {
    let this = cx.weak_entity();
    move |window, cx| {
        this.update(cx, |this, cx| f(this, window, cx)).ok();
    }
}

/// The banner over a finished review (DESIGN §4.4 "Read-only"): "Approved · this review is
/// finished", with "Start a new review". Its composer and thread actions stay, disabled.
fn finished_banner(status: ReviewStatus, theme: &DvTheme, cx: &mut Context<Shell>) -> AnyElement {
    let c = &theme.colors;
    let approved = status == ReviewStatus::Approved;
    let (glyph, color, fill) = if approved {
        (DvIcon::CircleCheck, c.green, c.green_soft)
    } else {
        (DvIcon::CircleMinus, c.fg2, c.layer)
    };
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(theme.spacing.md)
        .m(theme.spacing.lg)
        .min_h(px(40.))
        .py(theme.spacing.sm)
        .pl(theme.spacing.lg)
        .pr(theme.spacing.md)
        .rounded(theme.radii.control)
        .border_1()
        .border_color(color.opacity(0.35))
        .bg(fill)
        .child(icon(glyph, px(15.), color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(format!(
                    "{} · this review is finished",
                    status_label(status)
                )),
        )
        .child(
            small_button("start-new-review", "Start a new review", theme)
                .on_click(cx.listener(|this, _, window, cx| this.start_new_review(window, cx)))
                .automation_target("banner/finished/new_review"),
        )
        .automation_target("banner/finished")
}

/// The main area while a review opens or lists its files (DESIGN §4.4): a quiet progress line
/// where the file header goes, over skeleton rows, never a spinner.
fn loading_view(opening: bool, theme: &DvTheme) -> AnyElement {
    let c = &theme.colors;
    div()
        .flex_1()
        .flex()
        .flex_col()
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .h(px(44.))
                .pl(theme.spacing.xl)
                .bg(c.code_head)
                .border_b_1()
                .border_color(c.line)
                .text_spec(&theme.text.secondary)
                .text_color(c.fg3)
                .child(if opening {
                    "Opening the review…"
                } else {
                    "Listing changes…"
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(theme.spacing.md)
                .p(theme.spacing.xl)
                .children(
                    [420., 360., 500., 280., 440., 320.]
                        .into_iter()
                        .map(|width| skeleton(px(width), px(10.), theme)),
                ),
        )
        .automation_target("shell/main/loading")
}

/// DESIGN §4.4's empty states.
fn empty_view(empty: EmptyState, theme: &DvTheme) -> AnyElement {
    let c = &theme.colors;
    let centred = div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(theme.spacing.md);
    let heading = |text: &'static str| {
        div()
            .text_spec(&theme.text.heading)
            .text_color(c.fg)
            .child(text)
    };
    let note = |text: &'static str| div().text_color(c.fg2).child(text);
    match empty {
        EmptyState::NoReview => centred
            .child(heading("Pick a repo to start a review"))
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
                            .child(kbd(keymap::label("secondary-o"), theme))
                            .child(","),
                    )
                    .child("or run")
                    .child(inline_code("dv", theme))
                    .child("inside a repo"),
            ),
        EmptyState::WorkingTreeClean => centred
            .child(heading("Working tree clean — nothing to review"))
            .child(note("The review picks up changes as soon as files change.")),
        EmptyState::NoChanges => centred
            .child(heading("No changes — nothing to review"))
            .child(note("The two sides of this review are the same.")),
    }
    .automation_target("shell/main/empty")
}
