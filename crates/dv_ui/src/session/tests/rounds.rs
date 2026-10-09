//! Rounds in the session (PLAN §2.3, §8): the comparison a review opens on and moves to as
//! rounds close, the interdiff and all changes listed on demand, a missing round snapshot, the
//! interdiff's thread rule, and a moved thread recorded where it moved.

use dv_core::review::{Thread, ThreadId};
use pretty_assertions::assert_eq;

use super::*;

/// The review of `target` in `repo` with round 1 closed asking for changes, through the store
/// as another dv would have done it before this session opened.
fn closed_round_one(repo: &FixtureRepo, target: Target) -> ReviewId {
    let backend = backend(repo);
    let store = ReviewStore::new(backend.clone());
    let opened = ops::open_review(&store, target, reviewer()).unwrap();
    ops::finish_round(
        &store,
        &GitRepo::new(backend),
        &opened.review.id,
        Some(Verdict::RequestChanges),
        None,
        &reviewer(),
    )
    .unwrap();
    opened.review.id
}

fn thread<'a>(session: &'a ReviewSession, id: &ThreadId) -> &'a Thread {
    session.review().thread(id).expect("the thread")
}

#[test]
fn the_comparison_a_review_opens_on() {
    let repo = fixture();
    let store = ReviewStore::new(backend(&repo));
    let git = GitRepo::new(backend(&repo));
    let load = |id: &ReviewId| store.load(id).unwrap().review;
    let working = ops::open_review(&store, Target::Working, reviewer()).unwrap();
    // Nothing closed yet: all changes, no toggle.
    let review = load(&working.review.id);
    assert_eq!(default_comparison(&review), Comparison::All);
    assert!(!comparison_available(&review));
    ops::finish_round(
        &store,
        &git,
        &working.review.id,
        Some(Verdict::RequestChanges),
        None,
        &reviewer(),
    )
    .unwrap();
    let review = load(&working.review.id);
    assert_eq!(default_comparison(&review), Comparison::SinceRound(1));
    assert!(comparison_available(&review));
    // A fixed target's interdiff is always empty: never offered.
    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    let commit = ops::open_review(&store, Target::Commit { rev: head }, reviewer()).unwrap();
    ops::finish_round(
        &store,
        &git,
        &commit.review.id,
        Some(Verdict::RequestChanges),
        None,
        &reviewer(),
    )
    .unwrap();
    let review = load(&commit.review.id);
    assert_eq!(default_comparison(&review), Comparison::All);
    assert!(!comparison_available(&review));
    // A finished review shows all changes.
    ops::finish_round(
        &store,
        &git,
        &working.review.id,
        Some(Verdict::Approve),
        None,
        &reviewer(),
    )
    .unwrap();
    let review = load(&working.review.id);
    assert_eq!(default_comparison(&review), Comparison::All);
    assert!(!comparison_available(&review));
}

#[gpui::test]
fn a_review_with_a_closed_round_opens_on_the_interdiff(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture();
    closed_round_one(&repo, Target::Working);
    // The agent changes one file after the round.
    repo.write("src/a.rs", "fn a() -> u32 {\n    3\n}\n");
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the interdiff", |session| {
        !session.is_loading() && session.files().len() == 1
    });
    session.read_with(cx, |session, _| {
        assert_eq!(session.comparison(), Comparison::SinceRound(1));
        assert!(session.comparison_available());
        assert_eq!(paths(session), ["src/a.rs"]);
        assert_eq!(
            session.side_labels(),
            ("round 1".to_string(), "working tree".to_string())
        );
        let spec = session.spec().expect("listed");
        assert!(matches!(spec.old, dv_core::Side::Commit(_)));
        assert_eq!(spec.new, dv_core::Side::Worktree);
    });
    // The toggle lists all changes; again, the interdiff.
    session.update(cx, |session, cx| session.toggle_comparison(cx));
    session.read_with(cx, |session, _| {
        assert_eq!(session.asked_comparison(), Comparison::All);
        assert_eq!(
            session.comparison(),
            Comparison::SinceRound(1),
            "the files shown are the interdiff's until the new list lands"
        );
    });
    wait(cx, &session, "all changes", |session| {
        session.comparison() == Comparison::All
    });
    session.read_with(cx, |session, _| {
        assert_eq!(paths(session), ["notes.txt", "src/a.rs", "src/b.rs"]);
        assert_eq!(
            session.side_labels(),
            ("main".to_string(), "working tree".to_string())
        );
    });
    session.update(cx, |session, cx| session.toggle_comparison(cx));
    wait(cx, &session, "the interdiff again", |session| {
        session.comparison() == Comparison::SinceRound(1)
    });
    session.read_with(cx, |session, _| assert_eq!(paths(session), ["src/a.rs"]));
}

#[gpui::test]
fn closing_a_round_moves_the_comparison_to_since_that_round(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture();
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| {
        session.files().len() == 3
    });
    session.read_with(cx, |session, _| {
        assert_eq!(session.comparison(), Comparison::All);
        assert!(!session.comparison_available());
    });
    // `I` does nothing without a closed round.
    session.update(cx, |session, cx| session.toggle_comparison(cx));
    session.read_with(cx, |session, _| {
        assert_eq!(session.asked_comparison(), Comparison::All)
    });
    let finished = session.update(cx, |session, cx| {
        session.finish_round(Some(Verdict::RequestChanges), None, cx)
    });
    wait(cx, &session, "the interdiff since round 1", |session| {
        session.comparison() == Comparison::SinceRound(1)
    });
    cx.foreground_executor().block_test(finished).unwrap();
    // Nothing changed since the snapshot.
    session.read_with(cx, |session, _| assert!(session.files().is_empty()));
    repo.write("src/b.rs", "fn b() { let _ = 2; }\n");
    wait(cx, &session, "the agent's edit", |session| {
        paths(session) == ["src/b.rs"]
    });
    // Approving finishes the review: all changes, no toggle.
    let approved = session.update(cx, |session, cx| {
        session.finish_round(Some(Verdict::Approve), None, cx)
    });
    wait(cx, &session, "all changes", |session| {
        session.is_read_only() && session.comparison() == Comparison::All
    });
    cx.foreground_executor().block_test(approved).unwrap();
    session.read_with(cx, |session, _| {
        assert!(!session.comparison_available());
        assert_eq!(paths(session), ["notes.txt", "src/a.rs", "src/b.rs"]);
    });
}

#[gpui::test]
fn a_missing_snapshot_shows_all_changes_and_says_why(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture();
    let review = closed_round_one(&repo, Target::Working);
    // The round's ref deleted by hand and its commit pruned.
    repo.git(&["update-ref", "-d", &format!("refs/dv/reviews/{review}/1")]);
    repo.git(&["prune", "--expire=now"]);
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the fallback", |session| {
        !session.is_loading() && session.comparison_error().is_some()
    });
    session.read_with(cx, |session, _| {
        assert_eq!(session.comparison(), Comparison::All);
        assert_eq!(session.asked_comparison(), Comparison::All);
        assert_eq!(session.files().len(), 3);
        let error = session.comparison_error().unwrap();
        assert!(
            error
                .message
                .starts_with("Showing all changes: round 1's snapshot"),
            "{error:?}"
        );
        assert!(session.error().is_none(), "the review itself listed fine");
    });
    session.update(cx, |session, cx| session.dismiss_comparison_error(cx));
    session.read_with(cx, |session, _| {
        assert!(session.comparison_error().is_none())
    });
}

#[gpui::test]
fn the_interdiff_shows_new_side_threads_of_its_files_only(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture();
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| {
        session.files().len() == 3
    });
    let all: Vec<String> = ["src/a.rs", "src/b.rs"].map(String::from).to_vec();
    session.update(cx, |session, cx| session.ensure_loaded(all.clone(), cx));
    wait(cx, &session, "the diffs", |session| {
        all.iter()
            .all(|path| session.file_state(path).and_then(FileState::diff).is_some())
    });
    // Threads on a.rs's new side, b.rs's new side and a.rs's old side.
    let mut ids = Vec::new();
    for (path, side, line) in [
        ("src/a.rs", AnchorSide::New, 2),
        ("src/b.rs", AnchorSide::New, 1),
        ("src/a.rs", AnchorSide::Old, 2),
    ] {
        let anchor = session
            .read_with(cx, |session, _| session.anchor_for(path, side, line, line))
            .unwrap();
        let posted = session.update(cx, |session, cx| {
            session.add_thread(anchor, "Why?".to_string(), cx)
        });
        wait(cx, &session, "the thread", |_| true);
        ids.push(cx.foreground_executor().block_test(posted).unwrap());
    }
    let finished = session.update(cx, |session, cx| {
        session.finish_round(Some(Verdict::RequestChanges), None, cx)
    });
    wait(cx, &session, "the interdiff", |session| {
        session.comparison() == Comparison::SinceRound(1)
    });
    cx.foreground_executor().block_test(finished).unwrap();
    // The agent changes a.rs after the round, below the commented line.
    repo.write("src/a.rs", "fn a() -> u32 {\n    2\n}\n// done\n");
    wait(cx, &session, "a.rs in the interdiff", |session| {
        paths(session) == ["src/a.rs"]
    });
    session.update(cx, |session, cx| {
        session.ensure_loaded(["src/a.rs".to_string()], cx)
    });
    wait(cx, &session, "its diff", |session| {
        session
            .file_state("src/a.rs")
            .and_then(FileState::diff)
            .is_some()
    });
    session.read_with(cx, |session, _| {
        let keys: Vec<&str> = ids
            .iter()
            .map(|id| {
                session
                    .placement(thread(session, id))
                    .key(thread(session, id))
            })
            .collect();
        assert_eq!(
            keys,
            ["unchanged", "not_in_comparison", "not_in_comparison"]
        );
        let in_a: Vec<&ThreadId> = session.threads_in("src/a.rs").map(|t| &t.id).collect();
        assert_eq!(in_a, [&ids[0]]);
        assert_eq!(session.open_threads_in("src/a.rs"), 1);
        // Only the new side takes a comment here.
        assert!(
            session
                .anchor_for("src/a.rs", AnchorSide::Old, 1, 1)
                .is_err()
        );
        assert!(
            session
                .anchor_for("src/a.rs", AnchorSide::New, 4, 4)
                .is_ok()
        );
    });
    // In all changes every thread is placed again.
    session.update(cx, |session, cx| {
        session.set_comparison(Comparison::All, cx);
        session.ensure_loaded(all.clone(), cx);
    });
    wait(cx, &session, "all changes loaded", |session| {
        session.comparison() == Comparison::All
            && all
                .iter()
                .all(|path| session.file_state(path).and_then(FileState::diff).is_some())
    });
    session.read_with(cx, |session, _| {
        for id in &ids {
            let thread = thread(session, id);
            assert_eq!(session.placement(thread).key(thread), "unchanged");
        }
    });
}

#[gpui::test]
fn a_moved_thread_shows_moved_and_a_reply_records_where(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = FixtureRepo::new();
    let body: String = (1..=12).map(|n| format!("line {n}\n")).collect();
    repo.write("a.txt", &body);
    repo.commit("base");
    repo.write("a.txt", body.replace("line 6\n", "line six\n"));
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| {
        session.files().len() == 1
    });
    session.update(cx, |session, cx| {
        session.ensure_loaded(["a.txt".to_string()], cx)
    });
    wait(cx, &session, "the diff", |session| {
        session
            .file_state("a.txt")
            .and_then(FileState::diff)
            .is_some()
    });
    let anchor = session
        .read_with(cx, |session, _| {
            session.anchor_for("a.txt", AnchorSide::New, 6, 6)
        })
        .unwrap();
    let posted = session.update(cx, |session, cx| {
        session.add_thread(anchor, "Why six?".to_string(), cx)
    });
    wait(cx, &session, "the thread", |_| true);
    let id = cx.foreground_executor().block_test(posted).unwrap();
    // The agent inserts three lines above it.
    repo.write(
        "a.txt",
        body.replace("line 6\n", "line six\n")
            .replace("line 2\n", "line 2\nnew a\nnew b\nnew c\n"),
    );
    wait(cx, &session, "the move", |session| {
        let thread = thread(session, &id);
        session.placement(thread).lines(&thread.anchor) == Some((9, 9))
    });
    session.read_with(cx, |session, _| {
        let thread = thread(session, &id);
        let placement = session.placement(thread);
        assert_eq!(placement.key(thread), "moved");
        assert!(placement.shows_moved(thread));
        assert_eq!(placement.file(), Some("a.txt"));
        let relocated = session.relocated_anchor(&id).expect("a moved anchor");
        assert_eq!((relocated.start_line, relocated.end_line), (9, 9));
        assert_eq!(relocated.text, ["line six"]);
        assert_eq!(relocated.context_before, ["line 3", "line 4", "line 5"]);
    });
    // The reviewer's reply records the move; the thread then stands at its new lines and
    // still shows as moved.
    let replied = session.update(cx, |session, cx| {
        session.reply(id.clone(), "Still?".to_string(), cx)
    });
    wait(cx, &session, "the reply", |session| {
        thread(session, &id).replies.len() == 1
    });
    cx.foreground_executor().block_test(replied).unwrap();
    session.read_with(cx, |session, _| {
        let thread = thread(session, &id);
        assert_eq!(thread.anchor.start_line, 9);
        assert_eq!(
            thread.original_anchor.as_ref().map(|a| a.start_line),
            Some(6)
        );
        assert_eq!(
            session.relocated_anchor(&id),
            None,
            "nothing left to record"
        );
        let placement = session.placement(thread);
        assert_eq!(placement.key(thread), "moved");
        assert_eq!(placement.lines(&thread.anchor), Some((9, 9)));
    });
}
