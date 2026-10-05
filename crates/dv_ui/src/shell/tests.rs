use dv_core::{Location, Target};
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point};
use pretty_assertions::assert_eq;

use super::status_bar::Health;
use super::*;
use crate::services::Mode;
use crate::test_support::{add_repo, bounds, fixture, open_shell, settle};
use crate::theme::Appearance;

#[gpui::test]
fn the_empty_shell_shows_the_no_review_state(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
    cx.run_until_parked();
    cx.update(|window, cx| {
        let view = shell.read(cx);
        assert!(view.panel_visible());
        assert_eq!(view.empty_state(cx), Some(EmptyState::NoReview));
        assert_eq!(view.applied_material(), Material::platform_default());
        let status = view.status_line(cx);
        assert_eq!(status.health, Health::Idle);
        assert_eq!(status.location, "No repo");
        assert!(!status.finish);
        assert!(
            view.focus_handle.is_focused(window),
            "the shell has the keyboard, so Ctrl O works at once"
        );
    });
}

#[gpui::test]
fn a_material_change_reaches_the_window(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
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
    let (_dir, _shell, cx) = open_shell(cx);
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

#[gpui::test]
fn ctrl_b_toggles_the_panel(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
    assert_eq!(bounds(cx, "panel").map(|b| b.size.width), Some(px(280.)));
    cx.simulate_keystrokes("ctrl-b");
    assert!(!shell.read_with(cx, |shell, _| shell.panel_visible()));
    assert_eq!(bounds(cx, "panel"), None);
    assert_eq!(
        bounds(cx, "main").map(|b| (b.origin.x, b.size.width)),
        Some((px(0.), px(1440.))),
        "the main area takes the whole width"
    );
    cx.simulate_keystrokes("ctrl-b");
    assert!(shell.read_with(cx, |shell, _| shell.panel_visible()));
    assert_eq!(bounds(cx, "panel").map(|b| b.size.width), Some(px(280.)));
}

fn panel_width(cx: &mut VisualTestContext) -> f32 {
    cx.update(|_, cx| cx.services().settings.read(cx).panel_width())
}

#[gpui::test]
fn dragging_the_panel_edge_resizes_within_limits_and_is_saved(cx: &mut TestAppContext) {
    let (dir, shell, cx) = open_shell(cx);
    let handle = bounds(cx, "panel-resize-handle").expect("the handle");
    let start = handle.center();
    let drag_to = |cx: &mut VisualTestContext, x: f32| {
        cx.simulate_mouse_move(point(px(x), start.y), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
    };
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    // The first move past the threshold starts the drag; later moves resize.
    drag_to(cx, 300.);
    drag_to(cx, 330.);
    assert_eq!(panel_width(cx), 330.);
    drag_to(cx, 900.);
    assert_eq!(panel_width(cx), 420., "clamped to the maximum");
    drag_to(cx, 10.);
    assert_eq!(panel_width(cx), 220., "clamped to the minimum");
    drag_to(cx, 300.);
    cx.simulate_mouse_up(
        point(px(300.), start.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    assert_eq!(bounds(cx, "panel").map(|b| b.size.width), Some(px(300.)));
    settle(cx, &shell, "the settings to be saved", |_, _| true);
    let saved = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
    assert!(saved.contains("\"panel_width\": 300.0"), "{saved}");

    // A double click on the edge restores the default.
    let handle = bounds(cx, "panel-resize-handle").expect("the handle");
    cx.simulate_event(gpui::MouseDownEvent {
        position: handle.center(),
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(gpui::MouseUpEvent {
        position: handle.center(),
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 2,
    });
    assert_eq!(panel_width(cx), 280.);
}

fn open_review(
    shell: &Entity<Shell>,
    location: Location,
    request: OpenRequest,
    cx: &mut VisualTestContext,
) -> Result<Opened, Failure> {
    let task = cx.update(|window, cx| {
        shell.update(cx, |shell, cx| shell.open(location, request, window, cx))
    });
    cx.run_until_parked();
    settle(cx, shell, "the open to land", |shell, _| {
        shell.opening().is_none()
    });
    cx.foreground_executor().block_test(task)
}

#[gpui::test]
fn an_opened_review_fills_every_region(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
    let repo = fixture();
    add_repo(&repo, &shell, cx);
    let location = Location::Local(repo.path().to_path_buf());
    let opened = open_review(
        &shell,
        location.clone(),
        OpenRequest::Target(Target::Working),
        cx,
    )
    .expect("the review opens");
    assert!(opened.created);
    settle(cx, &shell, "the file list", |shell, cx| {
        shell
            .session()
            .is_some_and(|session| session.read(cx).files().len() == 3)
    });
    cx.update(|_, cx| {
        let view = shell.read(cx);
        assert_eq!(view.empty_state(cx), None);
        assert_eq!(view.banner().map(|b| b.failure.clone()), None);
        let status = view.status_line(cx);
        assert_eq!(status.health, Health::Live);
        assert_eq!(status.location, "Local · watching for changes");
        assert_eq!(
            status.review.as_deref(),
            Some("Open · round 1 · no comments")
        );
        assert!(status.finish);
        assert_eq!(
            view.file_list().read(cx).shown_paths(cx),
            [
                "README.md",
                "src/routes/users.test.ts",
                "src/routes/users.ts"
            ]
        );
        assert_eq!(
            view.diff_pane().read(cx).current_file(),
            Some("README.md"),
            "the stream starts at the first file"
        );
    });

    // Resuming the open review keeps its session.
    let session = shell.read_with(cx, |shell, _| shell.session().cloned().unwrap());
    let again = open_review(&shell, location, OpenRequest::Target(Target::Working), cx).unwrap();
    assert_eq!(again.review, opened.review);
    assert!(!again.created);
    shell.read_with(cx, |shell, _| {
        assert_eq!(
            shell.session().map(Entity::entity_id),
            Some(session.entity_id())
        )
    });
}

/// Presses `keys`, then draws, so focus changes take effect as they would on screen.
fn press(cx: &mut VisualTestContext, keys: &str) {
    cx.simulate_keystrokes(keys);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

#[gpui::test]
fn closing_an_overlay_never_strands_the_keyboard_on_a_hidden_filter(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
    let repo = fixture();
    add_repo(&repo, &shell, cx);
    open_review(
        &shell,
        Location::Local(repo.path().to_path_buf()),
        OpenRequest::Target(Target::Working),
        cx,
    )
    .unwrap();
    settle(cx, &shell, "the files", |shell, cx| {
        shell
            .session()
            .is_some_and(|session| session.read(cx).files().len() == 3)
    });
    let current = |cx: &mut VisualTestContext| {
        shell.read_with(cx, |shell, cx| {
            shell
                .diff_pane()
                .read(cx)
                .current_file()
                .map(str::to_string)
        })
    };
    assert_eq!(current(cx).as_deref(), Some("README.md"));
    // The switcher opens from the panel's filter, which the keyboard is to go back to...
    let filter = shell.read_with(cx, |shell, cx| shell.file_list().read(cx).filter_focus(cx));
    cx.update(|window, cx| filter.focus(window, cx));
    press(cx, "secondary-o");
    assert!(shell.read_with(cx, |shell, _| shell.switcher().is_some()));
    // ...but the panel goes away meanwhile.
    press(cx, "secondary-b");
    assert!(!shell.read_with(cx, |shell, _| shell.panel_visible()));
    press(cx, "escape");
    assert!(shell.read_with(cx, |shell, _| shell.switcher().is_none()));
    let main = shell.read_with(cx, |shell, _| shell.focus_handle.clone());
    assert!(
        cx.update(|window, _| main.is_focused(window)),
        "the keyboard is back on the main area"
    );
    // So the stream's and the shell's keys work.
    press(cx, "j");
    assert_eq!(current(cx).as_deref(), Some("src/routes/users.test.ts"));
    press(cx, "secondary-b");
    assert!(shell.read_with(cx, |shell, _| shell.panel_visible()));

    // Whatever else leaves the keyboard on an element that isn't drawn hands it to the main
    // area as the next frame is drawn.
    press(cx, "secondary-b");
    cx.update(|window, cx| filter.focus(window, cx));
    press(cx, "j");
    assert!(cx.update(|window, _| main.is_focused(window)));
    press(cx, "j");
    assert_eq!(current(cx).as_deref(), Some("src/routes/users.ts"));
}

#[gpui::test]
fn a_clean_working_tree_says_there_is_nothing_to_review(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
    let repo = dv_test_support::FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    add_repo(&repo, &shell, cx);
    open_review(
        &shell,
        Location::Local(repo.path().to_path_buf()),
        OpenRequest::Target(Target::Working),
        cx,
    )
    .unwrap();
    settle(cx, &shell, "the empty list", |shell, cx| {
        shell.empty_state(cx).is_some()
    });
    cx.update(|_, cx| {
        assert_eq!(
            shell.read(cx).empty_state(cx),
            Some(EmptyState::WorkingTreeClean)
        )
    });
    // A refresh (the agent touched a file) keeps saying so while it runs: no first-load
    // skeleton over it, in the main area or the panel.
    let session = shell.read_with(cx, |shell, _| shell.session().cloned().unwrap());
    session.update(cx, |session, cx| {
        session.refresh(crate::session::Batch::default(), cx)
    });
    cx.update(|_, cx| {
        let shell = shell.read(cx);
        assert!(session.read(cx).is_loading(), "the refresh runs");
        assert_eq!(shell.empty_state(cx), Some(EmptyState::WorkingTreeClean));
        assert!(!shell.file_list().read(cx).shows_skeletons());
    });
    settle(cx, &shell, "the refresh", |_, _| true);
    cx.update(|_, cx| {
        assert_eq!(
            shell.read(cx).empty_state(cx),
            Some(EmptyState::WorkingTreeClean)
        )
    });
}

#[gpui::test]
fn a_review_that_does_not_open_shows_a_banner_with_retry(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
    let wsl = Location::Wsl {
        distro: "Ubuntu".into(),
        path: "/home/kyle/web".into(),
    };
    let failure = open_review(&shell, wsl, OpenRequest::Target(Target::Working), cx)
        .expect_err("WSL can't be opened yet");
    assert!(
        failure.message.contains("WSL support arrives later"),
        "{failure}"
    );
    cx.update(|_, cx| {
        let view = shell.read(cx);
        let banner = view.banner().expect("a banner");
        assert_eq!(banner.failure, failure);
        assert!(banner.retry.is_some());
        assert_eq!(view.session().map(|_| ()), None);
        assert_eq!(view.empty_state(cx), Some(EmptyState::NoReview));
        let status = view.status_line(cx);
        assert_eq!(status.health, Health::Failing);
        assert_eq!(status.location, "Ubuntu · can't be opened");
    });
    // Retry runs the same open again, which fails the same way.
    cx.update(|window, cx| shell.update(cx, |shell, cx| shell.retry(window, cx)));
    cx.run_until_parked();
    settle(cx, &shell, "the retry", |shell, _| {
        shell.opening().is_none()
    });
    shell.read_with(cx, |shell, _| {
        assert_eq!(shell.banner().map(|b| &b.failure), Some(&failure))
    });
}
