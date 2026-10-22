//! The palette (DESIGN §4.1 "Palette"): its rows for a filter, commands needing a review only
//! with one open, files only for a typed filter; and from the keyboard, a review opened, a
//! shell command and a stream command run, and `Esc` giving the keyboard back.

use dv_core::Target;
use gpui::{Entity, TestAppContext, VisualTestContext};
use pretty_assertions::assert_eq;

use super::*;
use crate::file_finder::candidates;
use crate::services::review_ui::ViewMode;
use crate::shell::Shell;
use crate::test_support::{add_repo, fixture_at, open_shell, settle};

fn keys(rows: &[Row]) -> Vec<&str> {
    rows.iter().map(|row| row.key.as_str()).collect()
}

#[test]
fn commands_needing_a_review_list_only_with_one_open() {
    let listing = items::Listing::default();
    let without = build("", false, &listing, &[]);
    assert!(without.iter().all(|row| row.section == Section::Commands));
    assert!(keys(&without).contains(&"command/shell::ToggleSwitcher"));
    assert!(!keys(&without).contains(&"command/diff_pane::NextFile"));
    assert!(!keys(&without).contains(&"command/shell::FindFile"));

    let with = build("", true, &listing, &[]);
    assert_eq!(with.len(), COMMANDS.len());
    assert_eq!(with[0].key, "command/shell::FindFile");
}

#[test]
fn every_word_must_be_in_a_command_and_files_list_only_for_a_filter() {
    let listing = items::Listing::default();
    let side = ["src/next.ts", "src/file.ts", "README.md"].map(String::from);
    let files = candidates(&side, &[("src/next.ts".to_string(), FileStatus::Modified)]);

    // An empty filter lists no files.
    assert!(
        build("", true, &listing, &files)
            .iter()
            .all(|row| row.section != Section::Files)
    );
    // Its words in a command's title or its other words, ignoring case; then the files.
    assert_eq!(
        keys(&build("NEXT file", true, &listing, &files)),
        ["command/diff_pane::NextFile"]
    );
    assert_eq!(
        keys(&build("next", true, &listing, &files)),
        [
            "command/diff_pane::NextFile",
            "command/diff_pane::NextChange",
            "command/diff_pane::NextThread",
            "file/src/next.ts",
        ]
    );
    assert_eq!(
        keys(&build("theme", false, &listing, &files)),
        ["command/shell::ToggleSettings"]
    );
    // No review open: no files either.
    assert!(build("readme", false, &listing, &files).is_empty());
}

fn palette(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<Entity<Palette>> {
    shell.read_with(cx, |shell, _| shell.palette().cloned())
}

fn selected(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<String> {
    palette(shell, cx)?.read_with(cx, |palette, _| palette.selected_key().map(str::to_string))
}

#[gpui::test]
fn ctrl_k_opens_reviews_and_runs_commands_by_keyboard(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
    let repos = tempfile::tempdir().unwrap();
    let api = fixture_at(&repos.path().join("api-server"));
    add_repo(&api, &shell, cx);

    cx.simulate_keystrokes("secondary-k");
    settle(cx, &shell, "the rows", |_, _| true);
    let view = palette(&shell, cx).expect("Ctrl K opens the palette");
    assert!(cx.update(|window, cx| view.read(cx).is_focused(window, cx)));

    // A repository's staged changes by name; Enter opens them and closes the palette.
    cx.simulate_input("api staged");
    settle(cx, &shell, "the filter", |_, _| true);
    assert_eq!(
        selected(&shell, cx).as_deref(),
        Some("review/api-server/staged")
    );
    cx.simulate_keystrokes("enter");
    settle(cx, &shell, "the review", |shell, _| {
        shell.session().is_some()
    });
    shell.read_with(cx, |shell, cx| {
        assert!(shell.palette().is_none());
        assert_eq!(
            shell.session().unwrap().read(cx).review().target,
            Target::Staged
        );
    });

    // A stream command runs in the main area: S's split / unified.
    let mode = |cx: &mut VisualTestContext| {
        shell.read_with(cx, |shell, cx| shell.diff_pane().read(cx).view_mode())
    };
    let before = mode(cx);
    cx.simulate_keystrokes("secondary-k");
    settle(cx, &shell, "the rows", |_, _| true);
    cx.simulate_input("split unified");
    cx.simulate_keystrokes("down up enter");
    settle(cx, &shell, "the command", |shell, _| {
        shell.palette().is_none()
    });
    let after = mode(cx);
    assert_ne!(before, after);
    assert!(matches!(after, ViewMode::Split | ViewMode::Unified));

    // A shell command: the panel hides.
    cx.simulate_keystrokes("secondary-k");
    settle(cx, &shell, "the rows", |_, _| true);
    cx.simulate_input("panel");
    cx.simulate_keystrokes("enter");
    settle(cx, &shell, "the command", |shell, _| !shell.panel_visible());

    // Esc closes it, the keyboard back in the main area; Ctrl K again closes it too.
    let main = shell.read_with(cx, |shell, cx| shell.diff_pane().focus_handle(cx));
    cx.simulate_keystrokes("secondary-k");
    assert!(palette(&shell, cx).is_some());
    cx.simulate_keystrokes("escape");
    assert!(palette(&shell, cx).is_none());
    assert!(cx.update(|window, _| main.is_focused(window)));
    cx.simulate_keystrokes("secondary-k");
    assert!(palette(&shell, cx).is_some());
    cx.simulate_keystrokes("secondary-k");
    assert!(palette(&shell, cx).is_none());
}

#[gpui::test]
fn over_the_sheet_the_reviews_commands_and_files_stay_out(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
    let repos = tempfile::tempdir().unwrap();
    let api = fixture_at(&repos.path().join("api-server"));
    add_repo(&api, &shell, cx);
    cx.simulate_keystrokes("secondary-k");
    settle(cx, &shell, "the rows", |_, _| true);
    cx.simulate_input("api uncommitted");
    settle(cx, &shell, "the filter", |_, _| true);
    cx.simulate_keystrokes("enter");
    settle(cx, &shell, "the review", |shell, _| {
        shell.session().is_some()
    });

    cx.simulate_keystrokes("secondary-enter");
    settle(cx, &shell, "the sheet", |shell, _| shell.sheet().is_some());
    cx.simulate_keystrokes("secondary-k");
    let view = palette(&shell, cx).expect("Ctrl K opens the palette over the sheet");
    view.read_with(cx, |palette, _| {
        assert!(palette.rows().iter().all(|row| match &row.kind {
            RowKind::Command(command) => !command.needs_review,
            RowKind::Review { .. } => true,
            RowKind::File { .. } => false,
        }));
        assert!(!keys(palette.rows()).is_empty());
    });
}
