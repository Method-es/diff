//! dv's views and services: the Glass shell, switcher, palette, file list, diff stream, threads,
//! submit sheet, onboarding and settings, over services installed as gpui globals
//! (docs/PLAN.md §11, docs/DESIGN.md).
//!
//! gpui: **yes**. Together with the `dv` binary and `gpui_automation`, this is the only crate that
//! may depend on gpui (PLAN §4); everything it shows comes from `dv_core`, `dv_backend` and
//! `dv_cli` through their headless APIs.
//!
//! Startup order (the `dv` binary): [`init`] (gpui-component, fonts, a first theme), then
//! [`system::install`], [`services::init`] and [`appearance::init`] (which resolves the settings'
//! material and mode into the theme the first frame uses), then the main window with a
//! [`shell::Shell`] as its root view, opened through [`automation::open_window`].

pub mod appearance;
pub mod assets;
pub mod automation;
pub mod components;
pub mod icons;
pub mod platform;
pub mod services;
pub mod shell;
pub mod system;
pub mod theme;

pub use assets::Assets;
pub use icons::DvIcon;
pub use services::{ActiveServices, Services};
pub use shell::Shell;
pub use theme::{ActiveTheme, Appearance, DvTheme, Material};

use gpui::App;

/// Everything views need before the first window opens: gpui-component, the bundled fonts and a
/// first theme. The app must have been built with [`Assets`] as its asset source.
pub fn init(cx: &mut App) -> anyhow::Result<()> {
    gpui_component::init(cx);
    assets::load_fonts(cx)?;
    theme::apply(cx);
    Ok(())
}
