//! The file header (DESIGN §4.1, the board's 44px `codeHead` bar and its side labels): the
//! chevron, the path with its directory dimmed, the status pill and totals, Split | Unified,
//! Viewed, previous / next file and the file menu. The same header is pinned at the top while
//! its file scrolls; the menu holds Copy path and Reveal in folder, through the system seam.

use std::path::PathBuf;

use dv_core::diff::{FileEntry, FileStatus};
use gpui::{
    Anchor, AnyElement, Context, ElementId, FontWeight, HighlightStyle, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledText, Window, anchored, deferred, div,
    prelude::FluentBuilder as _, px,
};

use super::super::rows::{Row, heights};
use super::super::{DiffPane, Menu};

use crate::automation::AutomationTargetExt as _;
use crate::components::{checkbox, chip, icon, kbd, totals};
use crate::file_list::{split_path, status_letter};
use crate::icons::DvIcon;
use crate::keymap;
use crate::services::ViewMode;
use crate::theme::{DvTheme, StyledText as _};

/// The menu's paint order: over the stream, like the title bar's overlays (which never show
/// with it: the menu only shows while the main area has the keyboard).
const MENU_PRIORITY: usize = 2;

impl DiffPane {
    /// Block `block`'s header; `pinned` draws the copy that stays at the top.
    pub(in super::super) fn render_header(
        &self,
        block: usize,
        pinned: bool,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let Some((entry, _)) = self.entry_and_state(block, cx) else {
            return div().into_any_element();
        };
        let Some(session) = self.session.as_ref().map(|session| session.read(cx)) else {
            return div().into_any_element();
        };
        let path = entry.path.clone();
        let viewed = session.is_viewed(&path);
        let read_only = session.is_read_only();
        let (old_label, new_label) = session.side_labels();
        let (collapsed, labels) = match self.blocks[block].rows.first() {
            Some(&Row::Header { collapsed, labels }) => (collapsed && !pinned, labels),
            _ => (false, false),
        };
        let base = if pinned {
            "stream/pinned".to_string()
        } else {
            format!("stream/file/{path}")
        };
        let id = |key: &'static str| {
            ElementId::NamedInteger(key.into(), (block * 2 + usize::from(pinned)) as u64)
        };
        let letter = status_letter(entry.status);
        let (fg, bg) = letter.colors(theme);
        let hover = c.control_hover;
        let icon_button = |key: &'static str, glyph: DvIcon, enabled: bool| {
            div()
                .id(id(key))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(theme.sizes.control_height)
                .rounded(theme.radii.control)
                .child(icon(glyph, px(15.), if enabled { c.fg2 } else { c.fg3 }))
                .when(enabled, |this| {
                    this.cursor_pointer().hover(move |style| style.bg(hover))
                })
                .when(!enabled, |this| this.opacity(0.5))
        };

        let chevron = {
            let path = path.clone();
            div()
                .id(id("chevron"))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(theme.sizes.control_xsmall_height)
                .rounded(theme.radii.chip)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(icon(
                    if collapsed {
                        DvIcon::ChevronRight
                    } else {
                        DvIcon::ChevronDown
                    },
                    px(14.),
                    c.fg2,
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_open(&path, collapsed, cx);
                }))
                .automation_target(format!("{base}/chevron"))
        };
        let segment = |key: &'static str, label: &'static str, glyph: DvIcon, mode: ViewMode| {
            let on = self.mode == mode;
            div()
                .id(id(key))
                .flex()
                .items_center()
                .gap(theme.spacing.sm)
                .h(theme.sizes.control_xsmall_height)
                .px(theme.spacing.md)
                .rounded(theme.radii.chip)
                .text_spec(&theme.text.secondary)
                .font_weight(FontWeight::MEDIUM)
                .text_color(if on { c.fg } else { c.fg2 })
                // The board lifts the chosen segment with the control shadow (visible in light).
                .when(on, |this| {
                    this.bg(c.seg_on).shadow(theme.shadows.control.clone())
                })
                .when(!on, |this| {
                    this.cursor_pointer().hover(move |style| style.bg(hover))
                })
                .child(icon(glyph, px(14.), if on { c.fg } else { c.fg2 }))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.set_view_mode(mode, cx)))
                .automation_target(format!("{base}/{key}"))
        };
        let segmented = div()
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
            .child(segment("split", "Split", DvIcon::Columns, ViewMode::Split))
            .child(segment(
                "unified",
                "Unified",
                DvIcon::Rows,
                ViewMode::Unified,
            ))
            .child(
                div()
                    .ml(px(2.))
                    .mr(px(4.))
                    .child(kbd(keymap::label("s"), theme)),
            );
        let viewed_button = {
            let path = path.clone();
            crate::components::button(id("viewed"), theme)
                .child(checkbox(viewed, theme).size(px(14.)))
                .child("Viewed")
                .child(kbd(keymap::label("v"), theme))
                .when(read_only, |this| this.opacity(0.5).cursor_default())
                .when(!read_only, |this| {
                    this.on_click(cx.listener(move |this, _, _, cx| this.toggle_viewed(&path, cx)))
                })
                .automation_target(format!("{base}/viewed"))
        };
        let last = self.blocks.len().saturating_sub(1);
        let previous = icon_button("prev", DvIcon::ArrowUp, block > 0)
            .when(block > 0, |this| {
                this.on_click(cx.listener(move |this, _, _, cx| this.go_to_file(block - 1, cx)))
            })
            .automation_target(format!("{base}/prev"));
        let next = icon_button("next", DvIcon::ArrowDown, block < last)
            .when(block < last, |this| {
                this.on_click(cx.listener(move |this, _, _, cx| this.go_to_file(block + 1, cx)))
            })
            .automation_target(format!("{base}/next"));
        let menu_open = self.menu.as_ref().is_some_and(|menu| menu.path == path);
        let menu_button = {
            let path = path.clone();
            icon_button("menu", DvIcon::Ellipsis, true)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        // A click on the button of an open menu has already closed it (the menu
                        // closes on any click outside it); it only opens a closed one.
                        if !menu_open {
                            this.menu = Some(Menu {
                                path: path.clone(),
                                position: event.position,
                            });
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }),
                )
                .automation_target(format!("{base}/menu"))
        };
        let loading = self.slow_to_load(&path).then(|| {
            div()
                .flex_none()
                .text_spec(&theme.text.secondary)
                .text_color(c.fg3)
                .child("Loading…")
        });
        let bar = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(heights::HEADER_BAR))
            .pl(theme.spacing.md)
            .pr(px(10.))
            .bg(opaque(c.code_head, pinned))
            .border_b_1()
            .border_color(c.line)
            .child(chevron)
            .child(
                div()
                    .min_w_0()
                    .flex_shrink(1.)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_start()
                    .font_family(theme.fonts.mono.clone())
                    .text_size(px(13.))
                    // The name's weight: truncation measures every character in the element's
                    // own font, so the heaviest run's keeps the name from overflowing its box
                    // (the lighter directory comes out a little shorter than it could).
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c.fg)
                    .child(header_path(&entry, viewed, theme)),
            )
            .child(
                chip(fg, bg, theme)
                    .px(px(7.))
                    .text_size(px(11.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(letter.word),
            )
            .child(totals(
                u64::from(entry.additions),
                u64::from(entry.deletions),
                px(12.),
                theme,
            ))
            .child(div().flex_1())
            .children(loading)
            .child(segmented)
            .child(viewed_button)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .gap(px(2.))
                    .child(previous)
                    .child(next)
                    .child(menu_button),
            );
        let labels = labels.then(|| {
            side_labels(
                &old_label,
                &new_label,
                self.mode,
                opaque(c.code, pinned),
                theme,
            )
        });
        div()
            .flex()
            .flex_col()
            .w_full()
            .child(bar)
            .children(labels)
            .automation_target(if pinned {
                base
            } else {
                format!("{base}/header")
            })
    }

    /// The file menu, while it is open and the main area has the keyboard.
    pub(in super::super) fn render_menu(
        &mut self,
        theme: &DvTheme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.menu.is_some() && !self.focus_handle.is_focused(window) {
            self.menu = None;
        }
        let menu = self.menu.clone()?;
        let c = &theme.colors;
        let toplevel = self
            .session
            .as_ref()
            .map(|session| session.read(cx).backend().repo().toplevel.clone())?;
        let deleted = self
            .index
            .get(&menu.path)
            .and_then(|&block| self.entry_and_state(block, cx))
            .is_some_and(|(entry, _)| entry.status == FileStatus::Deleted);
        let mut target = menu
            .path
            .split('/')
            .fold(PathBuf::from(&toplevel), |at, part| at.join(part));
        if deleted && let Some(parent) = target.parent() {
            target = parent.to_path_buf();
        }
        let hover = c.control_hover;
        let item = |key: &'static str, label: &'static str, glyph: DvIcon| {
            div()
                .id(SharedString::from(format!("stream-menu-{key}")))
                .flex()
                .items_center()
                .gap(theme.spacing.md)
                .h(theme.sizes.control_height)
                .px(theme.spacing.md)
                .rounded(theme.radii.control)
                .text_spec(&theme.text.body)
                .text_color(c.fg)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(icon(glyph, px(14.), c.fg2))
                .child(label)
        };
        let path = menu.path.clone();
        let panel = div()
            .id("stream-menu")
            .occlude()
            .w(px(200.))
            .p(theme.spacing.xs)
            .rounded(px(8.))
            .border_1()
            .border_color(c.overlay_line)
            .bg(c.overlay)
            .shadow(theme.shadows.overlay.clone())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .child(
                item("copy_path", "Copy path", DvIcon::Copy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        crate::system::write_clipboard(path.clone(), cx);
                        cx.notify();
                    }))
                    .automation_target("stream/menu/copy_path"),
            )
            .child(
                item("reveal", "Reveal in folder", DvIcon::FolderOpen)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        crate::system::reveal(&target, cx);
                        cx.notify();
                    }))
                    .automation_target("stream/menu/reveal"),
            )
            .automation_target("stream/menu");
        Some(
            deferred(
                anchored()
                    .position(menu.position)
                    .anchor(Anchor::TopRight)
                    .offset(gpui::point(px(14.), px(18.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(panel),
            )
            .with_priority(MENU_PRIORITY)
            .into_any_element(),
        )
    }
}

/// The header's path: the directory in `fg3` and the name at 600 (`fg3` once viewed); a
/// renamed file shows where it came from.
fn header_path(entry: &FileEntry, viewed: bool, theme: &DvTheme) -> StyledText {
    let c = &theme.colors;
    let dim = HighlightStyle {
        color: Some(c.fg3),
        font_weight: Some(FontWeight::MEDIUM),
        ..Default::default()
    };
    let mut text = String::new();
    let mut highlights = Vec::new();
    if let Some(old) = &entry.old_path {
        text.push_str(old);
        text.push_str(" → ");
        highlights.push((0..text.len(), dim));
    }
    let (dir, name) = split_path(&entry.path);
    let start = text.len();
    text.push_str(&entry.path);
    highlights.push((start..start + dir.len(), dim));
    highlights.push((
        start + dir.len()..start + dir.len() + name.len(),
        HighlightStyle {
            font_weight: Some(FontWeight::SEMIBOLD),
            color: viewed.then_some(c.fg3),
            ..Default::default()
        },
    ));
    highlights.retain(|(range, _)| !range.is_empty());
    StyledText::new(text).with_highlights(highlights)
}

/// The side labels under the header bar (the board's `main` | `working tree`).
fn side_labels(
    old: &str,
    new: &str,
    mode: ViewMode,
    background: gpui::Hsla,
    theme: &DvTheme,
) -> AnyElement {
    let c = &theme.colors;
    let label = |glyph: DvIcon, text: &str| {
        div()
            .flex()
            .items_center()
            .gap(theme.spacing.sm)
            .min_w_0()
            .child(icon(glyph, px(12.), c.fg3))
            .child(div().truncate().child(text.to_string()))
    };
    let row = div()
        .flex()
        .h(px(heights::SIDE_LABELS))
        .bg(background)
        .border_b_1()
        .border_color(c.line)
        .text_spec(&theme.text.secondary)
        .text_color(c.fg3);
    match mode {
        ViewMode::Split => row
            .child(
                div()
                    .w_1_2()
                    .flex()
                    .items_center()
                    .pl(theme.spacing.xl)
                    .border_r_1()
                    .border_color(c.line)
                    .child(label(DvIcon::GitBranch, old)),
            )
            .child(
                div()
                    .w_1_2()
                    .flex()
                    .items_center()
                    .pl(theme.spacing.xl)
                    .child(label(DvIcon::File, new)),
            )
            .into_any_element(),
        ViewMode::Unified => row
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.sm)
                    .pl(theme.spacing.xl)
                    .child(label(DvIcon::GitBranch, old))
                    .child("→")
                    .child(label(DvIcon::File, new)),
            )
            .into_any_element(),
    }
}

/// `surface` drawn without its translucency for the pinned copy of a header, which sits over
/// the stream's rows rather than the window material: nothing scrolls through it.
fn opaque(surface: gpui::Hsla, pinned: bool) -> gpui::Hsla {
    if pinned { surface.alpha(1.) } else { surface }
}
