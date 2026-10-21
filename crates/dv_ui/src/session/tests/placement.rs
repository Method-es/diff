//! Placement in the session as the CLI places threads (PLAN §8, §10.1): a file shown too large,
//! files an interdiff lists that aren't among the review's changes, a file renamed again, and
//! placements kept across refreshes that don't touch what they were worked out from.

use dv_core::review::{Thread, ThreadId};
use pretty_assertions::assert_eq;

use super::*;

fn thread<'a>(session: &'a ReviewSession, id: &ThreadId) -> &'a Thread {
    session.review().thread(id).expect("the thread")
}

/// Loads `paths`' diffs and waits for them.
fn load(cx: &mut TestAppContext, session: &Entity<ReviewSession>, paths: &[&str]) {
    let paths: Vec<String> = paths.iter().map(|path| path.to_string()).collect();
    session.update(cx, |session, cx| session.ensure_loaded(paths.clone(), cx));
    wait(cx, session, "the diffs", |session| {
        paths
            .iter()
            .all(|path| session.file_state(path).and_then(FileState::diff).is_some())
    });
}

/// Comments on `path`'s new side, lines `from..=to`, as the reviewer, and waits for it.
fn comment(
    cx: &mut TestAppContext,
    session: &Entity<ReviewSession>,
    path: &str,
    from: u32,
    to: u32,
) -> ThreadId {
    let anchor = session
        .read_with(cx, |session, _| {
            session.anchor_for(path, AnchorSide::New, from, to)
        })
        .expect("the lines are loaded");
    let posted = session.update(cx, |session, cx| {
        session.add_thread(anchor, "Why?".to_string(), cx)
    });
    wait(cx, session, "the thread", |_| true);
    cx.foreground_executor().block_test(posted).unwrap()
}

fn finish(cx: &mut TestAppContext, session: &Entity<ReviewSession>, verdict: Verdict) {
    let finished = session.update(cx, |session, cx| {
        session.finish_round(Some(verdict), None, cx)
    });
    wait(cx, session, "the round", |session| {
        session.comparison() == default_comparison(session.review())
    });
    cx.foreground_executor().block_test(finished).unwrap();
}

#[gpui::test]
fn a_thread_in_a_file_too_large_to_show_is_placed_as_the_cli_places_it(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = FixtureRepo::new();
    repo.write("big.txt", "first\n");
    repo.commit("base");
    repo.write("big.txt", "first changed\n");
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| !session.is_loading());
    load(cx, &session, &["big.txt"]);
    let id = comment(cx, &session, "big.txt", 1, 1);
    // The agent grows the file past the cap; its first line stays.
    let big: String = std::iter::once("first changed\n".to_string())
        .chain((2..=24_000).map(|n| format!("row {n}\n")))
        .collect();
    repo.write("big.txt", &big);
    wait(cx, &session, "the cap", |session| {
        session
            .file_state("big.txt")
            .and_then(FileState::diff)
            .is_some_and(|diff| diff_kind(diff) == "too_large")
    });
    session.update(cx, |session, cx| {
        session.ensure_loaded(["big.txt".to_string()], cx)
    });
    // Its uncapped text is read in the background: the thread stands at its line, as
    // `dv comment list` says, not outdated.
    wait(cx, &session, "the full text", |session| {
        let thread = thread(session, &id);
        !session.placement(thread).is_pending()
    });
    session.read_with(cx, |session, _| {
        let thread = thread(session, &id);
        let placement = session.placement(thread);
        assert_eq!(placement.key(thread), "unchanged");
        assert_eq!(placement.lines(&thread.anchor), Some((1, 1)));
        assert!(!placement.is_outdated());
        assert_eq!(session.location(thread), "big.txt:1");
        assert_eq!(
            diff_kind(session.file_state("big.txt").unwrap().diff().unwrap()),
            "too_large",
            "the stream still shows it capped"
        );
    });
}

#[gpui::test]
fn a_comment_on_a_whole_file_too_large_to_show_reads_no_full_text(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = FixtureRepo::new();
    repo.write("big.txt", "first\n");
    repo.commit("base");
    let big: String = (1..=24_000).map(|n| format!("row {n}\n")).collect();
    repo.write("big.txt", &big);
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| !session.is_loading());
    load(cx, &session, &["big.txt"]);
    session.read_with(cx, |session, _| {
        assert_eq!(
            diff_kind(session.file_state("big.txt").unwrap().diff().unwrap()),
            "too_large"
        );
    });
    let anchor = session
        .read_with(cx, |session, _| {
            session.file_anchor_for("big.txt", AnchorSide::New)
        })
        .expect("a file-level anchor");
    let posted = session.update(cx, |session, cx| {
        session.add_thread(
            anchor,
            "Generated? Keep it out of the repo.".to_string(),
            cx,
        )
    });
    wait(cx, &session, "the thread", |_| true);
    let id = cx.foreground_executor().block_test(posted).unwrap();
    session.update(cx, |session, cx| {
        session.ensure_loaded(["big.txt".to_string()], cx)
    });
    wait(cx, &session, "settled", |session| session.is_settled());
    session.read_with(cx, |session, _| {
        let thread = thread(session, &id);
        assert!(session.placement(thread).is_file());
        // Placed without its text: the review's own diff of the file is never read for it.
        assert!(session.review_text("big.txt").is_none());
    });
}

#[gpui::test]
fn a_file_reverted_after_the_round_leaves_the_review_in_the_interdiff_too(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = FixtureRepo::new();
    repo.write("x.rs", "one\ntwo\nthree\nfour\nfive\nsix\n");
    repo.write("y.rs", "y\n");
    repo.commit("base");
    repo.write("x.rs", "one\ntwo\nTHREE\nfour\nfive\nsix\n");
    repo.write("y.rs", "y changed\n");
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| {
        session.files().len() == 2
    });
    load(cx, &session, &["x.rs", "y.rs"]);
    let changed = comment(cx, &session, "x.rs", 3, 3);
    let context = comment(cx, &session, "x.rs", 6, 6);
    finish(cx, &session, Verdict::RequestChanges);
    // The agent reverts x.rs: the interdiff lists it (snapshot → now), all changes don't.
    repo.git(&["checkout", "--", "x.rs"]);
    wait(cx, &session, "x.rs in the interdiff", |session| {
        paths(session) == ["x.rs"]
    });
    load(cx, &session, &["x.rs"]);
    wait(cx, &session, "x.rs read at the new side", |session| {
        session.is_settled()
    });
    session.read_with(cx, |session, _| {
        assert_eq!(session.comparison(), Comparison::SinceRound(1));
        assert!(!session.in_review("x.rs"));
        assert_eq!(
            session
                .target_files()
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["y.rs"]
        );
        // Its changed line is gone; its context line stands in the file at the new side, which
        // the review doesn't change now (PLAN §8), as `dv comment list` says.
        let thread_changed = thread(session, &changed);
        let placement = session.placement(thread_changed);
        assert_eq!(placement.key(thread_changed), "outdated");
        assert_eq!(placement.file(), None);
        let thread_context = thread(session, &context);
        let placement = session.placement(thread_context);
        assert_eq!(placement.key(thread_context), "unchanged");
        assert!(placement.is_unlisted() && placement.is_not_in_diff());
        assert_eq!(placement.lines(&thread_context.anchor), Some((6, 6)));
        assert_eq!(placement.file(), None);
        assert_eq!(session.location(thread_context), "x.rs:6");
        assert_eq!(session.threads_in("x.rs").count(), 0);
        // Nothing to comment on there: it isn't among the review's changes.
        let refused = session
            .anchor_for("x.rs", AnchorSide::New, 3, 3)
            .unwrap_err();
        assert!(
            refused
                .message
                .contains("isn't among this review's changes"),
            "{refused:?}"
        );
    });
}

#[gpui::test]
fn the_interdiff_names_a_file_by_the_reviews_own_rename(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = FixtureRepo::new();
    let body: String = (1..=20).map(|n| format!("line {n}\n")).collect();
    repo.write("a.rs", &body);
    repo.commit("base");
    // Renamed before round 1, with an edit.
    repo.git(&["mv", "a.rs", "b.rs"]);
    repo.write("b.rs", body.replace("line 5\n", "line five\n"));
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| {
        session.files().len() == 1
    });
    load(cx, &session, &["b.rs"]);
    let id = comment(cx, &session, "b.rs", 5, 5);
    session.read_with(cx, |session, _| {
        let anchor = &thread(session, &id).anchor;
        assert_eq!(
            (anchor.path.as_str(), anchor.old_path.as_deref()),
            ("b.rs", Some("a.rs"))
        );
    });
    finish(cx, &session, Verdict::RequestChanges);
    // The agent edits it further: in the interdiff it is plain b.rs (no rename since the
    // round), yet a comment there names the review's rename, as `dv comment add` does.
    repo.write(
        "b.rs",
        body.replace("line 5\n", "line five\n")
            .replace("line 15\n", "line fifteen\n"),
    );
    wait(cx, &session, "b.rs in the interdiff", |session| {
        paths(session) == ["b.rs"]
    });
    load(cx, &session, &["b.rs"]);
    session.read_with(cx, |session, _| {
        assert_eq!(session.file("b.rs").unwrap().old_path, None);
        let anchor = session.anchor_for("b.rs", AnchorSide::New, 15, 15).unwrap();
        assert_eq!(anchor.old_path.as_deref(), Some("a.rs"));
    });

    // Renamed again: git reports a.rs → c.rs against the base, and the thread follows.
    repo.git(&["mv", "b.rs", "c.rs"]);
    wait(cx, &session, "c.rs", |session| paths(session) == ["c.rs"]);
    load(cx, &session, &["c.rs"]);
    session.read_with(cx, |session, _| {
        let thread = thread(session, &id);
        let placement = session.placement(thread);
        assert_eq!(placement.file(), Some("c.rs"));
        assert_eq!(placement.lines(&thread.anchor), Some((5, 5)));
        assert_eq!(session.location(thread), "c.rs:5");
    });
    session.update(cx, |session, cx| {
        session.set_comparison(Comparison::All, cx)
    });
    wait(cx, &session, "all changes", |session| {
        session.comparison() == Comparison::All
    });
    load(cx, &session, &["c.rs"]);
    session.read_with(cx, |session, _| {
        let thread = thread(session, &id);
        assert_eq!(session.placement(thread).key(thread), "unchanged");
        assert_eq!(session.placement(thread).file(), Some("c.rs"));
    });
}

#[gpui::test]
fn a_refresh_that_keeps_a_files_diff_keeps_its_threads_placements(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture();
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| {
        session.files().len() == 3
    });
    load(cx, &session, &["src/a.rs", "src/b.rs"]);
    let id = comment(cx, &session, "src/a.rs", 2, 2);
    let placed = |cx: &mut TestAppContext| {
        session.read_with(cx, |session, _| {
            let _ = session.placement(thread(session, &id));
            session.placements_worked_out()
        })
    };
    let before = placed(cx);
    // The agent edits another file: the list refreshes, a.rs's diff stays.
    repo.write("src/b.rs", "fn b() { let _ = 3; }\n");
    wait(cx, &session, "b.rs again", |session| {
        new_side(session, "src/b.rs") == ["fn b() { let _ = 3; }"]
    });
    assert_eq!(placed(cx), before, "a.rs's thread wasn't placed again");
    // An edit to a.rs itself places it again.
    repo.write("src/a.rs", "// top\nfn a() -> u32 {\n    2\n}\n");
    load(cx, &session, &["src/a.rs"]);
    wait(cx, &session, "a.rs again", |session| {
        new_side(session, "src/a.rs").len() == 4
    });
    assert!(placed(cx) > before);
    session.read_with(cx, |session, _| {
        let thread = thread(session, &id);
        assert_eq!(
            session.placement(thread).lines(&thread.anchor),
            Some((3, 3))
        );
        assert_eq!(session.location(thread), "src/a.rs:3");
    });
}

#[gpui::test]
fn a_file_only_renamed_since_the_round_places_its_threads_as_all_changes_do(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let repo = FixtureRepo::new();
    let body: String = (1..=20).map(|n| format!("line {n}\n")).collect();
    repo.write("a.rs", &body);
    repo.commit("base");
    repo.write("a.rs", body.replace("line 5\n", "line five\n"));
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| {
        session.files().len() == 1
    });
    load(cx, &session, &["a.rs"]);
    let id = comment(cx, &session, "a.rs", 5, 5);
    finish(cx, &session, Verdict::RequestChanges);
    // Renamed, nothing else: the interdiff's diff of b.rs has no lines ("No content change"),
    // all changes' has the edit at line 5.
    repo.git(&["mv", "a.rs", "b.rs"]);
    wait(cx, &session, "b.rs", |session| paths(session) == ["b.rs"]);
    load(cx, &session, &["b.rs"]);
    wait(cx, &session, "the review's own diff of b.rs", |session| {
        !session.placement(thread(session, &id)).is_pending()
    });
    session.read_with(cx, |session, _| {
        assert!(!text(session, "b.rs").has_changes(), "no content change");
        let thread = thread(session, &id);
        let placement = session.placement(thread);
        assert_eq!(placement.key(thread), "unchanged");
        assert_eq!(placement.file(), Some("b.rs"));
        assert_eq!(placement.lines(&thread.anchor), Some((5, 5)));
    });
}

/// `count` numbered lines of distinct text, `line 1` to `line <count>`.
fn numbered(count: usize) -> String {
    (1..=count)
        .map(|n| format!("line {n} of the file\n"))
        .collect()
}

/// `body` with `count` comment lines inserted at its top.
fn with_lines_above(body: &str, count: usize) -> String {
    let top: String = (1..=count).map(|n| format!("// inserted {n}\n")).collect();
    format!("{top}{body}")
}

/// A thread on `path`'s new side at `line` of `body` (the working tree's text), in round 1,
/// added straight through `ops` as the CLI would.
fn thread_on(
    repo: &FixtureRepo,
    session: &Entity<ReviewSession>,
    cx: &mut TestAppContext,
    path: &str,
    body: &str,
    line: u32,
) -> ThreadId {
    let lines: Vec<&str> = body.lines().collect();
    let place = dv_core::review::AnchorPlace {
        round: 1,
        path: path.to_string(),
        old_path: None,
        side: AnchorSide::New,
    };
    let anchor = dv_core::review::anchor_selection(place, &lines, line, line).unwrap();
    let id = session.read_with(cx, |session, _| session.review_id().clone());
    let store = ReviewStore::new(backend(repo));
    let thread = ops::add_thread(&store, &id, anchor, "Why?", reviewer()).unwrap();
    session.update(cx, |session, cx| session.reload_fold(cx));
    thread
}

#[gpui::test]
fn a_file_the_agent_edits_away_from_the_viewport_places_its_threads_on_the_new_text(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let repo = FixtureRepo::new();
    let z = numbered(30);
    repo.write("a.rs", "fn a() {}\n");
    repo.write("z.rs", &z);
    repo.commit("base");
    repo.write("a.rs", "fn a() { 1 }\n");
    let z_edited = z.replace("line 20 of the file\n", "line twenty\n");
    repo.write("z.rs", &z_edited);
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| {
        session.files().len() == 2
    });
    load(cx, &session, &["z.rs"]);
    let id = comment(cx, &session, "z.rs", 10, 10);
    // The reviewer reads a.rs; the stream no longer asks for z.rs.
    load(cx, &session, &["a.rs"]);
    // The agent inserts lines at the top of z.rs.
    repo.write("z.rs", with_lines_above(&z_edited, 5));
    // The thread follows to line 15, as `dv comment list` says, though z.rs is nowhere near
    // the viewport: its file loads again because a thread shows in it.
    wait(cx, &session, "z.rs's thread on the new text", |session| {
        let thread = thread(session, &id);
        session.placement(thread).lines(&thread.anchor) == Some((15, 15))
    });
    session.read_with(cx, |session, _| {
        let thread = thread(session, &id);
        assert_eq!(session.placement(thread).key(thread), "moved");
        assert_eq!(session.location(thread), "z.rs:15");
        assert_eq!(
            session.file_state("z.rs").map(FileState::key),
            Some("loaded")
        );
        assert_eq!(
            session
                .relocated_anchor(&id)
                .map(|anchor| anchor.start_line),
            Some(15),
            "a write records where it is now"
        );
    });
}

#[gpui::test]
fn a_thread_in_a_file_the_stream_never_asked_for_is_placed(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = FixtureRepo::new();
    let z = numbered(30);
    repo.write("a.rs", "fn a() {}\n");
    repo.write("z.rs", &z);
    repo.commit("base");
    repo.write("a.rs", "fn a() { 1 }\n");
    let z_edited = z.replace("line 20 of the file\n", "line twenty\n");
    repo.write("z.rs", &z_edited);
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| {
        session.files().len() == 2
    });
    // The CLI comments on z.rs line 10, then the agent inserts lines above it; the stream
    // never asks for z.rs.
    let id = thread_on(&repo, &session, cx, "z.rs", &z_edited, 10);
    repo.write("z.rs", with_lines_above(&z_edited, 5));
    wait(cx, &session, "z.rs's thread placed", |session| {
        let thread = thread(session, &id);
        session.placement(thread).lines(&thread.anchor) == Some((15, 15))
    });
    session.read_with(cx, |session, _| {
        let thread = thread(session, &id);
        assert!(!session.placement(thread).is_pending());
        assert_eq!(session.location(thread), "z.rs:15");
        let rows = crate::comments_list::rows(session, false, false);
        assert_eq!(rows[0].location, "z.rs:15");
        assert!(!rows[0].pending);
        let prompt = crate::comments_list::prompt_threads(session);
        assert_eq!(prompt[0].1, "z.rs:15");
    });
}

#[gpui::test]
fn threads_the_interdiff_doesnt_show_are_named_where_they_stand_in_all_changes(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let repo = FixtureRepo::new();
    let p = numbered(40);
    repo.write("p.rs", &p);
    repo.write("q.rs", "q one\nq two\nq three\n");
    repo.commit("base");
    let p_edited = p.replace("line 30 of the file\n", "line thirty\n");
    repo.write("p.rs", &p_edited);
    repo.write("q.rs", "q one\nq 2\nq three\n");
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| {
        session.files().len() == 2
    });
    load(cx, &session, &["p.rs", "q.rs"]);
    let moved = comment(cx, &session, "p.rs", 10, 10);
    let gone = comment(cx, &session, "p.rs", 20, 20);
    let old_anchor = session
        .read_with(cx, |session, _| {
            session.anchor_for("q.rs", AnchorSide::Old, 2, 2)
        })
        .unwrap();
    let posted = session.update(cx, |session, cx| {
        session.add_thread(old_anchor, "Why change it?".to_string(), cx)
    });
    wait(cx, &session, "the old-side thread", |_| true);
    let old = cx.foreground_executor().block_test(posted).unwrap();
    finish(cx, &session, Verdict::RequestChanges);
    // Round 2: the agent inserts lines above the first thread and deletes the second's line;
    // the reviewer finishes it, so the interdiff since round 2 is empty.
    let round_two = with_lines_above(&p_edited, 5).replace("line 20 of the file\n", "");
    repo.write("p.rs", &round_two);
    wait(cx, &session, "p.rs in the interdiff", |session| {
        paths(session) == ["p.rs"]
    });
    finish(cx, &session, Verdict::RequestChanges);
    wait(cx, &session, "the empty interdiff", |session| {
        session.comparison() == Comparison::SinceRound(2) && session.files().is_empty()
    });
    session.read_with(cx, |session, _| {
        let thread = |id| thread(session, id);
        let placement = session.placement(thread(&moved));
        assert_eq!(placement.key(thread(&moved)), "not_in_comparison");
        assert_eq!(placement.lines(&thread(&moved).anchor), Some((15, 15)));
        assert_eq!(session.location(thread(&moved)), "p.rs:15");
        // Its line deleted: outdated, as `dv comment list` says, though not drawn either.
        let placement = session.placement(thread(&gone));
        assert!(placement.is_not_in_comparison() && placement.is_outdated());
        // On the old side, where it was written.
        assert_eq!(session.location(thread(&old)), "q.rs:2 (old)");
        let rows = crate::comments_list::rows(session, false, false);
        let row = |id: &ThreadId| rows.iter().find(|row| &row.id == id).unwrap();
        assert!(row(&moved).not_in_comparison && !row(&moved).outdated);
        assert_eq!(row(&moved).location, "p.rs:15");
        assert!(row(&gone).not_in_comparison && row(&gone).outdated);
    });
}

#[gpui::test]
fn a_full_text_read_that_fails_leaves_its_thread_outdated_not_waiting(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = FixtureRepo::new();
    repo.write("big.txt", "first\n");
    repo.commit("base");
    let big: String = std::iter::once("first changed\n".to_string())
        .chain((2..=24_000).map(|n| format!("row {n}\n")))
        .collect();
    repo.write("big.txt", "first changed\n");
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| !session.is_loading());
    load(cx, &session, &["big.txt"]);
    let id = comment(cx, &session, "big.txt", 1, 1);
    repo.write("big.txt", &big);
    wait(cx, &session, "the cap and the full text", |session| {
        let thread = thread(session, &id);
        session
            .file_state("big.txt")
            .and_then(FileState::diff)
            .is_some_and(|diff| diff_kind(diff) == "too_large")
            && !session.placement(thread).is_pending()
    });
    // The full text is read again, and placement asked for while that runs (as the stream
    // draws), then the read fails: the thread is outdated (the diff shown is all there is),
    // not left waiting on the placement worked out while it ran.
    session.update(cx, |session, cx| {
        session.review_texts.clear();
        session.ensure_review_texts(cx);
        let thread = session.review().thread(&id).unwrap().clone();
        assert!(session.placement(&thread).is_pending());
        session.fail_review_text("big.txt", cx);
        let placement = session.placement(&thread);
        assert!(placement.is_outdated(), "{placement:?}");
        assert_eq!(placement.file(), Some("big.txt"));
    });
}

#[gpui::test]
fn no_thread_has_left_the_review_before_a_list_lands(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture();
    let first = session(&repo, Target::Working, cx);
    wait(cx, &first, "the list", |session| !session.is_loading());
    load(cx, &first, &["notes.txt"]);
    let id = comment(cx, &first, "notes.txt", 1, 1);
    // The agent deletes the untracked file: the thread's file leaves the review.
    repo.remove("notes.txt");
    // The review opened again: until its first list lands the thread waits, at the path it
    // was written on, rather than going to the "Not in the diff" section.
    let again = session(&repo, Target::Working, cx);
    again.read_with(cx, |session, _| {
        assert!(session.is_first_load());
        let placement = session.placement(thread(session, &id));
        assert!(placement.is_pending(), "{placement:?}");
        assert!(!placement.is_not_in_diff());
        assert_eq!(session.threads_not_in_diff().count(), 0);
    });
    wait(cx, &again, "the list", |session| !session.is_loading());
    again.read_with(cx, |session, _| {
        let placement = session.placement(thread(session, &id));
        assert!(placement.is_not_in_diff(), "{placement:?}");
        let ids: Vec<&ThreadId> = session.threads_not_in_diff().map(|t| &t.id).collect();
        assert_eq!(ids, [&id]);
    });
}

/// The hook an agent wrote and never committed (PLAN §8: a file that only ever existed on the
/// branch), renamed between rounds.
const USE_CAN: &str = "import { useAuth } from './use-auth';\n\nexport function useCan(permission: string) {\n  const { user } = useAuth();\n  if (!user) {\n    return false;\n  }\n  return user.permissions.includes(permission);\n}\n";

#[gpui::test]
fn a_thread_follows_its_untracked_file_renamed_after_the_round(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = FixtureRepo::new();
    repo.write("README.md", "# web\n");
    repo.commit("base");
    repo.write("src/use-can.ts", USE_CAN);
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| {
        session.files().len() == 1
    });
    load(cx, &session, &["src/use-can.ts"]);
    let id = comment(cx, &session, "src/use-can.ts", 8, 8);
    finish(cx, &session, Verdict::RequestChanges);
    // The agent renames it: the review lists the new name alone, untracked.
    repo.remove("src/use-can.ts");
    repo.write(
        "src/use-has-permission.ts",
        USE_CAN.replace("useCan", "useHasPermission"),
    );
    wait(cx, &session, "the rename to be followed", |session| {
        let thread = thread(session, &id);
        session.placement(thread).file() == Some("src/use-has-permission.ts")
    });
    session.read_with(cx, |session, _| {
        let thread = thread(session, &id);
        let placement = session.placement(thread);
        assert_eq!(
            placement.key(thread),
            "unchanged",
            "as dv comment list says"
        );
        assert!(!placement.is_not_in_diff());
        assert_eq!(placement.lines(&thread.anchor), Some((8, 8)));
        assert_eq!(session.location(thread), "src/use-has-permission.ts:8");
        assert_eq!(session.threads_not_in_diff().count(), 0);
        // A reply records it under the file's name now.
        let recorded = session
            .relocated_anchor(&id)
            .expect("an anchor under the new name");
        assert_eq!(recorded.path, "src/use-has-permission.ts");
    });
}

#[gpui::test]
fn a_comment_on_a_whole_file_follows_its_file_and_leaves_the_diff_with_it(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = FixtureRepo::new();
    repo.write("README.md", "# web\n");
    repo.commit("base");
    repo.write("src/use-can.ts", USE_CAN);
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| {
        session.files().len() == 1
    });
    // No text is read for it: its file needn't load.
    let anchor = session
        .read_with(cx, |session, _| {
            assert_eq!(
                session.file_comment_side("src/use-can.ts"),
                Some(AnchorSide::New)
            );
            session.file_anchor_for("src/use-can.ts", AnchorSide::New)
        })
        .expect("a file-level anchor");
    assert!(anchor.is_file_level());
    let posted = session.update(cx, |session, cx| {
        session.add_thread(anchor, "Rename this hook".to_string(), cx)
    });
    wait(cx, &session, "the thread", |_| true);
    let id = cx.foreground_executor().block_test(posted).unwrap();
    session.read_with(cx, |session, _| {
        let thread = thread(session, &id);
        let placement = session.placement(thread);
        assert!(placement.is_file(), "{placement:?}");
        assert_eq!(placement.key(thread), "file");
        assert_eq!(session.location(thread), "src/use-can.ts (file)");
        assert!(session.thread_files().is_empty());
        assert_eq!(session.relocated_anchor(&id), None);
    });
    finish(cx, &session, Verdict::RequestChanges);
    // The agent renames it, untracked: its round's history follows it by content.
    repo.remove("src/use-can.ts");
    repo.write(
        "src/use-has-permission.ts",
        USE_CAN.replace("useCan", "useHasPermission"),
    );
    wait(cx, &session, "the rename to be followed", |session| {
        let thread = thread(session, &id);
        session.placement(thread).file() == Some("src/use-has-permission.ts")
    });
    session.read_with(cx, |session, _| {
        let thread = thread(session, &id);
        assert!(session.placement(thread).is_file());
        assert_eq!(session.location(thread), "src/use-has-permission.ts (file)");
        let recorded = session
            .relocated_anchor(&id)
            .expect("an anchor under the new name");
        assert!(recorded.is_file_level());
        assert_eq!(recorded.path, "src/use-has-permission.ts");
    });
    // Deleted too, it leaves the diff: the "Not in the diff" section.
    repo.remove("src/use-has-permission.ts");
    wait(cx, &session, "the file to leave", |session| {
        session.threads_not_in_diff().count() == 1
    });
    session.read_with(cx, |session, _| {
        let thread = thread(session, &id);
        assert!(session.placement(thread).is_not_in_diff());
        assert_eq!(session.location(thread), "src/use-can.ts (file)");
    });
}

#[gpui::test]
fn code_rewritten_after_the_round_is_outdated_where_it_went(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = FixtureRepo::new();
    let lines: Vec<String> = (1..=30)
        .map(|n| format!("let value_{n} = compute({n});"))
        .collect();
    repo.write("x.rs", lines.join("\n") + "\n");
    repo.commit("base");
    let mut edited = lines.clone();
    edited[11] = "let value_12 = compute_twice(12);".to_string();
    repo.write("x.rs", edited.join("\n") + "\n");
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "the list", |session| {
        session.files().len() == 1
    });
    load(cx, &session, &["x.rs"]);
    let id = comment(cx, &session, "x.rs", 12, 12);
    finish(cx, &session, Verdict::RequestChanges);
    // The agent replaces the line with two that share nothing with it.
    edited.splice(11..12, ["struct Q;".to_string(), "impl Q {}".to_string()]);
    repo.write("x.rs", edited.join("\n") + "\n");
    wait(cx, &session, "its trail", |session| {
        session.placement(thread(session, &id)).trail().is_some()
    });
    session.read_with(cx, |session, _| {
        let thread = thread(session, &id);
        let placement = session.placement(thread);
        assert!(placement.is_outdated());
        assert_eq!(placement.key(thread), "outdated", "as dv comment list says");
        assert_eq!(placement.lines(&thread.anchor), None);
        assert_eq!(
            placement.trail().map(|trail| (trail.round, trail.lines())),
            Some((1, (12, 13)))
        );
        assert_eq!(session.location(thread), "x.rs:12–13");
        // A reply records no anchor from it.
        assert_eq!(session.relocated_anchor(&id), None);
    });
}

#[gpui::test]
fn a_thread_placed_against_the_full_read_asks_for_its_history_once_that_lands(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let repo = FixtureRepo::new();
    let lines: Vec<String> = (1..=30)
        .map(|n| format!("let value_{n} = compute({n});"))
        .collect();
    repo.write("x.rs", lines.join("\n") + "\n");
    repo.write("y.rs", "y\n");
    repo.commit("base");
    let mut edited = lines.clone();
    edited[11] = "let value_12 = compute_twice(12);".to_string();
    repo.write("x.rs", edited.join("\n") + "\n");
    repo.write("y.rs", "y changed\n");
    // Before the app opens the review: a thread on x.rs's line 12 in round 1, round 1 closed,
    // the agent rewrites that line, round 2 closed, then an edit to y.rs alone.
    let backend = backend(&repo);
    let store = ReviewStore::new(backend.clone());
    let git = GitRepo::new(backend);
    let review = ops::open_review(&store, Target::Working, reviewer())
        .unwrap()
        .review;
    let anchor = dv_core::review::anchor_selection(
        dv_core::review::AnchorPlace {
            round: 1,
            path: "x.rs".to_string(),
            old_path: None,
            side: AnchorSide::New,
        },
        &edited,
        12,
        12,
    )
    .unwrap();
    let id = ops::add_thread(&store, &review.id, anchor, "Why twice?", reviewer()).unwrap();
    let finish_now = || {
        ops::finish_round(
            &store,
            &git,
            &review.id,
            Some(Verdict::RequestChanges),
            None,
            &reviewer(),
        )
        .unwrap()
    };
    finish_now();
    edited.splice(11..12, ["struct Q;".to_string(), "impl Q {}".to_string()]);
    repo.write("x.rs", edited.join("\n") + "\n");
    finish_now();
    repo.write("y.rs", "y changed again\n");

    // Opened on the interdiff since round 2, which lists y.rs alone: the thread is placed
    // against the review's own read of x.rs, and only then does it need round 1's history.
    let session = session(&repo, Target::Working, cx);
    wait(cx, &session, "its trail", |session| {
        let placement = session.placement(thread(session, &id));
        placement.is_not_in_comparison() && placement.trail().is_some()
    });
    session.read_with(cx, |session, _| {
        assert_eq!(session.comparison(), Comparison::SinceRound(2));
        let thread = thread(session, &id);
        assert_eq!(session.location(thread), "x.rs:12–13");
    });
}
