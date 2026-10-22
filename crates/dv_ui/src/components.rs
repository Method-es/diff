//! Small building blocks shared by the views, drawn from the theme's tokens.

use dv_core::review::ReviewStatus;
use gpui::{
    AnyElement, Div, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, SharedString, Stateful, Styled as _, div, prelude::FluentBuilder as _, px, svg,
};

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

/// A keycap on the accent fill of a primary button (the canvas's `kbdA`): the button's text
/// colour over a faint wash of it.
pub fn kbd_on_accent(label: impl Into<SharedString>, theme: &DvTheme) -> impl IntoElement {
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
        .border_color(c.accent_fg.opacity(0.3))
        .bg(c.accent_fg.opacity(0.14))
        .text_color(c.accent_fg)
        .text_spec(&theme.text.kbd)
        .whitespace_nowrap()
        .child(label.into())
}

/// The primary button (the canvas's `primary`: 28 high, the accent fill and edge, 13/600 in
/// `accentFg`). A disabled one fades and takes no clicks; give an enabled one an `on_click`.
pub fn primary_button(
    id: impl Into<gpui::ElementId>,
    enabled: bool,
    theme: &DvTheme,
) -> Stateful<Div> {
    let c = &theme.colors;
    let hover = c.accent_edge;
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .gap(px(7.))
        .h(theme.sizes.control_height)
        .px(px(10.))
        .rounded(theme.radii.control)
        .border_1()
        .border_color(c.accent_edge)
        .bg(c.accent)
        .text_color(c.accent_fg)
        .text_spec(&theme.text.control)
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .map(|this| {
            if enabled {
                this.cursor_pointer()
                    .shadow(theme.shadows.control.clone())
                    .hover(move |style| style.bg(hover))
            } else {
                this.opacity(0.45).cursor_default()
            }
        })
}

/// The outlined button (the canvas's `btn`: 28 high, `ctrl` fill, `ctrlLine` outline, 13/500).
/// Give it an `on_click`.
pub fn button(id: impl Into<gpui::ElementId>, theme: &DvTheme) -> Stateful<Div> {
    let c = &theme.colors;
    let hover = c.layer_strong;
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .gap(px(7.))
        .h(theme.sizes.control_height)
        .px(px(10.))
        .rounded(theme.radii.control)
        .border_1()
        .border_color(c.ctrl_line)
        .bg(c.ctrl)
        .shadow(theme.shadows.control.clone())
        .text_spec(&theme.text.control)
        .text_color(c.fg)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
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

/// A chip: a short label on a soft fill of its hue (the canvas's `envChip`, `modPill`; 20 high,
/// radius 5). Children go in by the caller.
pub fn chip(fg: Hsla, bg: Hsla, theme: &DvTheme) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(theme.spacing.xs)
        .h(px(20.))
        .px(theme.spacing.sm)
        .rounded(theme.radii.chip)
        .bg(bg)
        .text_color(fg)
        .text_spec(&theme.text.secondary)
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
}

/// The location chip of a repo outside Windows (DESIGN §4.1: `Ubuntu`): the distro's name by a
/// terminal glyph, in `blue`. Local repos show none.
pub fn location_chip(distro: &str, theme: &DvTheme) -> AnyElement {
    let c = &theme.colors;
    chip(c.blue, c.blue_soft, theme)
        .child(icon(DvIcon::Terminal, px(12.), c.blue))
        .child(SharedString::from(distro.to_string()))
        .into_any_element()
}

/// A review status as the UI words it (PLAN §2.4).
pub fn status_label(status: ReviewStatus) -> &'static str {
    match status {
        ReviewStatus::Open => "Open",
        ReviewStatus::ChangesRequested => "Changes requested",
        ReviewStatus::InProgress => "Agent working",
        ReviewStatus::Ready => "Ready for review",
        ReviewStatus::Approved => "Approved",
        ReviewStatus::Closed => "Closed",
        ReviewStatus::Draft => "Draft",
        ReviewStatus::Submitted => "Submitted",
    }
}

/// A review's status as a chip, coloured by whose turn it is: the reviewer's (`open`, `ready`,
/// `draft`) in the accent, changes asked for in red, the agent at work in blue, finished green or
/// grey. The words carry the meaning too, never the colour alone.
pub fn status_chip(status: ReviewStatus, theme: &DvTheme) -> AnyElement {
    let c = &theme.colors;
    let (fg, bg) = match status {
        ReviewStatus::Open | ReviewStatus::Draft => (c.accent_text, c.accent_soft),
        ReviewStatus::Ready => (c.yellow, c.yellow_soft),
        ReviewStatus::ChangesRequested => (c.red, c.red_soft),
        ReviewStatus::InProgress => (c.blue, c.blue_soft),
        ReviewStatus::Approved | ReviewStatus::Submitted => (c.green, c.green_soft),
        ReviewStatus::Closed => (c.fg3, c.layer),
    };
    chip(fg, bg, theme)
        .h(px(18.))
        .px(theme.spacing.sm)
        .text_size(px(11.5))
        .font_weight(FontWeight::SEMIBOLD)
        .child(status_label(status))
        .into_any_element()
}

/// A count badge (the canvas's `countBadge`: 18 high, fully rounded, accent soft fill).
pub fn count_badge(theme: &DvTheme) -> Div {
    let c = &theme.colors;
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(3.))
        .h(px(18.))
        .px(theme.spacing.sm)
        .rounded(px(9.))
        .bg(c.accent_soft)
        .text_color(c.accent_text)
        .text_size(px(11.))
        .line_height(px(18.))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
}

/// The viewed checkbox (the canvas's file rows: 16 square, radius 4, a 1.5 `line2` outline, or
/// the accent filled with a check when on).
pub fn checkbox(checked: bool, theme: &DvTheme) -> Div {
    let c = &theme.colors;
    let size = px(16.);
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(size)
        .rounded(theme.radii.small)
        .border(px(1.5))
        .border_color(if checked { c.accent } else { c.line2 })
        .when(checked, |this| this.bg(c.accent))
        .children(checked.then(|| icon(DvIcon::Check, px(11.), c.accent_fg)))
}

/// `+n −m` in mono, green and red (`fg3` for a zero, as the canvas dims an empty side).
pub fn totals(additions: u64, deletions: u64, size: Pixels, theme: &DvTheme) -> AnyElement {
    let c = &theme.colors;
    div()
        .flex_none()
        .flex()
        .gap(px(4.))
        .font_family(theme.fonts.mono.clone())
        .text_size(size)
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .child(
            div()
                .text_color(if additions > 0 { c.green } else { c.fg3 })
                .child(format!("+{additions}")),
        )
        .child(
            div()
                .text_color(if deletions > 0 { c.red } else { c.fg3 })
                .child(format!("−{deletions}")),
        )
        .into_any_element()
}

/// A skeleton bar (DESIGN §4.4: loading shows skeletons, never a spinner over content).
pub fn skeleton(width: Pixels, height: Pixels, theme: &DvTheme) -> AnyElement {
    div()
        .flex_none()
        .w(width)
        .h(height)
        .rounded(theme.radii.small)
        .bg(theme.colors.layer)
        .into_any_element()
}

/// A small outlined button (the canvas's `btnSm`: 24 high, `ctrl` fill, `ctrlLine` outline,
/// 12/500). Give it an `on_click`.
pub fn small_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    theme: &DvTheme,
) -> Stateful<Div> {
    let c = &theme.colors;
    let hover = c.layer_strong;
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .gap(px(5.))
        .h(theme.sizes.control_small_height)
        .px(theme.spacing.md)
        .rounded(theme.radii.control)
        .border_1()
        .border_color(c.ctrl_line)
        .bg(c.ctrl)
        .text_color(c.fg)
        .text_spec(&theme.text.secondary)
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .child(label.into())
}

/// [`small_button`] with a glyph in place of its label: square, for where the label has no
/// room. Give it a [`tooltip`] that says what it does.
pub fn small_icon_button(
    id: impl Into<gpui::ElementId>,
    glyph: DvIcon,
    theme: &DvTheme,
) -> Stateful<Div> {
    let c = &theme.colors;
    let hover = c.layer_strong;
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(theme.sizes.control_small_height)
        .rounded(theme.radii.control)
        .border_1()
        .border_color(c.ctrl_line)
        .bg(c.ctrl)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .child(icon(glyph, px(13.), c.fg))
}

/// A segmented control's frame (the file header's Split | Unified, DESIGN §1.5): 28 high, its
/// [`segment`]s side by side on `ctrl` inside a `ctrl_line` outline.
pub fn segmented(theme: &DvTheme) -> Div {
    let c = &theme.colors;
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(2.))
        .h(theme.sizes.control_height)
        .p(px(2.))
        .rounded(theme.radii.segmented)
        .border_1()
        .border_color(c.ctrl_line)
        .bg(c.ctrl)
}

/// One segment of a [`segmented`] control: the chosen one lifted on `seg_on` with the control
/// shadow, the others hovering; a disabled one (`enabled` false) in `fg3`, taking no clicks.
/// Give it its label and, when enabled and not chosen, an `on_click`.
pub fn segment(
    id: impl Into<gpui::ElementId>,
    on: bool,
    enabled: bool,
    theme: &DvTheme,
) -> Stateful<Div> {
    let c = &theme.colors;
    let hover = c.control_hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(theme.spacing.sm)
        .h(theme.sizes.control_xsmall_height)
        .px(theme.spacing.md)
        .rounded(theme.radii.chip)
        .text_spec(&theme.text.secondary)
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .text_color(match (on, enabled) {
            (_, false) => c.fg3,
            (true, true) => c.fg,
            (false, true) => c.fg2,
        })
        .when(on, |this| {
            this.bg(c.seg_on).shadow(theme.shadows.control.clone())
        })
        .when(!on && enabled, |this| {
            this.cursor_pointer().hover(move |style| style.bg(hover))
        })
}

/// A plain text tooltip, in gpui-component's style (mapped from the theme), for an element's
/// `.tooltip(…)`.
pub fn tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyView + 'static {
    let text: SharedString = text.into();
    move |window, cx| gpui_component::tooltip::Tooltip::new(text.clone()).build(window, cx)
}

/// A ghost button (the canvas's `ghost`: 28 high, no fill until hovered, 13/500).
pub fn ghost_button(id: impl Into<gpui::ElementId>, theme: &DvTheme) -> Stateful<Div> {
    let c = &theme.colors;
    let hover = c.control_hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(7.))
        .h(theme.sizes.control_height)
        .px(theme.spacing.md)
        .rounded(theme.radii.control)
        .text_spec(&theme.text.control)
        .text_color(c.fg)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
}

/// A menu's panel (the file header's `⋯` menu, a comment row's right-click menu): 200 wide,
/// radius 8, the `overlay` surface with its outline and shadow, its items stacked. It takes the
/// pointer's clicks (`occlude`); hang it with `deferred(anchored())` and close it on a click
/// outside it.
pub fn menu_panel(id: impl Into<gpui::ElementId>, theme: &DvTheme) -> Stateful<Div> {
    let c = &theme.colors;
    div()
        .id(id)
        .occlude()
        .w(px(200.))
        .p(theme.spacing.xs)
        .rounded(px(8.))
        .border_1()
        .border_color(c.overlay_line)
        .bg(c.popover)
        .shadow(theme.shadows.overlay.clone())
}

/// One of a menu's items: its glyph and label, a control's height, `control_hover` under the
/// pointer. A disabled one is dimmed and takes no pointer; give an enabled one its `on_click`.
pub fn menu_item(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    glyph: DvIcon,
    enabled: bool,
    theme: &DvTheme,
) -> Stateful<Div> {
    let c = &theme.colors;
    let hover = c.control_hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(theme.spacing.md)
        .h(theme.sizes.control_height)
        .px(theme.spacing.md)
        .rounded(theme.radii.control)
        .text_spec(&theme.text.body)
        .text_color(c.fg)
        .child(icon(glyph, px(14.), c.fg2))
        .child(label.into())
        .when(enabled, |this| {
            this.cursor_pointer().hover(move |style| style.bg(hover))
        })
        .when(!enabled, |this| this.opacity(0.5))
}

/// How wide `text` draws in `spec`'s font, as `window` shapes it.
pub fn text_width(window: &gpui::Window, text: &str, spec: &crate::theme::TextSpec) -> Pixels {
    if text.is_empty() {
        return px(0.);
    }
    let font = gpui::Font {
        weight: spec.weight,
        ..gpui::font(spec.family.clone())
    };
    let run = gpui::TextRun {
        len: text.len(),
        font,
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(
            SharedString::from(text.to_string()),
            spec.size,
            &[run],
            None,
        )
        .width
}

/// `path` as it fits where `fits` says: whole, else its first segment, "…" and as many whole
/// trailing segments as fit ("/home/…/react/index.d.ts:12"), so its start and its file stay in
/// view; the name alone after "…" when nothing more fits. Either separator.
pub fn elide_path_middle(path: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(path) {
        return path.to_string();
    }
    let sep = if path.contains('\\') && !path.contains('/') {
        '\\'
    } else {
        '/'
    };
    let parts: Vec<&str> = path.split(sep).collect();
    let n = parts.len();
    // The root and the first name (`/home`, `C:\Users`), or the first name alone.
    let head_len = if parts
        .first()
        .is_some_and(|first| first.is_empty() || first.ends_with(':'))
    {
        2
    } else {
        1
    };
    let tail = |keep: usize| parts[n - keep..].join(&sep.to_string());
    if n > head_len + 1 {
        let head = parts[..head_len].join(&sep.to_string());
        for keep in (1..n - head_len).rev() {
            let shown = format!("{head}{sep}…{sep}{}", tail(keep));
            if fits(&shown) {
                return shown;
            }
        }
    }
    format!("…{sep}{}", tail(1))
}

/// `path` as it fits where `fits` says: whole, else with as few whole leading segments as it
/// takes given way to "…/" ("…/feedback/feedback-badge.tsx"), never cut inside a segment;
/// the file's name alone (after "…/") when nothing more fits, which then truncates as it
/// draws.
pub fn elide_path_start(path: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(path) {
        return path.to_string();
    }
    // Either separator: a file outside the repository may be a Windows path.
    let separators = ['/', '\\'];
    let Some(name_at) = path.rfind(separators) else {
        return path.to_string();
    };
    path.match_indices(separators)
        .map(|(at, _)| format!("…{}", &path[at..]))
        .find(|shown| fits(shown))
        .unwrap_or_else(|| format!("…{}", &path[name_at..]))
}
