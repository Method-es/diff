//! The shell's part in setup (PLAN §12, DESIGN §4.3): the first-run screen, shown in place of
//! the columns and the status bar until it is continued or skipped; Settings (`Ctrl ,`, the
//! title bar's `⋯`), a sheet over the scrim that opens like a menu (in place of an open menu,
//! over the "Finish round" sheet) and gives the keyboard back where it was when it closes; and
//! the status bar's notice of a skill out of date ("dv-review skill out of date in Ubuntu ·
//! Update", with a dismiss).
//!
//! The first-run screen never blocks a review: what `dv open` asked for opens behind it as
//! ever, there the moment it is left (`Enter` continues).

use gpui::{
    AnyElement, App, AppContext as _, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window, deferred, div, px,
};

use super::render::MENU_PRIORITY;
use super::{Overlay, Shell};
use crate::actions::first_run::Continue;
use crate::actions::shell::ToggleSettings;
use crate::automation::AutomationTargetExt as _;
use crate::components::{icon, small_button, small_icon_button, tooltip};
use crate::icons::DvIcon;
use crate::services::ActiveServices as _;
use crate::services::setup::FirstRunEnd;
use crate::setup::settings::{SettingsEvent, SettingsView};
use crate::theme::{DvTheme, StyledText as _};

impl Shell {
    /// Whether the first-run screen shows.
    pub fn first_run_open(&self, cx: &App) -> bool {
        cx.services().setup.read(cx).first_run()
    }

    /// The Settings sheet, while it is open.
    pub fn settings(&self) -> Option<&gpui::Entity<SettingsView>> {
        self.settings.as_ref().map(|overlay| &overlay.view)
    }

    /// Opens Settings › Setup (`Ctrl ,`, the title bar's `⋯`, the backend banner's Open setup)
    /// in place of an open menu, over an open sheet.
    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        let return_focus = self.take_menu().unwrap_or_else(|| window.focused(cx));
        self.close_comments_menu(cx);
        let view = cx.new(SettingsView::new);
        let subscription = cx.subscribe_in(
            &view,
            window,
            |this, _, event: &SettingsEvent, window, cx| match event {
                SettingsEvent::Dismiss => {
                    this.close_menu(window, cx);
                }
            },
        );
        view.update(cx, |view, cx| view.focus(window, cx));
        self.settings = Some(Overlay {
            view,
            return_focus,
            _subscription: subscription,
        });
        cx.notify();
    }

    pub(super) fn toggle_settings(
        &mut self,
        _: &ToggleSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.is_some() {
            self.close_menu(window, cx);
        } else {
            self.open_settings(window, cx);
        }
    }

    /// `Enter` on the first-run screen: Continue.
    pub(super) fn continue_first_run(
        &mut self,
        _: &Continue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let setup = cx.services().setup.clone();
        setup.update(cx, |setup, cx| {
            setup.finish_first_run(FirstRunEnd::Continue, cx)
        });
        self.main_focus(cx).focus(window, cx);
    }

    /// The setup service changed: the first-run screen gone gives the keyboard to the main
    /// area.
    pub(super) fn setup_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let first_run = self.first_run_open(cx);
        if self.first_run_shown && !first_run && self.first_run_focus.is_focused(window) {
            self.main_focus(cx).focus(window, cx);
        }
        self.first_run_shown = first_run;
        cx.notify();
    }

    /// The first-run screen, in place of the columns and the status bar.
    pub(super) fn render_first_run(
        &self,
        theme: &DvTheme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let setup = cx.services().setup.clone();
        crate::setup::first_run::view(&setup, &self.first_run_focus, theme, window, cx)
    }

    /// Settings, centred over the scrim like the sheet, above it when both are open.
    pub(super) fn render_settings(&self, window: &Window, theme: &DvTheme) -> Option<AnyElement> {
        let settings = self.settings.as_ref()?;
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
                    .child(settings.view.clone()),
            )
            .with_priority(MENU_PRIORITY)
            .into_any_element(),
        )
    }

    /// The status bar's notice (module docs), if there is one.
    pub(super) fn render_skill_notice(
        &self,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let notice = cx.services().setup.read(cx).notice()?;
        let c = &theme.colors;
        let setup = cx.services().setup.clone();
        let update = {
            let setup = setup.clone();
            small_button("skill-notice-update", notice.action, theme)
                .h(theme.sizes.control_xsmall_height)
                .on_click(move |_, _, cx| setup.update(cx, |setup, cx| setup.apply_notice(cx)))
                .automation_target("status/skill_notice/update")
        };
        let dismiss = small_icon_button("skill-notice-dismiss", DvIcon::X, theme)
            .size(theme.sizes.control_xsmall_height)
            .tooltip(tooltip("Dismiss"))
            .on_click(move |_, _, cx| setup.update(cx, |setup, cx| setup.dismiss_notice(cx)))
            .automation_target("status/skill_notice/dismiss");
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_color(c.yellow)
                .child(icon(DvIcon::TriangleAlert, px(13.), c.yellow))
                .child(div().text_spec(&theme.text.secondary).child(notice.text))
                .child(update)
                .child(dismiss)
                .automation_target("status/skill_notice"),
        )
    }

    /// The title bar's `⋯`: Settings.
    pub(super) fn render_settings_button(
        &self,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let hover = c.control_hover;
        div()
            .id("titlebar-settings")
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .size(theme.sizes.control_height)
            .rounded(theme.radii.control)
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .occlude()
            .tooltip(tooltip(format!(
                "Settings  {}",
                crate::keymap::label("secondary-,")
            )))
            .child(icon(DvIcon::Ellipsis, px(16.), c.fg2))
            .on_click(cx.listener(|this, _, window, cx| {
                if this.settings.is_some() {
                    this.close_menu(window, cx);
                } else {
                    this.open_settings(window, cx);
                }
            }))
            .automation_target("titlebar/settings")
    }
}
