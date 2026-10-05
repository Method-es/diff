//! The panel's comments over a real fixture repo: the rows in stream order with their states,
//! an outdated thread flagged with its original line, the resolved ones on demand, a click
//! showing the thread, and "Copy as prompt" through the clipboard seam.

use dv_core::review::{AnchorSide, Author, ThreadId, ThreadState, ops};
use dv_core::{Location, Target};
use dv_test_support::FixtureRepo;
use gpui::{Entity, TestAppContext, VisualTestContext};
use pretty_assertions::assert_eq;

use super::*;
use crate::services::ActiveServices as _;
use crate::services::reviews::OpenRequest;
use crate::shell::Shell;
use crate::system::SystemRequest;
use crate::test_support::{add_repo, bounds, open_shell, settle};

fn fixture() -> FixtureRepo {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "one\ntwo\nthree\n");
    repo.write("b.txt", "alpha\nbeta\n");
    repo.commit("base");
    repo.write("a.txt", "one\n2\nthree\n");
    repo.write("b.txt", "alpha\nBETA\n");
    repo
}

fn open<'a>(
    repo: &FixtureRepo,
    cx: &'a mut TestAppContext,
) -> (tempfile::TempDir, Entity<Shell>, &'a mut VisualTestContext) {
    let (dir, shell, cx) = open_shell(cx);
    add_repo(repo, &shell, cx);
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
    draw(cx);
    settle(cx, &shell, "the files to load", |shell, cx| {
        shell.session().is_some_and(|session| {
            let session = session.read(cx);
            session.files().len() == 2
                && session.files().iter().all(|entry| {
                    session
                        .file_state(&entry.path)
                        .is_some_and(|s| s.diff().is_some())
                })
        })
    });
    cx.foreground_executor()
        .block_test(task)
        .expect("the review opens");
    (dir, shell, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn session(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<ReviewSession> {
    shell.read_with(cx, |shell, _| shell.session().cloned().expect("a review"))
}

fn add_thread(
    shell: &Entity<Shell>,
    path: &str,
    line: u32,
    body: &str,
    cx: &mut VisualTestContext,
) -> ThreadId {
    let session = session(shell, cx);
    let adding = session.update(cx, |session, cx| {
        let anchor = session
            .anchor_for(path, AnchorSide::New, line, line)
            .expect("the lines are loaded");
        session.add_thread(anchor, body.to_string(), cx)
    });
    settle(cx, shell, "the thread", |_, _| true);
    cx.foreground_executor()
        .block_test(adding)
        .expect("the thread is added")
}

fn list_rows(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<CommentRow> {
    shell.read_with(cx, |shell, cx| shell.comments_list().read(cx).rows(cx))
}

#[gpui::test]
fn rows_list_waiting_threads_in_stream_order_and_copy_the_open_ones(cx: &mut TestAppContext) {
    let repo = fixture();
    let (_dir, shell, cx) = open(&repo, cx);
    // Added out of order: the list follows the files, then the lines.
    let on_b = add_thread(&shell, "b.txt", 2, "Why upper case?\nMore detail.", cx);
    let on_a = add_thread(&shell, "a.txt", 2, "Spell out two.", cx);
    let done = add_thread(&shell, "a.txt", 3, "Fine.", cx);
    // The agent addresses one, the reviewer resolves another.
    let session = session(&shell, cx);
    let (store, review) = session.read_with(cx, |session, _| {
        (session.store().clone(), session.review_id().clone())
    });
    ops::set_thread_state(
        &store,
        &review,
        &on_b,
        ThreadState::Addressed,
        Some("Matches the API."),
        Author::agent("claude-code"),
    )
    .expect("the agent addresses it");
    ops::set_thread_state(
        &store,
        &review,
        &done,
        ThreadState::Resolved,
        None,
        Author::reviewer("kyle"),
    )
    .expect("the reviewer resolves it");
    session.update(cx, |session, cx| session.reload_fold(cx));
    settle(cx, &shell, "the fold", |shell, cx| {
        shell.session().is_some_and(|session| {
            session
                .read(cx)
                .review()
                .thread(&done)
                .is_some_and(|thread| thread.state == ThreadState::Resolved)
        })
    });

    let rows = list_rows(&shell, cx);
    assert_eq!(
        rows,
        [
            CommentRow {
                id: on_a.clone(),
                state: ThreadState::Open,
                outdated: false,
                location: "a.txt:2".to_string(),
                summary: "Spell out two.".to_string(),
                snippet: None,
            },
            CommentRow {
                id: on_b.clone(),
                state: ThreadState::Addressed,
                outdated: false,
                location: "b.txt:2".to_string(),
                summary: "Why upper case?".to_string(),
                snippet: None,
            },
        ]
    );
    // The resolved one shows on demand, last.
    let list = shell.read_with(cx, |shell, _| shell.comments_list().clone());
    list.update(cx, |list, cx| list.set_show_resolved(true, cx));
    let ids: Vec<ThreadId> = list_rows(&shell, cx)
        .into_iter()
        .map(|row| row.id)
        .collect();
    assert_eq!(ids, [on_a.clone(), on_b.clone(), done]);

    // "Copy as prompt" hands over the open thread only, through the seam.
    list.update(cx, |list, cx| list.copy_as_prompt(cx));
    let copied = cx.update(|_, cx| crate::system::requests(cx).0);
    let Some(SystemRequest::Clipboard(text)) = copied.last() else {
        panic!("nothing was copied: {copied:?}");
    };
    assert!(text.contains("## a.txt:2"), "{text}");
    assert!(text.contains("Spell out two."), "{text}");
    assert!(!text.contains("b.txt"), "{text}");

    // A click on a row shows and focuses its thread.
    list.update(cx, |list, cx| list.reveal(&on_b, cx));
    let focused = shell.read_with(cx, |shell, cx| {
        shell.diff_pane().read(cx).focused_thread().cloned()
    });
    assert_eq!(focused, Some(on_b));
}

#[gpui::test]
fn an_outdated_thread_is_flagged_with_its_original_line(cx: &mut TestAppContext) {
    let repo = fixture();
    let (_dir, shell, cx) = open(&repo, cx);
    let id = add_thread(&shell, "a.txt", 2, "Spell out two.", cx);
    repo.write("a.txt", "one\nII\nthree\n");
    settle(cx, &shell, "the refresh", |shell, cx| {
        shell
            .comments_list()
            .read(cx)
            .rows(cx)
            .first()
            .is_some_and(|row| row.outdated)
    });
    let rows = list_rows(&shell, cx);
    assert_eq!(rows[0].id, id);
    assert_eq!(rows[0].snippet.as_deref(), Some("2"));
}

#[gpui::test]
fn the_header_fits_the_narrowest_panel(cx: &mut TestAppContext) {
    let repo = fixture();
    let (_dir, shell, cx) = open(&repo, cx);
    add_thread(&shell, "a.txt", 2, "Spell out two.", cx);
    let set_width = |width: f32, cx: &mut VisualTestContext| {
        let settings = cx.update(|_, cx| cx.services().settings.clone());
        settings.update(cx, |settings, cx| settings.set_panel_width(width, cx));
        draw(cx);
    };
    let edges = |cx: &mut VisualTestContext| {
        let panel = bounds(cx, "panel").expect("the panel");
        let copy = bounds(cx, "comments-copy").expect("the copy button");
        (panel.right(), copy.right(), copy.size.width)
    };
    // At 280 "Copy as prompt" has room beside the heading.
    set_width(280., cx);
    let (panel, copy, wide) = edges(cx);
    assert!(copy <= panel, "{copy:?} past {panel:?}");
    // At the minimum (220) it keeps its glyph, inside the panel.
    set_width(220., cx);
    let (panel, copy, narrow) = edges(cx);
    assert!(copy <= panel, "{copy:?} past {panel:?}");
    assert!(narrow < wide, "{narrow:?} vs {wide:?}");
    // It still copies.
    shell.update(cx, |shell, cx| {
        shell
            .comments_list()
            .update(cx, |list, cx| list.copy_as_prompt(cx))
    });
    let copied = cx.update(|_, cx| crate::system::requests(cx).0);
    assert!(matches!(copied.last(), Some(SystemRequest::Clipboard(_))));
}
