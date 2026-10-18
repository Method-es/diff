//! The window frame where dv is its own window manager's title bar: Linux (DESIGN §2).
//!
//! On Windows the title bar is a drag region the OS handles (`WindowControlArea::Drag`) and its
//! caption buttons are the OS's hit-test areas; macOS draws its traffic lights over the bar. On
//! Linux gpui does neither, so the bar does what a window manager's would itself
//! ([`moves_window`]): a press and a drag move the window (`start_window_move`, started on the
//! first move after the press, so a click or a double-click still reaches the bar), a
//! double-click maximizes or restores it, and a right-click opens the window manager's menu.
//!
//! The main window asks for client-side decorations on Linux (`shell::window_options`). While it
//! has them ([`client_decorated`]: Wayland, or X11 with a compositor), the bar shows the caption
//! buttons the window manager supports ([`caption_click`] does what they say) and
//! gpui-component's `Root` draws the frame's border, shadow and resize edges around the window.
//! When the platform keeps the decorations itself (X11 without a compositor, such as an Xvfb
//! display), the window manager draws its own title bar and buttons and dv's bar shows none.

use gpui::{App, Decorations, Div, InteractiveElement as _, MouseButton, Stateful, Window};

use crate::automation::is_background;

/// Whether dv draws the window's decorations itself (module docs).
pub fn client_decorated(window: &Window) -> bool {
    matches!(window.window_decorations(), Decorations::Client { .. })
}

/// A press on the bar that a drag turns into a window move.
struct Press {
    armed: bool,
}

/// `bar` moving, maximizing and opening the menu of the window as a window manager's title bar
/// does (module docs). Linux only: elsewhere the platform does it.
pub(super) fn moves_window(bar: Stateful<Div>, window: &mut Window, cx: &mut App) -> Stateful<Div> {
    let press = window.use_state(cx, |_, _| Press { armed: false });
    bar.on_mouse_down(MouseButton::Left, {
        let press = press.clone();
        move |event, window, cx| {
            let double = event.click_count == 2;
            press.update(cx, |press, _| press.armed = !double);
            if double {
                toggle_maximized(window);
            }
        }
    })
    .on_mouse_up(MouseButton::Left, {
        let press = press.clone();
        move |_, _, cx| press.update(cx, |press, _| press.armed = false)
    })
    .on_mouse_down_out({
        let press = press.clone();
        move |_, _, cx| press.update(cx, |press, _| press.armed = false)
    })
    .on_mouse_move(move |event, window, cx| {
        if press.read(cx).armed && event.pressed_button == Some(MouseButton::Left) {
            press.update(cx, |press, _| press.armed = false);
            window.start_window_move();
        }
    })
    .on_mouse_down(MouseButton::Right, |event, window, _| {
        if client_decorated(window) && window.window_controls().window_menu {
            window.show_window_menu(event.position);
        }
    })
}

/// What a caption button does on Linux, where gpui leaves the click to the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caption {
    Minimize,
    /// Maximize, or restore a maximized window.
    Maximize,
    Close,
}

/// Does what the caption button `caption` says to the window.
pub fn caption_click(caption: Caption, window: &mut Window) {
    match caption {
        Caption::Minimize => window.minimize_window(),
        Caption::Maximize => toggle_maximized(window),
        // The last window closing quits the app (`QuitMode::LastWindowClosed`).
        Caption::Close => window.remove_window(),
    }
}

/// Maximizes the window, or restores it when it is maximized. Never in a background
/// automation run, where nothing may raise the window (`crate::automation`).
fn toggle_maximized(window: &Window) {
    if !is_background() {
        window.zoom_window();
    }
}
