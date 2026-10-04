//! Small building blocks shared by the views, drawn from the theme's tokens.

use gpui::{Hsla, IntoElement, ParentElement as _, Pixels, SharedString, Styled as _, div, svg};

use crate::icons::DvIcon;
use crate::theme::{DvTheme, StyledText as _};

/// A keycap such as `Ctrl O` (DESIGN §5 "kbd": 11px mono, `line2` border, `kbd_bg` fill).
pub fn kbd(label: impl Into<SharedString>, theme: &DvTheme) -> impl IntoElement {
    let c = &theme.colors;
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .h(theme.sizes.kbd_height)
        .min_w(theme.sizes.kbd_height)
        .px(theme.spacing.xs + gpui::px(1.))
        .rounded(theme.radii.small)
        .border_1()
        .border_color(c.line2)
        .bg(c.kbd_bg)
        .text_color(c.fg2)
        .text_spec(&theme.text.kbd)
        .whitespace_nowrap()
        .child(label.into())
}

/// Inline code such as `dv` in running text (the canvas's `icode`).
pub fn inline_code(text: impl Into<SharedString>, theme: &DvTheme) -> impl IntoElement {
    let c = &theme.colors;
    div()
        .flex_none()
        .px(gpui::px(5.))
        .py(gpui::px(2.))
        .rounded(theme.radii.small)
        .bg(c.icode_bg)
        .text_color(c.fg)
        .text_spec(&theme.text.inline_code)
        .whitespace_nowrap()
        .child(text.into())
}

/// `icon` at `size`, in `color`.
pub fn icon(icon: DvIcon, size: Pixels, color: Hsla) -> impl IntoElement {
    svg()
        .flex_none()
        .path(icon.path())
        .size(size)
        .text_color(color)
}
