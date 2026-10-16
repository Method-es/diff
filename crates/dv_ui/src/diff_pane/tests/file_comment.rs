//! Comments on a whole file in the stream (PLAN §8, DESIGN §4.1, §5) over a real fixture repo:
//! `Shift C` and the header menu's "Comment on file" open the composer heading the file, the
//! posted thread's card heads it ahead of an outdated card, `R` and `X` reach it, the comments
//! list names it as the file, a deleted file takes one on its old side, and a finished review
//! or an interdiff's deleted file takes none.

use dv_core::review::{ThreadId, ThreadState, Verdict};
use pretty_assertions::assert_eq;

use super::*;
use crate::session::ReviewSession;
use crate::thread::ComposerTarget;

fn session(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<ReviewSession> {
    shell.read_with(cx, |shell, _| {
        shell.session().cloned().expect("a review is open")
    })
}

fn composer_target(pane: &Entity<DiffPane>, cx: &mut VisualTestContext) -> Option<ComposerTarget> {
    pane.read_with(cx, |pane, _| {
        pane.composer().map(|composer| composer.target.clone())
    })
}

/// The cards heading `path`'s rows ([`FileSummary::heading`]).
fn heading(pane: &Entity<DiffPane>, path: &str, cx: &mut VisualTestContext) -> Vec<String> {
    summary(pane, path, cx).heading
}

/// The newest live thread.
fn newest(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> ThreadId {
    session(shell, cx).read_with(cx, |session, _| {
        session
            .review()
            .live_threads()
            .last()
            .map(|thread| thread.id.clone())
            .expect("a thread")
    })
}

#[gpui::test]
fn shift_c_comments_on_the_current_file_and_its_card_heads_the_file(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    press(cx, &shell, "j");
    assert_eq!(current(&pane, cx).as_deref(), Some("b.txt"));

    press(cx, &shell, "shift-c");
    assert_eq!(
        composer_target(&pane, cx),
        Some(ComposerTarget::file("b.txt".to_string(), AnchorSide::New))
    );
    assert_eq!(
        composer_target(&pane, cx).map(|target| target.title()),
        Some("Comment on file".to_string())
    );
    // The composer heads the file, under its header, above its first row.
    assert_eq!(heading(&pane, "b.txt", cx), ["composer"]);
    pane.read_with(cx, |pane, _| {
        let rows = &pane.blocks[1].rows;
        assert!(matches!(rows[0], Row::Header { .. }));
        assert_eq!(rows[1], Row::Heading(InsertItem::Composer));
        assert!(pane.composer_item().is_some());
    });
    // The field has the keyboard.
    cx.simulate_input("Split this file");
    press(cx, &shell, "secondary-enter");
    let id = newest(&shell, cx);
    let session = session(&shell, cx);
    session.read_with(cx, |session, _| {
        let thread = session.review().thread(&id).expect("the thread");
        assert!(thread.anchor.is_file_level(), "{:?}", thread.anchor);
        assert_eq!(thread.anchor.path, "b.txt");
        assert_eq!(thread.body, "Split this file");
        let placement = session.placement(thread);
        assert!(placement.is_file());
        assert_eq!(placement.key(thread), "file");
        assert_eq!(session.location(thread), "b.txt (file)");
    });
    pane.read_with(cx, |pane, _| {
        assert!(pane.composer().is_none());
        assert_eq!(pane.focused_thread(), Some(&id));
        assert!(pane.thread_item(&id).is_some());
    });
    assert_eq!(heading(&pane, "b.txt", cx), [id.to_string()]);

    // A line thread whose line the agent then deletes heads the file too, after it; the
    // composer of another comment on the file goes between the two.
    let line = session.update(cx, |session, cx| {
        let anchor = session
            .anchor_for("b.txt", AnchorSide::New, 2, 2)
            .expect("the lines are loaded");
        session.add_thread(anchor, "Why a digit?".to_string(), cx)
    });
    settle_drawn(cx, &shell, "the line thread", |_, _| true);
    let line = cx
        .foreground_executor()
        .block_test(line)
        .expect("the thread is added");
    repo.write("b.txt", "one\nthree\nfour\n");
    let watched = pane.clone();
    let wanted = vec![id.to_string(), line.to_string()];
    settle_drawn(cx, &shell, "the outdated card", move |_, cx| {
        watched
            .read(cx)
            .summary_of("b.txt")
            .is_some_and(|summary| summary.heading == wanted)
    });
    pane.update_in(cx, |pane, window, cx| {
        pane.comment_on_file("b.txt", window, cx)
    });
    settle_drawn(cx, &shell, "the composer", |_, _| true);
    assert_eq!(
        heading(&pane, "b.txt", cx),
        [id.to_string(), "composer".to_string(), line.to_string()]
    );
    press(cx, &shell, "escape");
    assert!(composer_target(&pane, cx).is_none());

    // The comments list names it as the file.
    session.read_with(cx, |session, _| {
        let rows = crate::comments_list::rows(session, false);
        let row = rows.iter().find(|row| row.id == id).expect("its row");
        assert!(row.file_level && !row.outdated);
        assert_eq!(row.location, "b.txt (file)");
        assert_eq!(row.snippet, None);
    });

    // `]` reaches its card, `R` replies in it, `X` resolves it.
    pane.update(cx, |pane, cx| {
        pane.focused_thread = None;
        pane.cursor = None;
        pane.go_to_file(0, cx);
    });
    settle_drawn(cx, &shell, "the top", |_, _| true);
    press(cx, &shell, "]");
    assert_eq!(
        pane.read_with(cx, |pane, _| pane.focused_thread().cloned()),
        Some(id.clone())
    );
    press(cx, &shell, "r");
    assert_eq!(
        composer_target(&pane, cx).and_then(|target| target.reply_to),
        Some(id.clone())
    );
    cx.simulate_input("Will do");
    press(cx, &shell, "secondary-enter");
    press(cx, &shell, "x");
    session.read_with(cx, |session, _| {
        let thread = session.review().thread(&id).expect("the thread");
        assert_eq!(thread.state, ThreadState::Resolved);
        assert_eq!(thread.replies[0].body, "Will do");
        // Nothing re-anchored a file standing where it was.
        assert_eq!(thread.original_anchor, None);
    });
}

#[gpui::test]
fn a_deleted_file_takes_a_comment_on_its_old_side_and_a_finished_review_none(
    cx: &mut TestAppContext,
) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let pane = pane(&shell, cx);
    // c.txt is deleted: the comment goes on its old side.
    pane.update_in(cx, |pane, window, cx| {
        pane.comment_on_file("c.txt", window, cx)
    });
    settle_drawn(cx, &shell, "the composer", |_, _| true);
    assert_eq!(
        composer_target(&pane, cx),
        Some(ComposerTarget::file("c.txt".to_string(), AnchorSide::Old))
    );
    assert_eq!(heading(&pane, "c.txt", cx), ["composer"]);
    cx.simulate_input("Why delete it?");
    press(cx, &shell, "secondary-enter");
    let id = newest(&shell, cx);
    let session = session(&shell, cx);
    session.read_with(cx, |session, _| {
        let thread = session.review().thread(&id).expect("the thread");
        assert!(thread.anchor.is_file_level());
        assert_eq!(thread.anchor.side, AnchorSide::Old);
        assert!(session.placement(thread).is_file());
        assert_eq!(session.location(thread), "c.txt (file, old)");
    });
    assert_eq!(heading(&pane, "c.txt", cx), [id.to_string()]);

    // Round 1 closes: the interdiff since it lists nothing, so no file takes a comment there.
    let finishing = session.update(cx, |session, cx| {
        session.finish_round(Some(Verdict::RequestChanges), None, cx)
    });
    settle_drawn(cx, &shell, "the interdiff", |shell, cx| {
        shell
            .session()
            .is_some_and(|session| session.read(cx).comparison().is_interdiff())
    });
    cx.foreground_executor()
        .block_test(finishing)
        .expect("the round finishes");
    session.read_with(cx, |session, _| {
        assert_eq!(session.file_comment_side("c.txt"), None);
        assert_eq!(session.file_comment_side("b.txt"), None);
    });
    // The agent deletes b.txt: the interdiff lists it deleted, and its old column there is the
    // round's snapshot, so it takes no comment on the whole file (the menu item dimmed).
    repo.remove("b.txt");
    settle_drawn(cx, &shell, "b.txt in the interdiff", |shell, cx| {
        shell.session().is_some_and(|session| {
            session
                .read(cx)
                .file("b.txt")
                .is_some_and(|entry| entry.status == dv_core::diff::FileStatus::Deleted)
        })
    });
    session.read_with(cx, |session, _| {
        assert!(session.comparison().is_interdiff());
        assert_eq!(session.file_comment_side("b.txt"), None);
    });
    pane.read_with(cx, |pane, cx| {
        assert!(!pane.can_comment_on_file("b.txt", cx));
    });
    pane.update_in(cx, |pane, window, cx| {
        pane.comment_on_file("b.txt", window, cx)
    });
    settle_drawn(cx, &shell, "nothing", |_, _| true);
    assert!(composer_target(&pane, cx).is_none());
    // In all changes the same deleted file takes one, on its old side.
    session.update(cx, |session, cx| {
        session.set_comparison(crate::session::Comparison::All, cx)
    });
    settle_drawn(cx, &shell, "all changes", |shell, cx| {
        shell
            .session()
            .is_some_and(|session| !session.read(cx).comparison().is_interdiff())
    });
    session.read_with(cx, |session, _| {
        assert_eq!(session.file_comment_side("b.txt"), Some(AnchorSide::Old));
    });
    // Approved, the review takes none at all, in all changes either.
    let approving = session.update(cx, |session, cx| {
        session.set_comparison(crate::session::Comparison::All, cx);
        session.finish_round(Some(Verdict::Approve), None, cx)
    });
    settle_drawn(cx, &shell, "the approval", |shell, cx| {
        shell
            .session()
            .is_some_and(|session| session.read(cx).is_read_only())
    });
    cx.foreground_executor()
        .block_test(approving)
        .expect("the round finishes");
    press(cx, &shell, "j shift-c");
    assert!(composer_target(&pane, cx).is_none());
    pane.read_with(cx, |pane, cx| {
        assert!(!pane.can_comment_on_file("b.txt", cx));
    });
}

impl DiffPane {
    /// `path`'s summary, if it is in the stream (tests).
    fn summary_of(&self, path: &str) -> Option<FileSummary> {
        self.file_summaries()
            .into_iter()
            .find(|file| file.path == path)
    }
}
