//! The panel's comments over a real fixture repo: the rows in stream order with their states,
//! an outdated thread flagged with its original line, the resolved ones on demand, a click
//! showing the thread, "Copy as prompt" through the clipboard seam, and a row's right-click
//! menu (Resolve, Reopen, Reply, Copy as prompt) on a thread whose file left the diff.

use dv_core::review::{AnchorSide, Author, ThreadId, ThreadState, ops};
use dv_core::{Location, Target};
use dv_test_support::FixtureRepo;
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};
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
        None,
    )
    .expect("the agent addresses it");
    ops::set_thread_state(
        &store,
        &review,
        &done,
        ThreadState::Resolved,
        None,
        Author::reviewer("kyle"),
        None,
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

    // Addressed threads first (the reviewer's turn), then open ones (PLAN §3 workflow A).
    let rows = list_rows(&shell, cx);
    assert_eq!(
        rows,
        [
            CommentRow {
                id: on_b.clone(),
                state: ThreadState::Addressed,
                outdated: false,
                not_in_comparison: false,
                pending: false,
                location: "b.txt:2".to_string(),
                summary: "Why upper case?".to_string(),
                snippet: None,
            },
            CommentRow {
                id: on_a.clone(),
                state: ThreadState::Open,
                outdated: false,
                not_in_comparison: false,
                pending: false,
                location: "a.txt:2".to_string(),
                summary: "Spell out two.".to_string(),
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
    assert_eq!(ids, [on_b.clone(), on_a.clone(), done]);

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

#[test]
fn copy_as_prompt_says_when_a_thread_isnt_placed_yet() {
    let row = CommentRow {
        id: dv_core::review::ThreadId::generate(),
        state: ThreadState::Open,
        outdated: false,
        not_in_comparison: false,
        pending: false,
        location: "src/a.rs:12".to_string(),
        summary: "Why?".to_string(),
        snippet: None,
    };
    assert_eq!(super::prompt_location(&row), "src/a.rs:12");
    let waiting = CommentRow {
        pending: true,
        ..row
    };
    assert_eq!(
        super::prompt_location(&waiting),
        "src/a.rs:12 (where it was written; dv hasn't placed it yet)"
    );
}

/// Right-clicks `thread`'s row.
fn right_click_row(thread: &ThreadId, cx: &mut VisualTestContext) {
    let name: &'static str = Box::leak(format!("comment-row-{thread}").into_boxed_str());
    let row = bounds(cx, name).expect("the row is drawn");
    cx.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::none());
    draw(cx);
}

fn click(name: &'static str, cx: &mut VisualTestContext) {
    let item = bounds(cx, name).unwrap_or_else(|| panic!("{name} is drawn"));
    cx.simulate_click(item.center(), Modifiers::none());
    draw(cx);
}

fn thread_state(shell: &Entity<Shell>, id: &ThreadId, cx: &mut VisualTestContext) -> ThreadState {
    session(shell, cx).read_with(cx, |session, _| {
        session.review().thread(id).expect("the thread").state
    })
}

#[gpui::test]
fn a_rows_menu_resolves_replies_and_copies_its_thread(cx: &mut TestAppContext) {
    let repo = fixture();
    let (_dir, shell, cx) = open(&repo, cx);
    let on_a = add_thread(&shell, "a.txt", 2, "Spell out two.", cx);
    let on_b = add_thread(&shell, "b.txt", 2, "Why upper case?", cx);
    // The agent reverts b.txt: its thread goes to the stream's "Not in the diff" section,
    // which heads the stream, so its row comes first.
    repo.write("b.txt", "alpha\nbeta\n");
    settle(cx, &shell, "b.txt to leave", |shell, cx| {
        shell
            .session()
            .is_some_and(|session| session.read(cx).files().len() == 1)
    });
    draw(cx);
    let rows = list_rows(&shell, cx);
    let ids: Vec<&ThreadId> = rows.iter().map(|row| &row.id).collect();
    assert_eq!(ids, [&on_b, &on_a]);
    assert!(rows[0].outdated);
    assert_eq!(rows[0].location, "b.txt:2");
    let list = shell.read_with(cx, |shell, _| shell.comments_list().clone());

    // An open thread's menu: Resolve, Reply, Copy as prompt.
    right_click_row(&on_b, cx);
    assert_eq!(
        list.read_with(cx, |list, _| list.menu_thread().cloned()),
        Some(on_b.clone())
    );
    assert!(bounds(cx, "comments-menu-resolve").is_some());
    assert!(bounds(cx, "comments-menu-reopen").is_none());
    assert!(bounds(cx, "comments-menu-reply").is_some());
    // Copy as prompt: that thread alone, in "Copy as prompt"'s words.
    click("comments-menu-copy", cx);
    assert_eq!(
        list.read_with(cx, |list, _| list.menu_thread().cloned()),
        None
    );
    let copied = cx.update(|_, cx| crate::system::requests(cx).0);
    let Some(SystemRequest::Clipboard(text)) = copied.last() else {
        panic!("nothing was copied: {copied:?}");
    };
    assert!(text.contains("(1 comment)"), "{text}");
    assert!(text.contains("## b.txt:2"), "{text}");
    assert!(text.contains("Why upper case?"), "{text}");
    assert!(!text.contains("a.txt"), "{text}");

    // Resolve from the menu; the resolved thread's menu offers Reopen.
    right_click_row(&on_b, cx);
    click("comments-menu-resolve", cx);
    settle(cx, &shell, "the resolve", |_, _| true);
    assert_eq!(thread_state(&shell, &on_b, cx), ThreadState::Resolved);
    list.update(cx, |list, cx| list.set_show_resolved(true, cx));
    draw(cx);
    right_click_row(&on_b, cx);
    assert!(bounds(cx, "comments-menu-resolve").is_none());
    assert!(bounds(cx, "comments-menu-reopen").is_some());
    // Reply: the reply composer opens in the thread's card, in the section.
    click("comments-menu-reply", cx);
    settle(cx, &shell, "the reply composer", |_, _| true);
    draw(cx);
    let pane = shell.read_with(cx, |shell, _| shell.diff_pane().clone());
    pane.read_with(cx, |pane, _| {
        let composer = pane.composer().expect("a composer");
        assert_eq!(composer.target.reply_to.as_ref(), Some(&on_b));
        assert_eq!(pane.focused_thread(), Some(&on_b));
        // Drawn in the thread's card, in the section's block.
        assert_eq!(
            pane.composer_card(),
            Some((crate::diff_pane::SECTION, Some(&on_b)))
        );
    });
    cx.simulate_input("Gone with the file.");
    cx.simulate_keystrokes("secondary-enter");
    settle(cx, &shell, "the reply", |_, _| true);
    let replies = session(&shell, cx).read_with(cx, |session, _| {
        session
            .review()
            .thread(&on_b)
            .expect("the thread")
            .replies
            .len()
    });
    assert_eq!(replies, 1);

    // On a finished review the writes do nothing; Copy as prompt still copies.
    let finishing = session(&shell, cx).update(cx, |session, cx| {
        session.finish_round(Some(dv_core::review::Verdict::Approve), None, cx)
    });
    settle(cx, &shell, "the round", |_, _| true);
    cx.foreground_executor()
        .block_test(finishing)
        .expect("the round finishes");
    draw(cx);
    right_click_row(&on_a, cx);
    click("comments-menu-resolve", cx);
    settle(cx, &shell, "nothing", |_, _| true);
    assert_eq!(thread_state(&shell, &on_a, cx), ThreadState::Open);
    right_click_row(&on_a, cx);
    click("comments-menu-reply", cx);
    settle(cx, &shell, "nothing", |_, _| true);
    assert!(pane.read_with(cx, |pane, _| pane.composer().is_none()));
    // A click on a disabled item leaves the menu open; Copy as prompt still copies, headed
    // for the thread's state.
    assert_eq!(
        list.read_with(cx, |list, _| list.menu_thread().cloned()),
        Some(on_a.clone())
    );
    click("comments-menu-copy", cx);
    let copied = cx.update(|_, cx| crate::system::requests(cx).0);
    let Some(SystemRequest::Clipboard(text)) = copied.last() else {
        panic!("nothing was copied: {copied:?}");
    };
    assert!(text.starts_with("Open review comment on "), "{text}");
    // A resolved thread's copy says so.
    list.update(cx, |list, cx| list.copy_thread(&on_b, cx));
    let copied = cx.update(|_, cx| crate::system::requests(cx).0);
    let Some(SystemRequest::Clipboard(text)) = copied.last() else {
        panic!("nothing was copied: {copied:?}");
    };
    assert!(text.starts_with("Resolved review comment on "), "{text}");
}

#[gpui::test]
fn the_row_menu_closes_with_esc_an_overlay_the_panel_or_its_row(cx: &mut TestAppContext) {
    let repo = fixture();
    let (_dir, shell, cx) = open(&repo, cx);
    let on_a = add_thread(&shell, "a.txt", 2, "Spell out two.", cx);
    let list = shell.read_with(cx, |shell, _| shell.comments_list().clone());
    let menu =
        |cx: &mut VisualTestContext| list.read_with(cx, |list, _| list.menu_thread().cloned());
    // Esc, the stream having the keyboard.
    right_click_row(&on_a, cx);
    assert_eq!(menu(cx), Some(on_a.clone()));
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert_eq!(menu(cx), None);
    // The Finish round sheet opening.
    right_click_row(&on_a, cx);
    cx.update(|window, cx| shell.update(cx, |shell, cx| shell.open_sheet(window, cx)));
    draw(cx);
    assert_eq!(menu(cx), None);
    cx.simulate_keystrokes("escape");
    draw(cx);
    // The panel hiding.
    right_click_row(&on_a, cx);
    shell.update(cx, |shell, cx| shell.set_panel_visible(false, cx));
    draw(cx);
    assert_eq!(menu(cx), None);
    shell.update(cx, |shell, cx| shell.set_panel_visible(true, cx));
    draw(cx);
    // Its row leaving the list: the thread resolved (the CLI, say) while the resolved ones are
    // hidden.
    right_click_row(&on_a, cx);
    let session = session(&shell, cx);
    let (store, review) = session.read_with(cx, |session, _| {
        (session.store().clone(), session.review_id().clone())
    });
    ops::set_thread_state(
        &store,
        &review,
        &on_a,
        ThreadState::Resolved,
        None,
        Author::reviewer("kyle"),
        None,
    )
    .expect("the reviewer resolves it elsewhere");
    session.update(cx, |session, cx| session.reload_fold(cx));
    settle(cx, &shell, "the fold", |shell, cx| {
        shell.comments_list().read(cx).rows(cx).is_empty()
    });
    assert_eq!(menu(cx), None);
}
