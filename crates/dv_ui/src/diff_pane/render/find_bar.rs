//! The find bar (DESIGN §4.1 "Find"): over the top right of the stream, 8 below the file
//! header's 44px bar (so the pinned header's controls stay in reach), 16 in from the right
//! edge, 400 wide, on the `overlay` surface with its outline and shadow, radius `card`. One line
//! holds the search glyph and the field, the count ("3 of 17",
//! "3 of 17+" while files still load, "No results"), the previous and next arrows and the close
//! ×; under it, when files couldn't be searched, "2 files not searched" in `fg3`.
//!
//! It takes the pointer's presses (`occlude`), so a press on it leaves the stream's selection
//! and the keyboard where they are. Automation targets: `find`, `find/input`, `find/prev`,
//! `find/next`, `find/close`.

use gpui::{
    AnyElement, Context, InteractiveElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px,
};
use gpui_component::input::Input;

use super::super::DiffPane;
use super::super::rows::heights;
use crate::automation::AutomationTargetExt as _;
use crate::components::{icon, tooltip};
use crate::icons::DvIcon;
use crate::keymap;
use crate::theme::{DvTheme, StyledText as _};

/// The bar's width.
const FIND_WIDTH: f32 = 400.;
/// Its gap below the file header's bar and from the stream's right edge.
const FIND_TOP: f32 = 8.;
const FIND_RIGHT: f32 = 16.;
/// Its height: the field (a control high) inside 6 of padding and the outline, and the "files not
/// searched" line (4 apart, 16 high) when there is one.
const FIND_HEIGHT: f32 = 28. + 2. * 6. + 2.;
const FIND_SKIPPED_HEIGHT: f32 = 4. + 16.;

impl DiffPane {
    /// How far down the stream the open find bar reaches, from the stream's top.
    pub(in super::super) fn find_bar_bottom(&self) -> f32 {
        let skipped = if self.find.skipped > 0 {
            FIND_SKIPPED_HEIGHT
        } else {
            0.
        };
        heights::HEADER_BAR + FIND_TOP + FIND_HEIGHT + skipped
    }

    /// The find bar, while it is open.
    pub(in super::super) fn render_find_bar(
        &self,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.find.open {
            return None;
        }
        let input = self.find.input.clone()?;
        let summary = self.find_summary()?;
        let c = &theme.colors;
        let label = summary.label().map(|label| {
            let quiet = summary.count == 0;
            div()
                .flex_none()
                .min_w(px(56.))
                .text_spec(&theme.text.secondary)
                .text_color(if quiet { c.fg3 } else { c.fg2 })
                .whitespace_nowrap()
                .child(label)
                .automation_target("find/count")
        });
        let field = div()
            .relative()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .h(theme.sizes.control_height)
            .pl(px(28.))
            .pr(theme.spacing.md)
            .rounded(theme.radii.control)
            .border_1()
            .border_color(c.ctrl_line)
            .bg(c.input_bg)
            .child(div().absolute().left(px(8.)).top(px(6.)).child(icon(
                DvIcon::Search,
                px(14.),
                c.fg3,
            )))
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&input)
                        .appearance(false)
                        .aria_label("Find in review")
                        .px_0()
                        .text_size(px(13.)),
                ),
            )
            .automation_target("find/input");
        let has_matches = summary.count > 0;
        let button = |key: &'static str, glyph: DvIcon, tip: String| {
            let hover = c.control_hover;
            div()
                .id(key)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(theme.sizes.control_small_height)
                .rounded(theme.radii.control)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(icon(glyph, px(14.), c.fg2))
                .tooltip(tooltip(tip))
        };
        let keys = |action: &dyn gpui::Action| {
            keymap::shortcut_for(action, keymap::FIND)
                .map(|shortcut| shortcut.label().to_string())
                .unwrap_or_default()
        };
        let prev = button(
            "find-prev",
            DvIcon::ArrowUp,
            format!("Previous match  {}", keys(&crate::actions::find::Previous)),
        )
        .when(has_matches, |this| {
            this.on_click(cx.listener(|this, _, _, cx| this.step_find(false, cx)))
        })
        .automation_target("find/prev");
        let next = button(
            "find-next",
            DvIcon::ArrowDown,
            format!("Next match  {}", keys(&crate::actions::find::Next)),
        )
        .when(has_matches, |this| {
            this.on_click(cx.listener(|this, _, _, cx| this.step_find(true, cx)))
        })
        .automation_target("find/next");
        let close = button(
            "find-close",
            DvIcon::X,
            format!("Close  {}", keys(&crate::actions::find::Close)),
        )
        .on_click(cx.listener(|this, _, window, cx| this.close_find_and_refocus(window, cx)))
        .automation_target("find/close");
        let skipped = summary.skipped_label().map(|text| {
            div()
                .pl(px(28.))
                .text_spec(&theme.text.secondary)
                .text_color(c.fg3)
                .child(text)
                .automation_target("find/skipped")
        });
        Some(
            div()
                .id("find-bar")
                .key_context(keymap::FIND)
                .occlude()
                .absolute()
                .top(px(heights::HEADER_BAR + FIND_TOP))
                .right(px(FIND_RIGHT))
                .w(px(FIND_WIDTH))
                .flex()
                .flex_col()
                .gap(theme.spacing.xs)
                .p(theme.spacing.sm)
                .rounded(theme.radii.card)
                .border_1()
                .border_color(c.overlay_line)
                .bg(c.overlay)
                .shadow(theme.shadows.overlay.clone())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.spacing.sm)
                        .child(field)
                        .children(label)
                        .child(prev)
                        .child(next)
                        .child(close),
                )
                .children(skipped)
                .debug_selector(|| "find-bar".into())
                .automation_target("find"),
        )
    }
}
