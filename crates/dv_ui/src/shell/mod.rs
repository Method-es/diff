//! The root view (PLAN §11 `shell`, DESIGN §2): the title bar, the panel, the main area and the
//! status bar, over the window material (DESIGN §3).
//!
//! The shell keeps its window in step with the theme: whenever [`crate::theme::install`] puts a
//! new [`DvTheme`] in place it applies that theme's material and light/dark appearance to the
//! window ([`crate::platform::apply_window_material`],
//! [`crate::platform::apply_window_appearance`]) and repaints. It also reports the OS's
//! light/dark changes to [`crate::appearance`] and has it re-probe the platform when the window
//! is activated.
//!
//! With no review open the main area shows DESIGN §4.4's "no reviews" empty state.

mod render;
pub mod title_bar;

#[cfg(test)]
mod tests;

use anyhow::{Context as _, Result};
use gpui::{
    App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable, Size, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, point, px, size,
};
use gpui_component::Root;

use crate::platform;
use crate::theme::{ActiveTheme as _, Appearance, DvTheme, Material};

/// What the main area shows when there is nothing to review (DESIGN §4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmptyState {
    /// No review is open: "Pick a repo to start a review".
    NoReview,
}

impl EmptyState {
    /// `"no_review"`, as automation state spells it.
    pub fn key(self) -> &'static str {
        match self {
            Self::NoReview => "no_review",
        }
    }
}

/// The root view of the main window.
pub struct Shell {
    focus_handle: FocusHandle,
    /// The material last applied to the window, for `state.shell`.
    applied: Material,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe_global_in::<DvTheme>(window, |this, window, cx| {
                this.apply_window_theme(window, cx);
                cx.notify();
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                crate::appearance::system_changed(window.appearance(), cx);
                // gpui sets the window's light/dark to the OS's right after this observer runs;
                // dv's mode may differ, so it is put back on the next frame.
                window.on_next_frame(|window, cx| {
                    platform::apply_window_appearance(window, cx.dv_theme().appearance);
                });
            }),
            cx.observe_window_activation(window, |_, window, cx| {
                if window.is_window_active() {
                    crate::appearance::refresh_support(cx);
                }
            }),
        ];
        let theme = cx.dv_theme();
        // gpui opened the window light or dark as the OS is; dv's mode may say otherwise.
        if theme.appearance != Appearance::of_window(window.appearance()) {
            platform::apply_window_appearance(window, theme.appearance);
        }
        Self {
            focus_handle: cx.focus_handle(),
            // The window opened with the theme's material ([`window_options`]).
            applied: theme.material,
            _subscriptions: subscriptions,
        }
    }

    /// Whether the panel column shows (`Ctrl B` toggles it, DESIGN §2).
    pub fn panel_visible(&self) -> bool {
        true
    }

    /// The empty state the main area shows, if any.
    pub fn empty_state(&self) -> Option<EmptyState> {
        Some(EmptyState::NoReview)
    }

    /// The material applied to the window.
    pub fn applied_material(&self) -> Material {
        self.applied
    }

    fn apply_window_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = cx.dv_theme();
        platform::apply_window_appearance(window, theme.appearance);
        if theme.material != self.applied {
            platform::apply_window_material(window, theme.material);
            self.applied = theme.material;
        }
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Options for the main window: centred at `window_size` (logical px), DESIGN's minimum size, a
/// custom title bar, and the installed theme's material.
pub fn window_options(window_size: Size<gpui::Pixels>, cx: &App) -> WindowOptions {
    let theme = cx.dv_theme();
    let min_size = size(theme.sizes.window_min_width, theme.sizes.window_min_height);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            window_size,
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("dv".into()),
            // Hides the system title bar; the shell draws its own (`title_bar`).
            appears_transparent: true,
            // macOS: centred on the 40px title bar.
            traffic_light_position: Some(point(px(12.), px(12.))),
        }),
        window_min_size: Some(min_size),
        window_background: theme.material.window_background(),
        app_id: Some("dv".into()),
        ..Default::default()
    }
}

/// The main window and its root view.
pub struct MainWindow {
    pub handle: WindowHandle<Root>,
    pub shell: Entity<Shell>,
}

/// Opens the main window, gpui-component's `Root` around a [`Shell`], through
/// [`crate::automation::open_window`] like every dv window. Call after
/// [`crate::appearance::init`], so the window opens in the resolved theme and material.
pub fn open_main_window(window_size: Size<gpui::Pixels>, cx: &mut App) -> Result<MainWindow> {
    let options = window_options(window_size, cx);
    let mut shell = None;
    let handle = crate::automation::open_window(cx, options, |window, cx| {
        let view = cx.new(|cx| Shell::new(window, cx));
        shell = Some(view.clone());
        cx.new(|cx| Root::new(view, window, cx))
    })
    .context("opening the main window")?;
    let shell = shell.context("the main window built no shell")?;
    Ok(MainWindow { handle, shell })
}
