//! Which theme is on screen: the settings' palette, material and mode (PLAN D8, DESIGN §1, §3),
//! narrowed by what the platform can show, what the palette has (DESIGN §1.3: Claude and
//! Dracula are opaque, Dracula dark) and, for `Mode::System`, the OS's light/dark setting.
//!
//! [`init`] runs after [`crate::services::init`] and before the first window, so the first frame
//! already uses the resolved theme, and from then on follows every settings change. The shell
//! reports the OS appearance as it changes ([`system_changed`]) and re-probes the platform when
//! the window is activated ([`refresh_support`]: the transparency switch may have flipped while
//! dv was in the background). Each change installs the theme ([`crate::theme::install`]), which
//! redraws every window; the shell then applies the material to its window.

use gpui::{App, Global, Subscription, WindowAppearance};

use crate::platform::MaterialSupport;
use crate::services::{ActiveServices as _, SettingsEvent};
use crate::theme::{self, Appearance, Material, Palette};

struct AppearanceState {
    /// The OS's light/dark setting.
    system: Appearance,
    /// What the platform can show.
    support: MaterialSupport,
    _settings: Subscription,
}

impl Global for AppearanceState {}

/// The settings resolved against the platform and the OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub palette: Palette,
    pub appearance: Appearance,
    pub material: Material,
}

/// Starts following the settings with `support` as the platform's abilities (from
/// [`MaterialSupport::probe`]) and installs the resolved theme.
pub fn init(support: MaterialSupport, cx: &mut App) {
    let settings = cx.services().settings.clone();
    let subscription = cx.subscribe(&settings, |_, event: &SettingsEvent, cx| match event {
        SettingsEvent::Changed => sync(cx),
    });
    log::info!("window materials available: {}", describe(support));
    cx.set_global(AppearanceState {
        system: Appearance::of_window(cx.window_appearance()),
        support,
        _settings: subscription,
    });
    sync(cx);
}

/// `"mica, acrylic, opaque"`, for the log.
fn describe(support: MaterialSupport) -> String {
    support
        .available()
        .iter()
        .map(|material| material.key())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The OS's appearance changed (the main window's appearance observer).
pub fn system_changed(appearance: WindowAppearance, cx: &mut App) {
    let system = Appearance::of_window(appearance);
    let Some(state) = cx.try_global::<AppearanceState>() else {
        return;
    };
    if state.system != system {
        cx.global_mut::<AppearanceState>().system = system;
        sync(cx);
    }
}

/// Re-reads what the platform can show (the transparency switch).
pub fn refresh_support(cx: &mut App) {
    set_support(MaterialSupport::probe(), cx);
}

/// Replaces what the platform can show: [`refresh_support`], and tests.
pub fn set_support(support: MaterialSupport, cx: &mut App) {
    let Some(state) = cx.try_global::<AppearanceState>() else {
        return;
    };
    if state.support != support {
        log::info!("window materials available now: {}", describe(support));
        cx.global_mut::<AppearanceState>().support = support;
        sync(cx);
    }
}

/// What the platform can show, as last probed.
pub fn support(cx: &App) -> MaterialSupport {
    cx.try_global::<AppearanceState>()
        .map_or(MaterialSupport::OPAQUE_ONLY, |state| state.support)
}

/// The OS's light/dark setting, as last reported.
pub fn system(cx: &App) -> Appearance {
    cx.try_global::<AppearanceState>()
        .map(|state| state.system)
        .unwrap_or_default()
}

/// The settings resolved now.
pub fn resolved(cx: &App) -> Resolved {
    let settings = cx.services().settings.read(cx);
    let palette = settings.palette();
    Resolved {
        palette,
        appearance: palette.appearance(settings.mode().resolve(system(cx))),
        material: palette.material(support(cx).resolve(settings.material())),
    }
}

fn sync(cx: &mut App) {
    let Resolved {
        palette,
        appearance,
        material,
    } = resolved(cx);
    theme::install(palette, appearance, material, cx);
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;
    use crate::ActiveTheme as _;
    use crate::services::{Mode, ServicesConfig};

    fn setup(cx: &mut TestAppContext, support: MaterialSupport) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().to_path_buf();
        cx.update(|cx| {
            crate::init(cx).unwrap();
            crate::services::init(
                ServicesConfig {
                    setup: crate::services::SetupConfig::sandboxed(
                        data_dir.join("home"),
                        data_dir.join("user-path.txt"),
                        false,
                    ),
                    data_dir,
                    wsl: crate::services::WslAccess::scripted(Vec::new()),
                },
                cx,
            );
            init(support, cx);
        });
        dir
    }

    fn on_screen(cx: &mut TestAppContext) -> (Appearance, Material) {
        cx.update(|cx| {
            let theme = cx.dv_theme();
            (theme.appearance, theme.material)
        })
    }

    #[gpui::test]
    fn the_theme_follows_the_settings(cx: &mut TestAppContext) {
        let _dir = setup(cx, MaterialSupport::ALL);
        let settings = cx.update(|cx| cx.services().settings.clone());
        settings.update(cx, |settings, cx| {
            settings.set_mode(Mode::Light, cx);
            settings.set_material(Material::Acrylic, cx);
        });
        assert_eq!(on_screen(cx), (Appearance::Light, Material::Acrylic));
        settings.update(cx, |settings, cx| {
            settings.set_mode(Mode::Dark, cx);
            settings.set_material(Material::Mica, cx);
        });
        assert_eq!(on_screen(cx), (Appearance::Dark, Material::Mica));
        cx.run_until_parked();
    }

    #[gpui::test]
    fn system_mode_follows_the_os(cx: &mut TestAppContext) {
        let _dir = setup(cx, MaterialSupport::ALL);
        let settings = cx.update(|cx| cx.services().settings.clone());
        settings.update(cx, |settings, cx| settings.set_mode(Mode::System, cx));
        cx.update(|cx| system_changed(WindowAppearance::Light, cx));
        assert_eq!(on_screen(cx).0, Appearance::Light);
        cx.update(|cx| system_changed(WindowAppearance::VibrantDark, cx));
        assert_eq!(on_screen(cx).0, Appearance::Dark);
        // An explicit mode ignores the OS.
        settings.update(cx, |settings, cx| settings.set_mode(Mode::Light, cx));
        cx.update(|cx| system_changed(WindowAppearance::Dark, cx));
        assert_eq!(on_screen(cx).0, Appearance::Light);
        cx.run_until_parked();
    }

    #[gpui::test]
    fn the_platform_narrows_the_material(cx: &mut TestAppContext) {
        let _dir = setup(cx, MaterialSupport::OPAQUE_ONLY);
        let settings = cx.update(|cx| cx.services().settings.clone());
        settings.update(cx, |settings, cx| settings.set_material(Material::Mica, cx));
        assert_eq!(on_screen(cx).1, Material::Opaque, "transparency off");
        // Transparency switched back on.
        cx.update(|cx| set_support(MaterialSupport::ALL, cx));
        assert_eq!(on_screen(cx).1, Material::Mica);
        assert_eq!(
            cx.update(|cx| resolved(cx).material),
            Material::Mica,
            "resolved matches the screen"
        );
        cx.run_until_parked();
    }
}
