use dv_core::diff::{FileEntry, FileStatus};
use dv_core::review::{AuthorKind, ReviewStatus, ReviewStore, ops};
use dv_core::{Location, Target};
use gpui::{Entity, TestAppContext, VisualTestContext};
use pretty_assertions::assert_eq;

use super::*;
use crate::services::reviews::OpenRequest;
use crate::shell::Shell;
use crate::test_support::{add_repo, fixture, open_shell, settle};

fn entry(path: &str, old_path: Option<&str>) -> FileEntry {
    FileEntry {
        path: path.to_string(),
        old_path: old_path.map(str::to_string),
        status: FileStatus::Modified,
        additions: 1,
        deletions: 0,
        binary: false,
        old_mode: None,
        new_mode: None,
        old_oid: None,
        new_oid: None,
    }
}

#[test]
fn paths_split_into_directory_and_name() {
    assert_eq!(
        split_path("src/routes/users.ts"),
        ("src/routes/", "users.ts")
    );
    assert_eq!(split_path("README.md"), ("", "README.md"));
}

#[test]
fn the_filter_matches_every_word_in_either_path() {
    let users = entry("src/routes/users.ts", None);
    let moved = entry("src/api/accounts.ts", Some("src/routes/users_old.ts"));
    for (query, users_shown, moved_shown) in [
        ("", true, true),
        ("users", true, true),
        ("USERS routes", true, true),
        ("accounts", false, true),
        ("routes .ts users", true, true),
        ("users api", false, true),
        ("nothing", false, false),
    ] {
        assert_eq!(matches_filter(&users, query), users_shown, "{query:?}");
        assert_eq!(matches_filter(&moved, query), moved_shown, "{query:?}");
    }
}

#[test]
fn every_status_has_a_letter_and_a_word() {
    for (status, letter, hue) in [
        (FileStatus::Added, "A", Hue::Green),
        (FileStatus::Untracked, "U", Hue::Green),
        (FileStatus::Modified, "M", Hue::Yellow),
        (FileStatus::TypeChanged, "T", Hue::Yellow),
        (FileStatus::Deleted, "D", Hue::Red),
        (FileStatus::Unmerged, "!", Hue::Red),
        (FileStatus::Renamed { similarity: 90 }, "R", Hue::Blue),
        (FileStatus::Copied { similarity: 90 }, "C", Hue::Blue),
    ] {
        let shown = status_letter(status);
        assert_eq!((shown.letter, shown.hue), (letter, hue), "{status:?}");
        assert!(!shown.word.is_empty());
    }
}

/// A shell showing the working review of the fixture repo.
fn open_working(
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    dv_test_support::FixtureRepo,
    Entity<Shell>,
    &mut VisualTestContext,
) {
    let (dir, shell, cx) = open_shell(cx);
    let repo = fixture();
    add_repo(&repo, &shell, cx);
    let task = cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open(
                Location::Local(repo.path().to_path_buf()),
                OpenRequest::Target(Target::Working),
                window,
                cx,
            )
        })
    });
    cx.run_until_parked();
    settle(cx, &shell, "the file list", |shell, cx| {
        shell
            .session()
            .is_some_and(|session| session.read(cx).files().len() == 3)
    });
    cx.foreground_executor()
        .block_test(task)
        .expect("the review opens");
    (dir, repo, shell, cx)
}

fn file_list(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<FileList> {
    shell.read_with(cx, |shell, _| shell.file_list().clone())
}

fn shown(list: &Entity<FileList>, cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|_, cx| {
        list.read(cx)
            .shown_paths(cx)
            .into_iter()
            .map(str::to_string)
            .collect()
    })
}

fn current(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|_, cx| {
        shell
            .read(cx)
            .diff_pane()
            .read(cx)
            .current_file()
            .map(str::to_string)
    })
}

#[gpui::test]
fn typing_in_the_filter_narrows_the_rows_and_enter_shows_the_first(cx: &mut TestAppContext) {
    let (_dir, _repo, shell, cx) = open_working(cx);
    let list = file_list(&shell, cx);
    assert_eq!(
        shown(&list, cx),
        [
            "README.md",
            "src/routes/users.test.ts",
            "src/routes/users.ts"
        ]
    );
    assert_eq!(current(&shell, cx).as_deref(), Some("README.md"));

    // Clicking into the field and typing, as the reviewer would.
    let field = list.read_with(cx, |list, cx| list.filter_focus(cx));
    cx.update(|window, cx| field.focus(window, cx));
    cx.simulate_input("routes test");
    cx.run_until_parked();
    assert_eq!(shown(&list, cx), ["src/routes/users.test.ts"]);
    list.read_with(cx, |list, _| assert_eq!(list.query(), "routes test"));

    // Enter shows the first file left and hands the keyboard back to the shell.
    cx.simulate_keystrokes("enter");
    assert_eq!(
        current(&shell, cx).as_deref(),
        Some("src/routes/users.test.ts")
    );
    let shell_focus = shell.read_with(cx, gpui::Focusable::focus_handle);
    assert!(cx.update(|window, _| shell_focus.is_focused(window)));

    // Esc in the field clears it.
    cx.update(|window, cx| field.focus(window, cx));
    cx.simulate_keystrokes("escape");
    assert_eq!(shown(&list, cx).len(), 3);
    list.read_with(cx, |list, _| assert_eq!(list.query(), ""));
    assert!(cx.update(|window, _| shell_focus.is_focused(window)));

    // Hidden from inside the field, the panel hands the keyboard to the shell, so the shell's
    // keys keep working.
    cx.update(|window, cx| field.focus(window, cx));
    cx.simulate_keystrokes("ctrl-b");
    assert!(!shell.read_with(cx, |shell, _| shell.panel_visible()));
    assert!(cx.update(|window, _| shell_focus.is_focused(window)));
    cx.simulate_keystrokes("ctrl-b");
    assert!(shell.read_with(cx, |shell, _| shell.panel_visible()));

    // A filter nothing matches leaves no rows.
    list.update_in(cx, |list, window, cx| list.set_query("zzz", window, cx));
    assert!(shown(&list, cx).is_empty());
}

#[gpui::test]
fn the_viewed_checkbox_marks_the_file_through_the_store(cx: &mut TestAppContext) {
    let (_dir, _repo, shell, cx) = open_working(cx);
    let list = file_list(&shell, cx);
    let session = shell.read_with(cx, |shell, _| shell.session().cloned().unwrap());
    list.update(cx, |list, cx| list.toggle_viewed("src/routes/users.ts", cx));
    settle(cx, &shell, "the viewed mark", |_, cx| {
        session.read(cx).is_viewed("src/routes/users.ts")
    });
    // The mark is an event in the review's log, as another reader of the store sees it.
    let (backend, id) = session.read_with(cx, |session, _| {
        (session.backend().clone(), session.review_id().clone())
    });
    let stored = ReviewStore::new(backend.clone()).load(&id).unwrap().review;
    assert!(stored.is_viewed(1, "src/routes/users.ts"));
    session.read_with(cx, |session, _| assert_eq!(session.viewed_count(), 1));

    // A second click takes it back.
    list.update(cx, |list, cx| list.toggle_viewed("src/routes/users.ts", cx));
    settle(cx, &shell, "the viewed mark to go", |_, cx| {
        !session.read(cx).is_viewed("src/routes/users.ts")
    });
    assert!(
        !ReviewStore::new(backend.clone())
            .load(&id)
            .unwrap()
            .review
            .is_viewed(1, "src/routes/users.ts")
    );

    // A finished review is read-only: the checkbox writes nothing.
    ops::set_status(
        &ReviewStore::new(backend.clone()),
        &id,
        ReviewStatus::Approved,
        AuthorKind::Reviewer,
    )
    .unwrap();
    session.update(cx, |session, cx| session.reload_fold(cx));
    settle(cx, &shell, "the approval", |_, cx| {
        session.read(cx).is_read_only()
    });
    list.update(cx, |list, cx| list.toggle_viewed("README.md", cx));
    settle(cx, &shell, "nothing", |_, _| true);
    assert!(
        !ReviewStore::new(backend)
            .load(&id)
            .unwrap()
            .review
            .is_viewed(1, "README.md")
    );
}

#[gpui::test]
fn a_click_on_a_row_makes_it_the_current_file(cx: &mut TestAppContext) {
    let (_dir, _repo, shell, cx) = open_working(cx);
    let list = file_list(&shell, cx);
    list.update(cx, |list, cx| list.reveal("src/routes/users.ts", cx));
    assert_eq!(current(&shell, cx).as_deref(), Some("src/routes/users.ts"));
    // A path that isn't in the diff changes nothing.
    list.update(cx, |list, cx| list.reveal("nope.ts", cx));
    assert_eq!(current(&shell, cx).as_deref(), Some("src/routes/users.ts"));
}
