//! The file finder's panel (DESIGN §4.1 "File finder", the switcher's look: `overlay` over the
//! scrim, radius 12, the overlay shadow): the filter field, the files (30-high rows: the status
//! letter of a file the review changes, the path with its directory dimmed), and a footer with
//! the count and the keys.

use std::ops::Range;

use gpui::{
    AnyElement, Context, FontWeight, HighlightStyle, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, StyledText, Window,
    div, prelude::FluentBuilder as _, px, uniform_list,
};
use gpui_component::input::Input;

use super::{FINDER_WIDTH, FileFinder};
use crate::automation::AutomationTargetExt as _;
use crate::components::{icon, kbd};
use crate::file_list::{split_path, status_letter};
use crate::icons::DvIcon;
use crate::keymap;
use crate::theme::{ActiveTheme as _, DvTheme, StyledText as _};

/// A file row's height.
const ROW_HEIGHT: f32 = 30.;
/// The most rows the list shows before it scrolls.
const MAX_ROWS: usize = 14;

impl Render for FileFinder {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.dv_theme().clone();
        let c = &theme.colors;
        let viewport = window.viewport_size();
        let room = (viewport.height - theme.sizes.title_bar_height - px(200.)).max(px(120.));
        let rows = self.shown.len().clamp(1, MAX_ROWS) as f32;
        let list_height = px(rows * ROW_HEIGHT + 8.).min(room);
        div()
            .key_context(keymap::FILE_FINDER)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .w(FINDER_WIDTH)
            .flex()
            .flex_col()
            .rounded(theme.radii.sheet)
            .border_1()
            .border_color(c.overlay_line)
            .bg(c.overlay)
            .shadow(theme.shadows.overlay.clone())
            .overflow_hidden()
            .occlude()
            .text_spec(&theme.text.body)
            .text_color(c.fg)
            .child(self.render_filter(&theme))
            .child(self.render_list(list_height, &theme, cx))
            .child(self.render_footer(&theme))
            .automation_target("file_finder")
    }
}

impl FileFinder {
    fn render_filter(&self, theme: &DvTheme) -> AnyElement {
        let c = &theme.colors;
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(46.))
            .px(px(14.))
            .border_b_1()
            .border_color(c.line)
            .child(icon(DvIcon::Search, px(15.), c.fg3))
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.input)
                        .appearance(false)
                        .aria_label("Go to file")
                        .px_0()
                        .text_size(px(14.)),
                ),
            )
            .automation_target("file_finder/input")
    }

    fn render_list(
        &self,
        height: gpui::Pixels,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        if self.shown.is_empty() {
            let (title, note) = if self.query.trim().is_empty() {
                ("Listing files…".to_string(), String::new())
            } else {
                (
                    format!("No files match “{}”", self.query.trim()),
                    "Every word must be in the file's path.".to_string(),
                )
            };
            let quiet = !self.complete && self.query.trim().is_empty();
            return div()
                .flex()
                .flex_col()
                .items_center()
                .gap(theme.spacing.xs)
                .py(theme.spacing.xxl)
                .child(
                    div()
                        .text_color(if quiet { c.fg3 } else { c.fg2 })
                        .child(title),
                )
                .when(!note.is_empty(), |this| {
                    this.child(
                        div()
                            .text_spec(&theme.text.secondary)
                            .text_color(c.fg3)
                            .child(note),
                    )
                })
                .automation_target("file_finder/empty");
        }
        let theme = theme.clone();
        div()
            .flex_none()
            .h(height)
            .p(theme.spacing.sm)
            .child(
                uniform_list(
                    "file-finder-rows",
                    self.shown.len(),
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        this.render_rows(range, &theme, cx)
                    }),
                )
                .track_scroll(&self.scroll)
                .size_full(),
            )
            .into_any_element()
    }

    fn render_rows(
        &self,
        range: Range<usize>,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let c = &theme.colors;
        let hover = c.control_hover;
        range
            .filter_map(|ix| {
                let candidate = self.candidates.get(*self.shown.get(ix)?)?;
                let selected = ix == self.selected;
                let letter = candidate.status.map(|status| {
                    let letter = status_letter(status);
                    let (fg, _) = letter.colors(theme);
                    div()
                        .flex_none()
                        .w(px(14.))
                        .text_spec(&theme.text.secondary)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg)
                        .child(letter.letter)
                });
                Some(
                    div()
                        .id(gpui::ElementId::NamedInteger(
                            "file-finder-row".into(),
                            ix as u64,
                        ))
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(theme.spacing.md)
                        .h(px(ROW_HEIGHT))
                        .px(theme.spacing.md)
                        .rounded(theme.radii.control)
                        .cursor_pointer()
                        .map(|this| {
                            if selected {
                                this.bg(c.sel)
                            } else {
                                this.hover(move |style| style.bg(hover))
                            }
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.pick(ix, cx)))
                        .child(icon(
                            DvIcon::File,
                            px(14.),
                            if selected { c.fg } else { c.fg3 },
                        ))
                        .child(match letter {
                            Some(letter) => letter.into_any_element(),
                            None => div().flex_none().w(px(14.)).into_any_element(),
                        })
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis_start()
                                .child(path_text(&candidate.path, theme)),
                        )
                        .automation_target(format!("file_finder/row/{}", candidate.path))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_footer(&self, theme: &DvTheme) -> AnyElement {
        let c = &theme.colors;
        let hint = |keys: Vec<&'static str>, what: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .children(keys.into_iter().map(|key| kbd(keymap::label(key), theme)))
                .child(what)
        };
        let count = match self.shown.len() {
            1 => "1 file".to_string(),
            n => format!("{n} files"),
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.lg)
            .h(px(40.))
            .px(theme.spacing.lg)
            .border_t_1()
            .border_color(c.line)
            .text_spec(&theme.text.secondary)
            .text_color(c.fg3)
            .child(count)
            .child(div().flex_1())
            .child(hint(vec!["up", "down"], "move"))
            .child(hint(vec!["enter"], "open"))
            .child(hint(vec!["escape"], "close"))
            .into_any_element()
    }
}

/// A path: its directory in `fg3`, its name at 500.
fn path_text(path: &str, theme: &DvTheme) -> StyledText {
    let (dir, _) = split_path(path);
    let mut highlights = vec![(
        dir.len()..path.len(),
        HighlightStyle {
            font_weight: Some(FontWeight::MEDIUM),
            ..Default::default()
        },
    )];
    if !dir.is_empty() {
        highlights.insert(
            0,
            (
                0..dir.len(),
                HighlightStyle {
                    color: Some(theme.colors.fg3),
                    ..Default::default()
                },
            ),
        );
    }
    highlights.retain(|(range, _)| !range.is_empty());
    StyledText::new(path.to_string()).with_highlights(highlights)
}
