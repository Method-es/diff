//! `dv review wait` (PLAN §10.1): state-based, so it returns at once when the agent has
//! something to do, blocks otherwise, and never misses the reviewer's verdict, however close to
//! the call it lands.

use std::time::{Duration, Instant};

use dv_core::review::ReviewStatus;
use dv_core::target::Target;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::GlobalArgs;
use crate::commands::wait::{Stage, Waited, wait_for};
use crate::repo::Repo;

/// A working review with one open thread, handed back by the agent: the reviewer's turn.
fn ready(fixture: &Fixture) -> Review {
    let review = fixture.review(Target::Working);
    fixture.thread(&review, "src/app.ts", 10, 10, "Why a cache?");
    fixture.finish(&review.id, Verdict::RequestChanges);
    ops::set_status(
        &fixture.store(),
        &review.id,
        ReviewStatus::Ready,
        &agent(),
        Some("Done."),
    )
    .unwrap();
    fixture.load(&review.id)
}

#[test]
fn wait_returns_at_once_when_the_agent_has_something_to_do() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    fixture.thread(&review, "src/app.ts", 10, 10, "Why a cache?");
    ops::finish_round(
        &fixture.store(),
        &fixture.git(),
        &review.id,
        Some(Verdict::RequestChanges),
        Some("One thing.".into()),
        &reviewer(),
    )
    .unwrap();

    let started = Instant::now();
    let json = fixture
        .run(&["review", "wait", "--timeout", "30", "--json"])
        .ok_json();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(json["outcome"], "changes_requested");
    assert_eq!(json["review"]["id"], review.id.as_str());
    assert_eq!(json["review"]["status"], "changes_requested");
    let closed_at = json["round"]["closed_at"].clone();
    assert_eq!(
        json["round"],
        json!({"n": 1, "verdict": "request_changes", "body": "One thing.", "closed_at": closed_at})
    );
    assert_eq!(
        json["threads"],
        json!({"open": 1, "addressed": 0, "resolved": 0})
    );
    let ran = fixture.run(&["review", "wait"]);
    assert_eq!(
        ran.lines(),
        [
            format!(
                "{}  working tree vs main  changes_requested  round 2  1 open, 0 addressed, 0 \
                 resolved",
                review.id
            ),
            "round 1 closed: request_changes".to_string(),
            "    One thing.".to_string(),
            format!(
                "changes requested, 1 open thread: do what the round's summary asks, run `dv \
                 comment list --status open`, fix, reply and address each, then `dv review done \
                 {} --round 1`",
                review.id
            ),
        ]
    );

    // Finished reviews, named, too.
    fixture.finish(&review.id, Verdict::Approve);
    let json = fixture
        .run(&["review", "wait", review.id.as_str(), "--json"])
        .ok_json();
    assert_eq!(json["outcome"], "approved");
    assert_eq!(json["round"]["n"], 2);
    later();
    let other = fixture.review(Target::Staged);
    ops::set_status(
        &fixture.store(),
        &other.id,
        ReviewStatus::Closed,
        &reviewer(),
        None,
    )
    .unwrap();
    let ran = fixture.run(&["review", "wait", "--review", other.id.as_str()]);
    assert_eq!(ran.exit, Exit::Ok);
    assert_eq!(
        ran.lines()[1..],
        ["closed: the reviewer closed the review"],
        "no round line before the first round"
    );
}

#[test]
fn wait_blocks_until_the_reviewer_finishes_the_round() {
    let fixture = Fixture::new();
    let review = ready(&fixture);
    let opener = fixture.opener();
    let id = review.id.clone();
    let reviewer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        opener.finish(&id, Verdict::Approve);
        Instant::now()
    });
    let started = Instant::now();
    let ran = fixture.run(&["review", "wait", "--timeout", "30", "--json"]);
    let returned = Instant::now();
    let appended = reviewer.join().unwrap();
    let json = ran.ok_json();
    assert_eq!(json["outcome"], "approved");
    assert_eq!(json["round"]["verdict"], "approve");
    assert!(
        returned >= appended,
        "it returned before the verdict landed"
    );
    assert!(
        returned - started >= Duration::from_millis(400),
        "it blocked"
    );
    assert!(
        returned - appended < Duration::from_millis(1500),
        "it woke {:?} after the verdict",
        returned - appended
    );
}

#[test]
fn no_wakeup_is_lost_around_the_watch() {
    // The verdict lands right before the watch is registered, then right after: either way the
    // load that follows the registration sees it.
    for at in [Stage::BeforeWatch, Stage::AfterWatch] {
        let fixture = Fixture::new();
        let review = ready(&fixture);
        let (ctx, _, _) = fixture.context(fixture.path(), &Input::default());
        let repo = Repo::open(&GlobalArgs::default(), &ctx).unwrap();
        let mut seen = Vec::new();
        let started = Instant::now();
        let waited = wait_for(
            &repo,
            review.clone(),
            Some(Duration::from_secs(20)),
            &mut |stage| {
                seen.push(stage);
                if stage == at {
                    fixture.finish(&review.id, Verdict::RequestChanges);
                }
            },
        )
        .unwrap();
        assert_eq!(seen, [Stage::BeforeWatch, Stage::AfterWatch]);
        match waited {
            Waited::Woke(review) => assert_eq!(review.status, ReviewStatus::ChangesRequested),
            Waited::TimedOut(_) => panic!("{at:?}: the verdict was missed"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "{at:?}: it waited for the re-check"
        );
    }

    // And from another thread as the call starts, with no coordination at all.
    for _ in 0..3 {
        let fixture = Fixture::new();
        let review = ready(&fixture);
        let opener = fixture.opener();
        let id = review.id.clone();
        let reviewer = std::thread::spawn(move || opener.finish(&id, Verdict::RequestChanges));
        let started = Instant::now();
        let json = fixture
            .run(&["review", "wait", "--timeout", "20", "--json"])
            .ok_json();
        reviewer.join().unwrap();
        assert_eq!(json["outcome"], "changes_requested");
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}

#[test]
fn wait_times_out_with_exit_3() {
    let fixture = Fixture::new();
    let review = ready(&fixture);
    let started = Instant::now();
    let ran = fixture.run(&["review", "wait", "--timeout", "0.4", "--json"]);
    let took = started.elapsed();
    assert_eq!(ran.exit, Exit::Timeout, "{}", ran.stderr);
    assert_eq!(ran.stdout, "{\"schema\":1,\"outcome\":\"timeout\"}\n");
    assert_eq!(ran.stderr, "");
    assert!(took >= Duration::from_millis(400), "{took:?}");
    assert!(took < Duration::from_secs(4), "{took:?}");

    let ran = fixture.run(&["review", "wait", "--timeout", "0.1"]);
    assert_eq!(ran.exit, Exit::Timeout);
    assert_eq!(
        ran.lines(),
        [format!(
            "timed out: review {id} is still ready; run `dv review wait {id}` again to keep \
             waiting",
            id = review.id
        )]
    );
}

#[test]
fn a_review_deleted_while_waiting_is_exit_1() {
    let fixture = Fixture::new();
    let review = ready(&fixture);
    let (ctx, _, _) = fixture.context(fixture.path(), &Input::default());
    let repo = Repo::open(&GlobalArgs::default(), &ctx).unwrap();
    // The review is deleted from another process's point of view once the wait is watching
    // (its watch, not the re-check, brings the news): a moment later, while it blocks.
    let mut deleter = None;
    let started = Instant::now();
    let waited = wait_for(
        &repo,
        review.clone(),
        Some(Duration::from_secs(30)),
        &mut |stage| {
            if stage == Stage::AfterWatch {
                let (opener, id) = (fixture.opener(), review.id.clone());
                deleter = Some(std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(300));
                    assert!(opener.store().delete(&id).unwrap());
                }));
            }
        },
    );
    deleter.expect("the wait got to its watch").join().unwrap();
    let error = waited.expect_err("the review is gone");
    assert_eq!((error.exit, error.code), (Exit::Failure, "not_found"));
    assert_eq!(
        error.message,
        format!("review {} was deleted while waiting for it", review.id)
    );
    assert!(started.elapsed() < Duration::from_secs(4));

    // Through the command: exit 1 and the error document, whenever the delete lands.
    let ran = fixture.run(&["review", "wait", review.id.as_str(), "--json"]);
    ran.error(Exit::Failure, "not_found");
}

#[test]
fn only_a_status_the_agent_acts_on_wakes_it() {
    use crate::commands::wait::wakes;
    for status in [
        ReviewStatus::ChangesRequested,
        ReviewStatus::Approved,
        ReviewStatus::Closed,
        ReviewStatus::Submitted,
    ] {
        assert!(wakes(status), "{status}");
    }
    for status in [
        ReviewStatus::Open,
        ReviewStatus::InProgress,
        ReviewStatus::Ready,
        ReviewStatus::Draft,
    ] {
        assert!(!wakes(status), "{status}");
    }

    // An open review, and one the agent is working on (its first reply after changes were
    // requested), keep the wait blocked until the timeout.
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let thread = fixture.thread(&review, "src/app.ts", 10, 10, "Why a cache?");
    let timed_out = |fixture: &Fixture| {
        let ran = fixture.run(&["review", "wait", "--timeout", "0.3", "--json"]);
        assert_eq!(ran.exit, Exit::Timeout, "{}", ran.stderr);
    };
    timed_out(&fixture);
    fixture.finish(&review.id, Verdict::RequestChanges);
    let ran = fixture.run(&[
        "comment",
        "reply",
        thread.as_str(),
        "--body",
        "On it.",
        "--author",
        "codex",
    ]);
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    assert_eq!(fixture.load(&review.id).status, ReviewStatus::InProgress);
    timed_out(&fixture);
}

#[test]
fn the_re_check_comes_however_busy_the_store_is() {
    use crate::commands::wait::{Until, until_reload};
    use dv_core::backend::WatchEvent;

    // Another review's log changes every 50 ms for two seconds; the re-check is due in 300.
    let (sender, events) = std::sync::mpsc::channel::<WatchEvent>();
    let busy = std::thread::spawn(move || {
        for _ in 0..40 {
            let event = WatchEvent {
                worktree_paths: Vec::new(),
                store_paths: vec!["reviews/r-other.jsonl".to_string()],
                git_changed: false,
                rescan: false,
            };
            if sender.send(event).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    let started = Instant::now();
    let until = until_reload(
        Some(&events),
        "reviews/r-mine.jsonl",
        None,
        started + Duration::from_millis(300),
    );
    let took = started.elapsed();
    drop(events);
    busy.join().unwrap();
    assert_eq!(until, Until::Reload);
    assert!(
        took >= Duration::from_millis(300) && took < Duration::from_millis(1200),
        "{took:?}"
    );

    // Its own log wakes it at once; a deadline before the re-check ends it.
    let (sender, events) = std::sync::mpsc::channel::<WatchEvent>();
    sender
        .send(WatchEvent {
            worktree_paths: Vec::new(),
            store_paths: vec!["reviews/r-mine.jsonl".to_string()],
            git_changed: false,
            rescan: false,
        })
        .unwrap();
    let far = Instant::now() + Duration::from_secs(30);
    assert_eq!(
        until_reload(Some(&events), "reviews/r-mine.jsonl", None, far),
        Until::Reload
    );
    let soon = Instant::now() + Duration::from_millis(100);
    assert_eq!(
        until_reload(Some(&events), "reviews/r-mine.jsonl", Some(soon), far),
        Until::Timeout
    );
    drop(sender);
    assert_eq!(
        until_reload(Some(&events), "reviews/r-mine.jsonl", None, far),
        Until::Unwatched
    );
}

#[test]
fn a_round_with_no_open_thread_still_says_what_to_do() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    ops::finish_round(
        &fixture.store(),
        &fixture.git(),
        &review.id,
        Some(Verdict::RequestChanges),
        Some("Add a test for square.".into()),
        &reviewer(),
    )
    .unwrap();
    let ran = fixture.run(&["review", "wait"]);
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    assert_eq!(
        ran.lines()[1..],
        [
            "round 1 closed: request_changes".to_string(),
            "    Add a test for square.".to_string(),
            format!(
                "changes requested, 0 open threads: do what the round's summary asks, then `dv \
                 review done {} --round 1`",
                review.id
            ),
        ]
    );
    let json = fixture.run(&["review", "wait", "--json"]).ok_json();
    assert_eq!(json["round"]["body"], "Add a test for square.");
    assert_eq!(json["threads"]["open"], 0);

    // Without a summary, the reviewer's replies are where the request is.
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    fixture.finish(&review.id, Verdict::RequestChanges);
    let ran = fixture.run(&["review", "wait"]);
    assert_eq!(
        ran.lines()[1..],
        [
            "round 1 closed: request_changes".to_string(),
            format!(
                "changes requested, 0 open threads: read the reviewer's replies (`dv comment \
                 list`), act on them, then `dv review done {} --round 1`",
                review.id
            ),
        ]
    );
}

#[test]
fn an_agents_own_question_is_not_counted_among_the_threads_to_address() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    fixture.thread(&review, "src/app.ts", 10, 10, "Why a cache?");
    // The agent asks the reviewer something (the skill's `dv comment add`).
    let asked = fixture
        .run(&[
            "comment",
            "add",
            "--file",
            "src/app.ts",
            "--lines",
            "3",
            "--author",
            "codex",
            "--body",
            "Should this be exported?",
            "--json",
        ])
        .ok_json();
    let question = ThreadId::parse(asked["thread"]["id"].as_str().unwrap()).unwrap();
    fixture.finish(&review.id, Verdict::RequestChanges);
    let hint = |fixture: &Fixture| {
        let ran = fixture.run(&["review", "wait", "--author", "codex"]);
        assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
        ran.lines().last().unwrap().to_string()
    };
    assert_eq!(
        hint(&fixture),
        format!(
            "changes requested, 1 open thread: run `dv comment list --status open`, fix, reply \
             and address each, then `dv review done {} --round 1`; 1 more open thread is an \
             agent's question still waiting for the reviewer: leave it open",
            review.id
        )
    );
    // The JSON counts every open thread.
    let json = fixture
        .run(&["review", "wait", "--author", "codex", "--json"])
        .ok_json();
    assert_eq!(json["threads"]["open"], 2);

    // Once the reviewer answers it, it is the agent's to act on like any other.
    ops::reply(
        &fixture.store(),
        &review.id,
        &question,
        "Yes, export it.",
        reviewer(),
        None,
    )
    .unwrap();
    assert_eq!(
        hint(&fixture),
        format!(
            "changes requested, 2 open threads: run `dv comment list --status open`, fix, reply \
             and address each, then `dv review done {} --round 1`",
            review.id
        )
    );
}
