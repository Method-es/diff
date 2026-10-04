use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::ActiveServices as _;
use crate::platform::MaterialSupport;
use crate::services::{Mode, ServicesConfig};
use crate::theme::Appearance;

fn open(cx: &mut TestAppContext) -> (tempfile::TempDir, Entity<Shell>, &mut VisualTestContext) {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_path_buf();
    cx.update(|cx| {
        crate::init(cx).unwrap();
        crate::services::init(ServicesConfig { data_dir }, cx);
        crate::appearance::init(MaterialSupport::ALL, cx);
    });
    let (shell, cx) = cx.add_window_view(Shell::new);
    (dir, shell, cx)
}

#[gpui::test]
fn the_empty_shell_shows_the_no_review_state(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open(cx);
    cx.run_until_parked();
    shell.read_with(cx, |shell, _| {
        assert!(shell.panel_visible());
        assert_eq!(shell.empty_state(), Some(EmptyState::NoReview));
        assert_eq!(shell.applied_material(), Material::platform_default());
    });
}

#[gpui::test]
fn a_material_change_reaches_the_window(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open(cx);
    let settings = cx.update(|_, cx| cx.services().settings.clone());
    for material in [Material::Acrylic, Material::Opaque, Material::Mica] {
        settings.update(cx, |settings, cx| settings.set_material(material, cx));
        cx.run_until_parked();
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.applied_material(), material)
        });
    }
    settings.update(cx, |settings, cx| settings.set_mode(Mode::Light, cx));
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(cx.dv_theme().appearance, Appearance::Light));
}

#[gpui::test]
fn the_window_opens_at_its_size_with_the_material(cx: &mut TestAppContext) {
    let (_dir, _shell, cx) = open(cx);
    let options = cx.update(|_, cx| window_options(size(px(1440.), px(900.)), cx));
    assert_eq!(
        options.window_background,
        Material::platform_default().window_background()
    );
    assert_eq!(options.window_min_size, Some(size(px(1100.), px(700.))));
    let Some(WindowBounds::Windowed(bounds)) = options.window_bounds else {
        panic!("a windowed window");
    };
    assert_eq!(bounds.size, size(px(1440.), px(900.)));
}
