//! The macOS menu bar (DESIGN §5, "macOS"): dv's app menu (About dv, Settings…, Services,
//! Hide dv, Hide Others, Show All, Quit dv), an Edit menu whose items act on the focused text
//! field (Undo, Redo, Cut, Copy, Paste, Select All) and a Window menu (Minimize). A Mac app
//! without them has no Quit and no `⌘Q`; their keys are [`crate::keymap::MACOS_SHORTCUTS`] and
//! the rows the menus' actions already have, which gpui shows beside each item. Windows and Linux
//! have no menu bar: dv draws its own title bar there (DESIGN §2), and this does nothing.

use gpui::{App, Menu, MenuItem, OsAction, PromptLevel, SystemMenuType};
use gpui_component::input;

use crate::actions::{app, field, shell};

/// Installs the menu bar and the app actions behind it, on macOS.
pub fn init(cx: &mut App) {
    if !cfg!(target_os = "macos") {
        return;
    }
    cx.on_action(|_: &app::Quit, cx| cx.quit());
    cx.on_action(|_: &app::Hide, cx| cx.hide());
    cx.on_action(|_: &app::HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &app::ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &app::Minimize, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.minimize_window());
        }
    });
    cx.on_action(about);
    cx.set_menus(menus());
}

/// The menus, left to right.
pub fn menus() -> Vec<Menu> {
    vec![
        menu(
            "dv",
            vec![
                MenuItem::action("About dv", app::About),
                MenuItem::separator(),
                MenuItem::action("Settings…", shell::ToggleSettings),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide dv", app::Hide),
                MenuItem::action("Hide Others", app::HideOthers),
                MenuItem::action("Show All", app::ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit dv", app::Quit),
            ],
        ),
        menu(
            "Edit",
            vec![
                MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
                MenuItem::separator(),
                // dv's own cut and copy, which go through the clipboard seam (`crate::system`).
                MenuItem::os_action("Cut", field::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", field::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
                MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
            ],
        ),
        menu("Window", vec![MenuItem::action("Minimize", app::Minimize)]),
    ]
}

fn menu(name: &'static str, items: Vec<MenuItem>) -> Menu {
    Menu {
        name: name.into(),
        items,
        disabled: false,
    }
}

/// dv › About dv: its name and version in a prompt over the active window.
fn about(_: &app::About, cx: &mut App) {
    let Some(window) = cx.active_window() else {
        return;
    };
    let detail = format!(
        "Version {}\nLocal diff and PR review.",
        dv_backend::build::version_text()
    );
    let _ = window.update(cx, |_, window, cx| {
        // The answer needs nothing: OK only closes it.
        drop(window.prompt(PromptLevel::Info, "dv", Some(&detail), &["OK"], cx));
    });
}
