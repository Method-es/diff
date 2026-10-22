//! Settings (`Ctrl ,`, DESIGN §4.3): a sheet over the scrim, centred like the "Finish round"
//! sheet, scrolling through two pages: **Appearance** (the mode and the material,
//! [`super::appearance`]) and **Setup**, the setup screen's cards for every environment
//! (`super`) with **Check again**. Opening it checks everything again (the distros' running
//! state, a skill edited meanwhile). `Esc`, its close button or a click on the scrim closes it.
//!
//! Automation targets: `settings` (the sheet), `settings/close`, and the pages' own
//! (`settings/appearance`, `settings/mode/…`, `settings/material/…`; `setup/…`, `super`).

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::actions::settings::Dismiss;
use crate::automation::AutomationTargetExt as _;
use crate::components::{small_icon_button, tooltip};
use crate::icons::DvIcon;
use crate::keymap;
use crate::services::ActiveServices as _;
use crate::theme::{ActiveTheme as _, StyledText as _};

/// What the sheet tells the shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsEvent {
    Dismiss,
}

/// The Settings sheet (module docs).
pub struct SettingsView {
    focus_handle: FocusHandle,
    _observe: gpui::Subscription,
    _settings: gpui::Subscription,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let setup = cx.services().setup.clone();
        setup.update(cx, |setup, cx| setup.check_again(cx));
        let settings = cx.services().settings.clone();
        Self {
            focus_handle: cx.focus_handle(),
            _observe: cx.observe(&setup, |_, _, cx| cx.notify()),
            _settings: cx.observe(&settings, |_, _, cx| cx.notify()),
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
    }

    fn on_dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(SettingsEvent::Dismiss);
    }
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.dv_theme().clone();
        let c = &theme.colors;
        let setup = cx.services().setup.clone();
        let viewport = window.viewport_size();
        let max_height = (viewport.height
            - theme.sizes.title_bar_height
            - crate::submit::sheet_top(viewport.height)
            - px(24.))
        .max(px(320.));
        let width = (viewport.width - px(48.)).min(px(super::COLUMN_WIDTH + 40.));
        let close = small_icon_button("settings-close", DvIcon::X, &theme)
            .tooltip(tooltip("Close  Esc"))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Dismiss)))
            .automation_target("settings/close");
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.md)
            .h(px(52.))
            .px(theme.spacing.xl)
            .border_b_1()
            .border_color(c.line)
            .child(
                div()
                    .text_spec(&theme.text.heading)
                    .text_color(c.fg)
                    .child("Settings"),
            )
            .child(div().flex_1())
            .child(close);
        let body = div()
            .id("settings-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.md)
                    .p(theme.spacing.xl)
                    .child(super::appearance::page(&theme, cx))
                    .child(div().mt(theme.spacing.xl).mb(px(6.)).child(super::heading(
                        "Setup",
                        &theme.text.title,
                        &setup,
                        &theme,
                        cx,
                    )))
                    .children(super::environments(
                        &setup,
                        &theme,
                        width - theme.spacing.xl * 2. - px(2.),
                        window,
                        cx,
                    )),
            );
        div()
            .id("settings")
            .key_context(keymap::SETTINGS)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_dismiss))
            .occlude()
            .w(width)
            .max_h(max_height)
            .flex()
            .flex_col()
            .rounded(theme.radii.sheet)
            .border_1()
            .border_color(c.overlay_line)
            .bg(c.overlay)
            .shadow(theme.shadows.overlay.clone())
            .overflow_hidden()
            .child(header)
            .child(body)
            .automation_target("settings")
    }
}
