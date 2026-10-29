//! Settings › Appearance (DESIGN §4.3): the theme's palette, the light/dark mode and the window
//! material, each a segmented control that changes the setting at once (`services::Settings`,
//! which `crate::appearance` follows). A material the platform or the palette can't show
//! (Claude and Dracula are opaque, DESIGN §1.3) is drawn disabled, and a line under the control
//! says why; a setting it can't show (Mica with transparency effects since turned off, or with
//! Claude chosen) stays chosen, the window showing Opaque meanwhile (DESIGN §3). With Dracula,
//! which has no light palette, the mode row says so.
//!
//! Automation targets: `settings/appearance`, `settings/palette/<key>`, `settings/mode/<key>`
//! and `settings/material/<key>` (the settings' keys, `aura`, `system`, `mica`, …).

use gpui::{
    AnyElement, App, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _,
    px,
};

use crate::automation::AutomationTargetExt as _;
use crate::components::{segment, segmented};
use crate::platform::MaterialSupport;
use crate::services::{ActiveServices as _, Mode};
use crate::theme::{DvTheme, Material, Palette, StyledText as _};

/// The page: its heading over one card with a row per setting.
pub fn page(theme: &DvTheme, cx: &App) -> AnyElement {
    let c = &theme.colors;
    let settings = cx.services().settings.read(cx);
    let (palette, mode, material) = (settings.palette(), settings.mode(), settings.material());
    let support = crate::appearance::support(cx);

    let palettes = Palette::ALL
        .into_iter()
        .fold(segmented(theme), |control, each| {
            let on = each == palette;
            control.child(
                segment(
                    SharedString::from(format!("settings-palette-{}", each.key())),
                    on,
                    true,
                    theme,
                )
                .debug_selector(move || format!("settings-palette-{}", each.key()))
                .child(each.label())
                .when(!on, |this| {
                    this.on_click(move |_, _, cx| {
                        let settings = cx.services().settings.clone();
                        settings.update(cx, |settings, cx| settings.set_palette(each, cx));
                    })
                })
                .automation_target(format!("settings/palette/{}", each.key())),
            )
        });

    let modes = Mode::ALL
        .into_iter()
        .fold(segmented(theme), |control, each| {
            let on = each == mode;
            control.child(
                segment(
                    SharedString::from(format!("settings-mode-{}", each.key())),
                    on,
                    true,
                    theme,
                )
                .debug_selector(move || format!("settings-mode-{}", each.key()))
                .child(mode_label(each))
                .when(!on, |this| {
                    this.on_click(move |_, _, cx| {
                        let settings = cx.services().settings.clone();
                        settings.update(cx, |settings, cx| settings.set_mode(each, cx));
                    })
                })
                .automation_target(format!("settings/mode/{}", each.key())),
            )
        });
    let materials = Material::ALL
        .into_iter()
        .fold(segmented(theme), |control, each| {
            let on = each == material;
            let enabled = palette.material(each) == each && support.supports(each);
            control.child(
                segment(
                    SharedString::from(format!("settings-material-{}", each.key())),
                    on,
                    enabled,
                    theme,
                )
                .debug_selector(move || format!("settings-material-{}", each.key()))
                .child(each.label())
                .when(!on && enabled, |this| {
                    this.on_click(move |_, _, cx| {
                        let settings = cx.services().settings.clone();
                        settings.update(cx, |settings, cx| settings.set_material(each, cx));
                    })
                })
                .automation_target(format!("settings/material/{}", each.key())),
            )
        });

    let card = div()
        .flex()
        .flex_col()
        .rounded(theme.radii.card)
        .border_1()
        .border_color(c.line)
        .bg(c.layer)
        .child(row(
            "Theme",
            "Aura is drawn on the window material; Claude and Dracula are opaque.".into(),
            None,
            palettes.into_any_element(),
            theme,
        ))
        .child(div().border_t_1().border_color(c.line).child(row(
            "Mode",
            mode_detail().into(),
            (!palette.has_light()).then(|| {
                format!(
                    "{} has no light palette: dv shows it dark, keeping the mode for the other \
                     themes.",
                    palette.label()
                )
                .into()
            }),
            modes.into_any_element(),
            theme,
        )))
        .child(
            div().border_t_1().border_color(c.line).child(row(
                "Material",
                "What shows through behind dv's surfaces: Mica tints them with the wallpaper, \
                 Acrylic blurs the windows behind dv."
                    .into(),
                material_note(support, palette, material),
                materials.into_any_element(),
                theme,
            )),
        );
    div()
        .flex()
        .flex_col()
        .gap(theme.spacing.md)
        .child(
            div()
                .mb(px(6.))
                .text_spec(&theme.text.title)
                .text_color(c.fg)
                .child("Appearance"),
        )
        .child(card)
        .automation_target("settings/appearance")
}

/// One setting: its name and what it does on the left, its control on the right, and `note`
/// (in `yellow`) under them when the platform holds it back.
fn row(
    title: &'static str,
    detail: SharedString,
    note: Option<SharedString>,
    control: AnyElement,
    theme: &DvTheme,
) -> AnyElement {
    let c = &theme.colors;
    div()
        .flex()
        .items_center()
        .gap(theme.spacing.xl)
        .p(theme.spacing.lg)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(
                    div()
                        .text_spec(&theme.text.secondary)
                        .text_color(c.fg2)
                        .child(detail),
                )
                .when_some(note, |this, note| {
                    this.child(
                        div()
                            .text_spec(&theme.text.secondary)
                            .text_color(c.yellow)
                            .child(note),
                    )
                }),
        )
        .child(control)
        .into_any_element()
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::System => "System",
        Mode::Dark => "Dark",
        Mode::Light => "Light",
    }
}

/// What the mode row says, naming the OS whose setting System follows.
fn mode_detail() -> &'static str {
    if cfg!(windows) {
        "Light or dark. System follows Windows' setting."
    } else if cfg!(target_os = "macos") {
        "Light or dark. System follows macOS's setting."
    } else {
        "Light or dark. System follows the desktop's setting."
    }
}

/// Why a material can't be chosen here, and what the window shows when the setting is one of
/// those (`platform`'s rules, PLAN D8; an opaque palette's, DESIGN §1.3).
fn material_note(
    support: MaterialSupport,
    palette: Palette,
    chosen: Material,
) -> Option<SharedString> {
    if !palette.is_glass() {
        let keeping = if chosen == Material::Opaque {
            String::new()
        } else {
            format!(", keeping {} for Aura", chosen.label())
        };
        return Some(
            format!(
                "Mica and Acrylic are Aura's: {} is opaque{keeping}.",
                palette.label()
            )
            .into(),
        );
    }
    let showing = (!support.supports(chosen)).then(|| {
        format!(
            " dv shows Opaque until then, keeping {} for when it can.",
            chosen.label()
        )
    });
    let why = if !cfg!(windows) {
        if support.acrylic || support.mica {
            return None;
        }
        let platform = if cfg!(target_os = "macos") {
            "macOS"
        } else {
            "Linux"
        };
        return Some(
            format!("Mica and Acrylic are Windows 11's: on {platform}, dv is Opaque.").into(),
        );
    } else if !support.acrylic {
        "Mica and Acrylic need Windows 11 with Transparency effects on (Windows Settings › \
         Personalization › Colors)."
    } else if !support.mica {
        "Mica needs Windows 11 22H2 or later."
    } else {
        return None;
    };
    Some(format!("{why}{}", showing.unwrap_or_default()).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_opaque_palette_says_so_keeping_the_material_for_aura() {
        assert_eq!(
            material_note(MaterialSupport::ALL, Palette::Claude, Material::Mica).as_deref(),
            Some("Mica and Acrylic are Aura's: Claude is opaque, keeping Mica for Aura.")
        );
        assert_eq!(
            material_note(MaterialSupport::ALL, Palette::Dracula, Material::Opaque).as_deref(),
            Some("Mica and Acrylic are Aura's: Dracula is opaque.")
        );
    }

    #[test]
    fn the_note_says_why_a_material_is_missing() {
        assert_eq!(
            material_note(MaterialSupport::ALL, Palette::Aura, Material::Mica),
            None
        );
        if cfg!(windows) {
            let off = material_note(
                MaterialSupport::OPAQUE_ONLY,
                Palette::Aura,
                Material::Acrylic,
            )
            .unwrap();
            assert!(off.contains("Transparency effects"), "{off}");
            assert!(off.contains("keeping Acrylic"), "{off}");
            let no_mica = MaterialSupport {
                mica: false,
                acrylic: true,
            };
            assert_eq!(
                material_note(no_mica, Palette::Aura, Material::Acrylic).as_deref(),
                Some("Mica needs Windows 11 22H2 or later.")
            );
            let showing = material_note(no_mica, Palette::Aura, Material::Mica).unwrap();
            assert!(showing.contains("keeping Mica"), "{showing}");
        } else {
            let note = material_note(
                MaterialSupport::OPAQUE_ONLY,
                Palette::Aura,
                Material::Opaque,
            )
            .unwrap();
            assert!(note.contains("dv is Opaque"), "{note}");
        }
    }
}
