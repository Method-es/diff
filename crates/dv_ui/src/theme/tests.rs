use pretty_assertions::assert_eq;

use super::*;

/// `#rrggbb` and the alpha in hundredths of a resolved colour.
fn css(color: Hsla) -> (u32, u32) {
    let c = color.to_rgb();
    let byte = |v: f32| (v * 255.).round() as u32;
    (
        (byte(c.r) << 16) | (byte(c.g) << 8) | byte(c.b),
        (c.a * 1000.).round() as u32,
    )
}

#[test]
fn surfaces_take_the_materials_values_from_design() {
    let acrylic = DvColors::new(Appearance::Dark, Material::Acrylic);
    let mica = DvColors::new(Appearance::Dark, Material::Mica);
    let opaque = DvColors::new(Appearance::Dark, Material::Opaque);
    // DESIGN §1.1: chrome rgba(26,24,36,.56) / rgba(28,26,37,.82) / #110f18.
    assert_eq!(css(acrylic.chrome), (0x1a1824, 560));
    assert_eq!(css(mica.chrome), (0x1c1a25, 820));
    assert_eq!(css(opaque.chrome), (0x110f18, 1000));
    assert_eq!(css(mica.code), (0x15141b, 975));
    assert_eq!(css(opaque.input_bg), (0x0f0e15, 1000));

    let light = DvColors::new(Appearance::Light, Material::Mica);
    // DESIGN §1.2: chrome Mica rgba(239,238,244,.85), code rgba(254,254,255,.975).
    assert_eq!(css(light.chrome), (0xefeef4, 850));
    assert_eq!(css(light.code), (0xfefeff, 975));
    assert_eq!(css(light.fg), (0x1d1b26, 1000));
    assert_eq!(css(light.accent), (0x6d43e0, 1000));
}

#[test]
fn solid_tokens_match_design() {
    let dark = DvColors::new(Appearance::Dark, Material::Mica);
    assert_eq!(css(dark.fg), (0xedecee, 1000));
    assert_eq!(css(dark.fg3), (0x9d9ab3, 1000));
    assert_eq!(css(dark.accent), (0xa277ff, 1000));
    assert_eq!(css(dark.accent_text), (0xc4a9ff, 1000));
    assert_eq!(css(dark.line), (0xffffff, 80));
    assert_eq!(css(dark.scrim), (0x06050a, 560));
    assert_eq!(css(dark.add_bg), (0x61ffca, 85));
    assert_eq!(css(dark.syntax.property), (0xf694ff, 1000));
    // Comments are the lowest text tier in both appearances.
    assert_eq!(dark.syntax.comment, dark.fg3);
    let light = DvColors::new(Appearance::Light, Material::Mica);
    assert_eq!(css(light.syntax.comment), (0x5e5a6f, 1000));
    assert_eq!(light.syntax.comment, light.fg3);
    // Solid tokens don't depend on the material.
    let acrylic = DvColors::new(Appearance::Dark, Material::Acrylic);
    assert_eq!(dark.fg, acrylic.fg);
    assert_eq!(dark.line2, acrylic.line2);
    assert_eq!(dark.syntax, acrylic.syntax);
}

#[test]
fn opaque_surfaces_are_opaque_and_translucent_ones_keep_design_minimums() {
    for appearance in [Appearance::Dark, Appearance::Light] {
        let opaque = DvColors::new(appearance, Material::Opaque);
        for (name, color) in [
            ("chrome", opaque.chrome),
            ("panel", opaque.panel),
            ("code", opaque.code),
            ("code_head", opaque.code_head),
            ("layer_strong", opaque.layer_strong),
            ("thread", opaque.thread),
            ("overlay", opaque.overlay),
            ("ctrl", opaque.ctrl),
            ("input_bg", opaque.input_bg),
        ] {
            assert_eq!(color.a, 1., "{appearance:?} opaque {name}");
        }
        // DESIGN §3: the diff, threads and overlays stay near-opaque over the material (code
        // >= 94%, though DESIGN §1.2 gives light Acrylic's code 93%; thread >= 98%; overlays
        // >= 96.5%).
        for material in [Material::Mica, Material::Acrylic] {
            let c = DvColors::new(appearance, material);
            assert!(
                c.code.a >= 0.93,
                "{appearance:?} {material:?} code {}",
                c.code.a
            );
            assert!(c.thread.a >= 0.98, "{appearance:?} {material:?} thread");
            assert!(c.overlay.a >= 0.965, "{appearance:?} {material:?} overlay");
        }
    }
}

#[test]
fn the_window_background_shows_the_material_except_when_opaque() {
    for appearance in [Appearance::Dark, Appearance::Light] {
        for material in [Material::Mica, Material::Acrylic] {
            let c = DvColors::new(appearance, material);
            assert_eq!(c.window_background.a, 0., "{appearance:?} {material:?}");
        }
        let opaque = DvColors::new(appearance, Material::Opaque);
        assert_eq!(opaque.window_background, opaque.chrome);
    }
}

#[test]
fn the_lowest_text_tier_meets_the_contrast_floor_on_solid_surfaces() {
    // DESIGN §1.1: fg3 is >= 4.5:1 on every surface's Opaque value. Over Acrylic and Mica the
    // contrast also depends on the desktop behind the window (DESIGN §6), which no test controls.
    for appearance in [Appearance::Dark, Appearance::Light] {
        let c = DvColors::new(appearance, Material::Opaque);
        for (name, surface) in [
            ("chrome", c.chrome),
            ("panel", c.panel),
            ("code", c.code),
            ("code_head", c.code_head),
            ("thread", c.thread),
            ("overlay", c.overlay),
        ] {
            let ratio = contrast_ratio(c.fg3, surface);
            assert!(ratio >= 4.5, "{appearance:?} fg3 on {name}: {ratio:.2}");
        }
    }
}

#[test]
fn materials_round_trip_their_keys_and_map_to_gpui() {
    for material in Material::ALL {
        assert_eq!(Material::from_key(material.key()), Some(material));
        assert_eq!(
            serde_json::to_value(material).unwrap(),
            serde_json::Value::String(material.key().into())
        );
    }
    assert_eq!(Material::from_key("Mica"), None);
    assert_eq!(
        Material::Mica.window_background(),
        WindowBackgroundAppearance::MicaBackdrop
    );
    assert_eq!(
        Material::Acrylic.window_background(),
        WindowBackgroundAppearance::Blurred
    );
    assert_eq!(
        Material::Opaque.window_background(),
        WindowBackgroundAppearance::Opaque
    );
}

#[test]
fn sizes_follow_design() {
    let sizes = Sizes::default();
    assert_eq!(sizes.title_bar_height, px(40.));
    assert_eq!(sizes.status_bar_height, px(28.));
    assert_eq!(sizes.panel_width, px(280.));
    assert!(sizes.panel_min_width <= sizes.panel_width);
    assert!(sizes.panel_width <= sizes.panel_max_width);
    assert!(sizes.window_min_width <= sizes.window_width);
    assert!(sizes.window_min_height <= sizes.window_height);
}

#[test]
fn the_type_scale_uses_the_bundled_families() {
    let theme = DvTheme::new(Appearance::Dark, Material::Mica);
    assert_eq!(theme.text.body.family.as_ref(), fonts::GEIST);
    assert_eq!(theme.text.body.size, px(13.));
    assert_eq!(theme.text.body.line_height, px(18.));
    assert_eq!(theme.text.kbd.family.as_ref(), fonts::JETBRAINS_MONO);
    assert_eq!(theme.text.code.line_height, theme.sizes.code_row_height);
}

#[test]
fn the_component_theme_takes_the_palette() {
    let dv = DvTheme::new(Appearance::Light, Material::Opaque);
    let mut theme = Theme::default();
    map_component_theme(&dv, &mut theme);
    assert_eq!(theme.mode, ThemeMode::Light);
    assert_eq!(theme.background, dv.colors.chrome);
    assert_eq!(theme.tokens.background.color, dv.colors.chrome);
    assert_eq!(theme.foreground, dv.colors.fg);
    assert_eq!(theme.primary, dv.colors.accent);
    assert_eq!(theme.popover, dv.colors.overlay);
    assert_eq!(theme.font_family, dv.fonts.ui);
    assert_eq!(theme.mono_font_family, dv.fonts.mono);

    let mica = DvTheme::new(Appearance::Dark, Material::Mica);
    map_component_theme(&mica, &mut theme);
    assert_eq!(theme.mode, ThemeMode::Dark);
    assert_eq!(theme.tokens.background.color.a, 0., "Root lets Mica show");
}

#[gpui::test]
fn install_switches_the_theme_once(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        apply(cx);
        assert_eq!(cx.dv_theme().material, Material::Opaque);
        assert!(Theme::global(cx).is_dark());

        install(Appearance::Light, Material::Acrylic, cx);
        let dv = cx.dv_theme();
        assert_eq!(
            (dv.appearance, dv.material),
            (Appearance::Light, Material::Acrylic)
        );
        let theme = Theme::global(cx);
        assert!(!theme.is_dark());
        assert_eq!(theme.background.a, 0.);
        assert_eq!(theme.foreground, dv.colors.fg);
    });
}
