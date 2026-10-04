//! Names for the elements automation scripts address (PLAN §14, `tests/e2e/README.md`), and the
//! one way dv opens windows.
//!
//! Views call `.automation_target("titlebar/close")` unconditionally. With the `automation`
//! feature this is `gpui_automation`'s extension, which records the element's bounds while the
//! driver runs (and is a plain `into_any_element()` otherwise). Without the feature the
//! stand-ins below compile to the same `into_any_element()`, so the shipped build, which leaves
//! automation out, needs no `cfg` in the views.
//!
//! Root views also render [`pointer_guard`] first, which keeps the real mouse from interleaving
//! with scripted input while the driver runs.
//!
//! **Every window dv opens goes through [`open_window`].** Under `--automation-background` it
//! opens the window unfocused, behind every other window and out of the taskbar, so test runs
//! never disturb the person using the machine; otherwise, and without the `automation` feature,
//! it is plain `App::open_window`. While [`is_background`] holds, nothing may activate a window
//! or the app (`Window::activate_window`, `App::activate`), resize it (`Window::resize`) or
//! maximize it (`Window::zoom_window`): on Windows each of these raises and activates it.
//!
//! Names are hierarchical so `query{prefix}` can list a group: `titlebar/…` for the title bar,
//! `shell/…` for the columns, `statusbar/…` for the status bar, then one prefix per view.

/// The name the main window has: the one automation starts with.
pub const MAIN_WINDOW: &str = "main";

#[cfg(feature = "automation")]
pub use gpui_automation::{
    AutomationTargetExt, is_active, is_background, open_window, pointer_guard, register_window,
    unregister_window,
};

#[cfg(not(feature = "automation"))]
pub use disabled::{
    AutomationTargetExt, is_active, is_background, open_window, pointer_guard, register_window,
    unregister_window,
};

#[cfg(not(feature = "automation"))]
mod disabled {
    use gpui::{
        AnyElement, AnyWindowHandle, App, Empty, Entity, IntoElement, Render, SharedString, Window,
        WindowHandle, WindowOptions,
    };

    /// Stand-in for `gpui_automation::pointer_guard`: nothing to guard without automation.
    pub fn pointer_guard() -> AnyElement {
        Empty.into_any_element()
    }

    /// Without automation nothing runs in the background.
    pub fn is_background() -> bool {
        false
    }

    /// Without automation nothing drives the app.
    pub fn is_active() -> bool {
        false
    }

    /// Stand-in for `gpui_automation::register_window`: no driver, no names.
    pub fn register_window(name: impl Into<String>, window: AnyWindowHandle, cx: &mut App) {
        let _ = (name.into(), window, cx);
    }

    /// Stand-in for `gpui_automation::unregister_window`.
    pub fn unregister_window(name: &str, cx: &mut App) {
        let _ = (name, cx);
    }

    /// Stand-in for `gpui_automation::open_window`: a plain `App::open_window`.
    pub fn open_window<V: 'static + Render>(
        cx: &mut App,
        options: WindowOptions,
        build_root_view: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> anyhow::Result<WindowHandle<V>> {
        cx.open_window(options, build_root_view)
    }

    /// Stand-in for `gpui_automation::AutomationTargetExt` when automation is compiled out.
    pub trait AutomationTargetExt: IntoElement + Sized {
        fn automation_target(self, name: impl Into<SharedString>) -> AnyElement {
            let _ = name;
            self.into_any_element()
        }

        fn automation_target_with(self, name: impl FnOnce() -> SharedString) -> AnyElement {
            let _ = name;
            self.into_any_element()
        }
    }

    impl<T: IntoElement> AutomationTargetExt for T {}
}
