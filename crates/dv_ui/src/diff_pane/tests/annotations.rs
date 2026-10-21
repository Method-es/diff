//! The agent's annotations in the stream (PLAN §19.3): a card where a thread's would be, `A`
//! hiding and showing them, `]`/`[` and `X` passing them over, `R` asking about one (which opens
//! it as a thread), and an interdiff drawing only those written since its round.

use dv_core::review::{Author, ThreadId, ThreadState, Verdict, ops};

use pretty_assertions::assert_eq;

use super::*;
use crate::session::ReviewSession;

fn session(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<ReviewSession> {
    shell.read_with(cx, |shell, _| {
        shell.session().cloned().expect("a review is open")
    })
}

fn state(shell: &Entity<Shell>, id: &ThreadId, cx: &mut VisualTestContext) -> ThreadState {
    session(shell, cx).read_with(cx, |session, _| {
        session.review().thread(id).expect("the thread").state
    })
}

/// The agent annotates line `line` of `path`'s new side as the CLI does (`ops::annotate`), and
/// the session reads it back.
fn annotate(
    shell: &Entity<Shell>,
    path: &str,
    line: u32,
    body: &str,
    cx: &mut VisualTestContext,
) -> ThreadId {
    let session = session(shell, cx);
    let (store, review, anchor) = session.read_with(cx, |session, _| {
        let anchor = session
            .anchor_for(path, AnchorSide::New, line, line)
            .expect("the lines are loaded");
        (session.store().clone(), session.review_id().clone(), anchor)
    });
    let id = ops::annotate(&store, &review, anchor, body, Author::agent("claude-code"))
        .expect("the agent annotates");
    session.update(cx, |session, cx| session.reload_fold(cx));
    let wanted = id.clone();
    settle_drawn(cx, shell, "the annotation", move |shell, cx| {
        shell
            .session()
            .is_some_and(|session| session.read(cx).review().thread(&wanted).is_some())
    });
    id
}

/// The reviewer's thread on line `line` of `path`'s new side.
fn comment(shell: &Entity<Shell>, path: &str, line: u32, cx: &mut VisualTestContext) -> ThreadId {
    let session = session(shell, cx);
    let adding = session.update(cx, |session, cx| {
        let anchor = session
            .anchor_for(path, AnchorSide::New, line, line)
            .expect("the lines are loaded");
        session.add_thread(anchor, "Why?".to_string(), cx)
    });
    settle_drawn(cx, shell, "the thread", |_, _| true);
    cx.foreground_executor()
        .block_test(adding)
        .expect("the thread is added")
}

fn card(shell: &Entity<Shell>, id: &ThreadId, cx: &mut VisualTestContext) -> bool {
    pane(shell, cx).read_with(cx, |pane, _| pane.thread_item(id).is_some())
}

#[gpui::test]
fn an_annotation_shows_beside_the_code_and_waits_on_no_one(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let note = annotate(&shell, "a/long.txt", 10, "Renamed: `ten` reads better.", cx);
    let asked = comment(&shell, "b.txt", 2, cx);
    assert!(card(&shell, &note, cx), "its card is in the stream");
    assert_eq!(state(&shell, &note, cx), ThreadState::Annotation);

    // `]` passes over it to the thread that waits on someone.
    let pane = pane(&shell, cx);
    pane.update(cx, |pane, cx| {
        pane.focused_thread = None;
        pane.list.scroll_to(gpui::ListOffset::default());
        pane.viewport_changed(cx);
    });
    press(cx, &shell, "]");
    assert_eq!(
        pane.read_with(cx, |pane, _| pane.focused_thread().cloned()),
        Some(asked.clone())
    );

    // `X` changes nothing on it.
    pane.update(cx, |pane, cx| pane.focus_thread(&note, cx));
    press(cx, &shell, "x");
    assert_eq!(state(&shell, &note, cx), ThreadState::Annotation);

    // `A` hides every annotation, and shows them again.
    press(cx, &shell, "a");
    assert!(!session(&shell, cx).read_with(cx, |session, _| session.annotations_shown()));
    assert!(!card(&shell, &note, cx), "hidden");
    assert!(card(&shell, &asked, cx), "a thread stays");
    press(cx, &shell, "a");
    assert!(card(&shell, &note, cx), "shown again");

    // `R` asks about it: posted, it is an open thread the agent answers.
    pane.update(cx, |pane, cx| pane.focus_thread(&note, cx));
    press(cx, &shell, "r");
    assert_eq!(
        pane.read_with(cx, |pane, _| pane
            .composer()
            .and_then(|composer| composer.target.reply_to.clone())),
        Some(note.clone())
    );
    cx.simulate_input("Why not keep the number?");
    press(cx, &shell, "secondary-enter");
    let wanted = note.clone();
    settle_drawn(cx, &shell, "the question", move |shell, cx| {
        shell.session().is_some_and(|session| {
            session
                .read(cx)
                .review()
                .thread(&wanted)
                .is_some_and(|thread| thread.state == ThreadState::Open)
        })
    });
    let asked_about = session(&shell, cx).read_with(cx, |session, _| {
        session.review().thread(&note).cloned().expect("the thread")
    });
    assert!(asked_about.annotation);
    assert_eq!(asked_about.replies.len(), 1);
    assert!(card(&shell, &note, cx));
}

#[gpui::test]
fn an_interdiff_draws_only_the_annotations_written_since_its_round(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let earlier = annotate(&shell, "a/long.txt", 10, "Round 1's reason.", cx);

    // Round 1 closes; the agent changes line 40 and explains it in round 2.
    let session = session(&shell, cx);
    let finishing = session.update(cx, |session, cx| {
        session.finish_round(Some(Verdict::RequestChanges), None, cx)
    });
    settle_drawn(cx, &shell, "the round", |_, _| true);
    cx.foreground_executor()
        .block_test(finishing)
        .expect("the round finishes");
    let long: String = (1..=80)
        .map(|i| match i {
            10 => "line ten\n".to_string(),
            40 => "line forty\n".to_string(),
            70 => "line seventy\n".to_string(),
            i => format!("line {i}\n"),
        })
        .collect();
    repo.write("a/long.txt", &long);
    let stream = pane(&shell, cx);
    settle_drawn(cx, &shell, "the agent's edit", move |_, cx| {
        let pane = stream.read(cx);
        pane.blocks.len() == 1 && pane.lines("a/long.txt").is_some()
    });
    assert_eq!(
        session.read_with(cx, |session, _| session.comparison()),
        crate::session::Comparison::SinceRound(1)
    );
    let later = annotate(&shell, "a/long.txt", 40, "Round 2's reason.", cx);

    let placed = |id: &ThreadId, cx: &mut VisualTestContext| {
        session.read_with(cx, |session, _| {
            let thread = session.review().thread(id).expect("the annotation");
            session.placement(thread).is_not_in_comparison()
        })
    };
    assert!(placed(&earlier, cx), "read already in round 1");
    assert!(!placed(&later, cx));
    assert!(!card(&shell, &earlier, cx));
    assert!(card(&shell, &later, cx));

    // All changes draws both.
    session.update(cx, |session, cx| {
        session.set_comparison(crate::session::Comparison::All, cx)
    });
    let stream = pane(&shell, cx);
    settle_drawn(cx, &shell, "all changes", move |_, cx| {
        stream.read(cx).blocks.len() == 3
    });
    assert!(card(&shell, &earlier, cx));
    assert!(card(&shell, &later, cx));
}

#[gpui::test]
fn a_question_being_written_keeps_its_annotations_card(cx: &mut TestAppContext) {
    let repo = stream_fixture(80);
    let (_dir, shell, cx) = open(&repo, cx);
    let note = annotate(&shell, "a/long.txt", 10, "Renamed on purpose.", cx);
    let pane = pane(&shell, cx);
    pane.update(cx, |pane, cx| pane.focus_thread(&note, cx));
    press(cx, &shell, "r");
    cx.simulate_input("Why");
    draw(cx);

    // `A` hides the other annotations; this one keeps its card and its draft, so the draft
    // stays where it can be posted or discarded.
    session(&shell, cx).update(cx, |session, cx| session.set_annotations_shown(false, cx));
    settle_drawn(cx, &shell, "the annotations hidden", |_, _| true);
    assert!(card(&shell, &note, cx), "the card being answered stays");
    assert!(pane.read_with(cx, |pane, _| pane.composer_item().is_some()));

    // Discarded, it goes with the others.
    pane.update_in(cx, |pane, window, cx| pane.cancel_composer(window, cx));
    settle_drawn(cx, &shell, "the composer gone", |_, _| true);
    assert!(!card(&shell, &note, cx));
}
