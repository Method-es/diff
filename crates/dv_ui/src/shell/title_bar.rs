//! The custom title bar (DESIGN §2, §4.1): 40px of `chrome` holding dv's mark, the review
//! switcher (`Ctrl O`), the centred command field (`Ctrl K`) and, on Windows, the caption
//! buttons. The whole bar is a window drag region; its controls occlude it so they stay
//! clickable.
//!
//! The caption buttons are gpui `WindowControlArea`s, so Windows itself handles their clicks,
//! hover and Snap Layouts on Maximize (the pattern of Zed's `platform_title_bar`). macOS draws
//! its traffic lights over the left edge, which the bar leaves room for; Linux draws its own
//! decorations.

use gpui::{
    AnyElement, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, RenderOnce,
    Role, StatefulInteractiveElement as _, Styled as _, Window, WindowControlArea, div,
    prelude::FluentBuilder as _, px,
};

use crate::automation::AutomationTargetExt as _;
use crate::components::{icon, kbd};
use crate::icons::DvIcon;
use crate::theme::{DvTheme, StyledText as _};

/// The title bar of the main window in its no-review state.
pub fn title_bar(theme: &DvTheme) -> AnyElement {
    let c = &theme.colors;
    let left_inset = if cfg!(target_os = "macos") {
        theme.sizes.traffic_lights_inset
    } else {
        px(14.)
    };
    div()
        .id("titlebar")
        .relative()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(theme.sizes.title_bar_height)
        .bg(c.chrome)
        .border_b_1()
        .border_color(c.line)
        .window_control_area(WindowControlArea::Drag)
        .child(mark(left_inset, theme))
        .child(div().flex_none().w(px(1.)).h(px(18.)).bg(c.line2))
        .child(switcher(theme))
        .child(div().ml(px(2.)).child(kbd("Ctrl O", theme)))
        .child(div().flex_1())
        .child(WindowControls::new(theme))
        // Centred on the window, not between its neighbours. The wrapper has no hitbox, so the
        // drag region still reaches everywhere but the field.
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(command_field(theme)),
        )
        .automation_target("titlebar")
}

/// dv's mark in the accent and the wordmark.
fn mark(left_inset: Pixels, theme: &DvTheme) -> AnyElement {
    let c = &theme.colors;
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(theme.spacing.md)
        .pl(left_inset)
        .pr(theme.spacing.lg)
        .child(icon(DvIcon::Mark, px(18.), c.accent))
        .child(
            div()
                .text_spec(&theme.text.wordmark)
                .text_color(c.fg)
                .child("dv"),
        )
        .automation_target("titlebar/mark")
}

/// The review switcher (`Ctrl O`, PLAN §11 `switcher`); it opens nothing yet.
fn switcher(theme: &DvTheme) -> AnyElement {
    let c = &theme.colors;
    let hover = c.control_hover;
    div()
        .id("titlebar-switcher")
        .role(Role::Button)
        .aria_label("Switch repository or review (Ctrl O)")
        .flex_none()
        .flex()
        .items_center()
        .gap(px(7.))
        .ml(theme.spacing.sm)
        .h(theme.sizes.control_height)
        .px(theme.spacing.md)
        .rounded(theme.radii.control)
        .text_spec(&theme.text.control)
        .text_color(c.fg2)
        // A control inside the drag region.
        .occlude()
        .hover(move |style| style.bg(hover))
        .child(icon(DvIcon::Folder, px(15.), c.fg2))
        .child("No review open")
        .child(icon(DvIcon::ChevronDown, px(12.), c.fg3))
        .automation_target("titlebar/switcher")
}

/// The command field (`Ctrl K`, PLAN §11 `palette`); it opens nothing yet.
fn command_field(theme: &DvTheme) -> AnyElement {
    let c = &theme.colors;
    div()
        .id("titlebar-command")
        .role(Role::Button)
        .aria_label("Go to file, review or command (Ctrl K)")
        .flex()
        .items_center()
        .gap(theme.spacing.md)
        .w(theme.sizes.command_field_width)
        .h(theme.sizes.control_height)
        .pl(px(10.))
        .pr(px(5.))
        .rounded(theme.radii.control)
        .border_1()
        .border_color(c.ctrl_line)
        .bg(c.input_bg)
        .text_spec(&theme.text.body)
        .text_color(c.fg3)
        .occlude()
        .child(icon(DvIcon::Search, px(14.), c.fg3))
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .child("Go to file, review or command"),
        )
        .child(kbd("Ctrl K", theme))
        .automation_target("titlebar/command")
}

/// Minimize, maximize/restore and close: 46 wide and the title bar's height. Nothing on macOS
/// and Linux, where the platform draws its own, or in fullscreen.
#[derive(IntoElement)]
struct WindowControls {
    glyph: Hsla,
    hover: Hsla,
    close_hover: Hsla,
    close_glyph: Hsla,
    width: Pixels,
    height: Pixels,
}

impl WindowControls {
    fn new(theme: &DvTheme) -> Self {
        let c = &theme.colors;
        Self {
            glyph: c.fg2,
            hover: c.control_hover,
            close_hover: c.caption_close_hover,
            close_glyph: c.caption_close_fg,
            width: theme.sizes.caption_button_width,
            height: theme.sizes.title_bar_height,
        }
    }

    fn button(&self, button: CaptionButton) -> AnyElement {
        let (hover_bg, hover_glyph) = match button {
            CaptionButton::Close => (self.close_hover, self.close_glyph),
            _ => (self.hover, self.glyph),
        };
        let group = button.id();
        div()
            .id(button.id())
            .role(Role::Button)
            .aria_label(button.label())
            .group(group)
            .flex()
            .items_center()
            .justify_center()
            .w(self.width)
            .h(self.height)
            // Keep the drag region from claiming the hit test.
            .occlude()
            .window_control_area(button.control_area())
            .hover(move |style| style.bg(hover_bg))
            .active(move |style| style.bg(hover_bg.opacity(0.8)))
            .child(
                gpui::svg()
                    .path(button.icon().path())
                    .size(px(10.))
                    .text_color(self.glyph)
                    .group_hover(group, move |style| style.text_color(hover_glyph)),
            )
            .automation_target(button.target())
    }
}

impl RenderOnce for WindowControls {
    fn render(self, window: &mut Window, _cx: &mut gpui::App) -> impl IntoElement {
        let visible = cfg!(target_os = "windows") && !window.is_fullscreen();
        let maximize = if window.is_maximized() {
            CaptionButton::Restore
        } else {
            CaptionButton::Maximize
        };
        div()
            .id("window-controls")
            .flex_none()
            .flex()
            .flex_row()
            .when(visible, |this| {
                this.child(self.button(CaptionButton::Minimize))
                    .child(self.button(maximize))
                    .child(self.button(CaptionButton::Close))
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptionButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl CaptionButton {
    fn id(self) -> &'static str {
        match self {
            Self::Minimize => "caption-minimize",
            Self::Maximize => "caption-maximize",
            Self::Restore => "caption-restore",
            Self::Close => "caption-close",
        }
    }

    fn icon(self) -> DvIcon {
        match self {
            Self::Minimize => DvIcon::WindowMinimize,
            Self::Maximize => DvIcon::WindowMaximize,
            Self::Restore => DvIcon::WindowRestore,
            Self::Close => DvIcon::WindowClose,
        }
    }

    /// The automation target. Maximize and Restore are one button in two states.
    fn target(self) -> &'static str {
        match self {
            Self::Minimize => "titlebar/min",
            Self::Maximize | Self::Restore => "titlebar/max",
            Self::Close => "titlebar/close",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Minimize => "Minimize",
            Self::Maximize => "Maximize",
            Self::Restore => "Restore",
            Self::Close => "Close",
        }
    }

    fn control_area(self) -> WindowControlArea {
        match self {
            Self::Minimize => WindowControlArea::Min,
            Self::Maximize | Self::Restore => WindowControlArea::Max,
            Self::Close => WindowControlArea::Close,
        }
    }
}
