//! `dv review done` never hands back a round the reviewer finished while the agent worked
//! (PLAN §2.4, §10.1): exit 1 `unseen_round` until the agent says it read it with `--round`.

use dv_core::review::{ReviewStatus, Verdict};
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn done_refuses_a_round_finished_while_the_agent_worked() {
    let mut fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let id = review.id.as_str().to_string();
    let thread = fixture.thread(&review, "src/app.ts", 10, 10, "Why a cache?");
    fixture.finish(&review.id, Verdict::RequestChanges);
    fixture.env.insert("CLAUDECODE".into(), "1".into());
    // The agent starts on round 1, and the reviewer finishes round 2 meanwhile.
    fixture
        .run(&[
            "comment",
            "reply",
            thread.as_str(),
            "--body",
            "On it.",
            "--json",
        ])
        .ok_json();
    assert_eq!(fixture.load(&review.id).status, ReviewStatus::InProgress);
    fixture.finish(&review.id, Verdict::RequestChanges);

    // Its `done`, about round 1, is refused, saying what to do.
    let before = fixture.events(&review.id).len();
    let ran = fixture.run(&["review", "done", "--note", "Fixed round 1.", "--json"]);
    let error = ran.error(Exit::Failure, "unseen_round");
    let message = error["message"].as_str().unwrap();
    for part in [
        "the reviewer finished round 2 while you were working",
        "the `body` of the last of `rounds`",
        &format!("`dv review show {id} --json`"),
        "`dv comment list --status open --json`",
        &format!("`dv review done {id} --round 2`"),
    ] {
        assert!(message.contains(part), "{part:?} in {message}");
    }
    assert_eq!(fixture.events(&review.id).len(), before, "nothing written");
    assert_eq!(
        fixture.load(&review.id).status,
        ReviewStatus::ChangesRequested
    );
    // The agent's wait still hears of round 2.
    let wait = fixture
        .run(&["review", "wait", &id, "--timeout", "1", "--json"])
        .ok_json();
    assert_eq!(wait["outcome"], "changes_requested");
    assert_eq!(wait["round"]["n"], 2);

    // An older round is no acknowledgement, a round that hasn't closed a mistake.
    fixture
        .run(&["review", "done", "--round", "1", "--json"])
        .error(Exit::Failure, "unseen_round");
    let error = fixture
        .run(&["review", "done", "--round", "3", "--json"])
        .error(Exit::Usage, "usage");
    assert_eq!(
        error["message"],
        "round 3 isn't the last closed round (round 2 is)"
    );
    // Read and acknowledged, it hands back.
    let json = fixture
        .run(&[
            "review",
            "done",
            &id,
            "--round",
            "2",
            "--note",
            "Read round 2; renamed.",
            "--json",
        ])
        .ok_json();
    assert_eq!(json["changed"], true);
    assert_eq!(json["review"]["status"], "ready");
}

#[test]
fn done_names_the_round_it_hands_back() {
    let mut fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let thread = fixture.thread(&review, "src/app.ts", 10, 10, "Why a cache?");
    fixture.finish(&review.id, Verdict::RequestChanges);
    fixture.env.insert("DV_AUTHOR".into(), "codex".into());
    // Round 1 closed from `open`: done at once, nothing written first, with or without its
    // number.
    fixture.run(&["review", "done", "--json"]).ok_json();
    // Round 2 closed from `ready`: the same.
    fixture.finish(&review.id, Verdict::RequestChanges);
    fixture
        .run(&["review", "done", "--round", "2", "--json"])
        .ok_json();
    // Round 4 closes while the agent works on round 3 (its reply moved the review to
    // in_progress). Its replies since may still be about round 3, so only the round's number
    // hands it back.
    fixture.finish(&review.id, Verdict::RequestChanges);
    let reply = [
        "comment",
        "reply",
        thread.as_str(),
        "--body",
        "Again.",
        "--json",
    ];
    fixture.run(&reply).ok_json();
    fixture.finish(&review.id, Verdict::RequestChanges);
    fixture.run(&reply).ok_json();
    fixture
        .run(&["review", "done", "--json"])
        .error(Exit::Failure, "unseen_round");
    fixture
        .run(&["review", "done", "--round", "3", "--json"])
        .error(Exit::Failure, "unseen_round");
    let json = fixture
        .run(&["review", "done", "--round", "4", "--json"])
        .ok_json();
    assert_eq!(json["review"]["status"], "ready");
}

#[test]
fn done_after_the_approval_is_finished_when_it_names_the_review() {
    let mut fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    fixture.finish(&review.id, Verdict::Approve);
    fixture.env.insert("DV_AUTHOR".into(), "codex".into());
    // An approved review is no longer open, so no longer the work checked out here.
    fixture
        .run(&["review", "done", "--json"])
        .error(Exit::Usage, "no_review");
    // Named, it is found, and finished.
    fixture
        .run(&["review", "done", review.id.as_str(), "--json"])
        .error(Exit::Failure, "finished");
}
