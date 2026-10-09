//! The agent's hand-back (PLAN §2.4, `ops::hand_back`): never of a round the reviewer finished
//! while the agent was working, acknowledged with `--round`, and conditional on the closed
//! rounds it was checked against.

use crate::review::fold::Folder;
use crate::review::ops::{
    OpError, add_thread, finish_round, hand_back, hand_back_on, open_review, reply, set_status,
    start_work,
};
use crate::testing::git_repo;

use super::*;

/// A working review with a thread on it, and the git to finish its rounds with.
fn review_with_thread() -> (FixtureRepo, GitRepoAndStore, ReviewId, ThreadId) {
    let (repo, store) = fixture_store();
    repo.write("src/lib.rs", "one\nTWO\nthree\n");
    let id = open_review(&store, Target::Working, reviewer())
        .unwrap()
        .review
        .id;
    let anchor = Anchor {
        round: 1,
        path: "src/lib.rs".to_string(),
        old_path: None,
        side: AnchorSide::New,
        start_line: 2,
        end_line: 2,
        text: vec!["TWO".to_string()],
        context_before: vec!["one".to_string()],
        context_after: vec!["three".to_string()],
    };
    let thread = add_thread(&store, &id, anchor, "Why upper case?", reviewer()).unwrap();
    let git = git_repo(&repo);
    (repo, GitRepoAndStore { git, store }, id, thread)
}

struct GitRepoAndStore {
    git: crate::git::GitRepo,
    store: ReviewStore,
}

impl GitRepoAndStore {
    fn request_changes(&self, id: &ReviewId, summary: &str) -> u32 {
        finish_round(
            &self.store,
            &self.git,
            id,
            Some(Verdict::RequestChanges),
            Some(summary.to_string()),
            &reviewer(),
        )
        .unwrap()
        .round
        .n
    }

    fn load(&self, id: &ReviewId) -> Review {
        let loaded = self.store.load(id).unwrap();
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        loaded.review
    }

    /// The `if_round` of the last `status` event in the log.
    fn last_if_round(&self, id: &ReviewId) -> Option<u32> {
        self.store
            .events(id)
            .unwrap()
            .events
            .iter()
            .rev()
            .find_map(|event| match &event.body {
                EventBody::Status { if_round, .. } => Some(*if_round),
                _ => None,
            })
            .flatten()
    }
}

fn unseen(result: Result<bool, OpError>) -> u32 {
    match result {
        Err(OpError::UnseenRound { round }) => round,
        other => panic!("expected UnseenRound, got {other:?}"),
    }
}

#[test]
fn a_round_finished_while_the_agent_worked_is_not_handed_back_unseen() {
    let (_repo, r, id, thread) = review_with_thread();
    assert_eq!(r.request_changes(&id, "Round 1."), 1);
    // The agent starts on round 1: its reply moves the review to in_progress.
    reply(&r.store, &id, &thread, "On it.", agent(), None).unwrap();
    assert!(start_work(&r.store, &id, &agent()).unwrap());
    // The reviewer finishes round 2 meanwhile, asking for more.
    assert_eq!(r.request_changes(&id, "Round 2: also rename it."), 2);
    let review = r.load(&id);
    assert_eq!(
        review.rounds[1].closed_from,
        Some(ReviewStatus::InProgress),
        "derived by the fold"
    );
    assert_eq!(review.unseen_round(), Some(2));

    // The agent's `done`, about round 1, is refused, and nothing is written.
    let before = r.store.events(&id).unwrap().events.len();
    assert_eq!(
        unseen(hand_back(
            &r.store,
            &id,
            &agent(),
            Some("Fixed round 1."),
            None
        )),
        2
    );
    // So is `set_status`'s way to `ready`, which is the same hand-back.
    assert_eq!(
        unseen(set_status(
            &r.store,
            &id,
            ReviewStatus::Ready,
            &agent(),
            Some("Fixed round 1.")
        )),
        2
    );
    // Acknowledging an older round is no acknowledgement; one that hasn't closed is a mistake.
    assert_eq!(unseen(hand_back(&r.store, &id, &agent(), None, Some(1))), 2);
    assert!(matches!(
        hand_back(&r.store, &id, &agent(), None, Some(3)),
        Err(OpError::NotLastRound {
            round: 3,
            closed: 2
        })
    ));
    assert_eq!(r.store.events(&id).unwrap().events.len(), before);
    assert_eq!(r.load(&id).status, ReviewStatus::ChangesRequested);

    // Read and acknowledged, it hands back.
    assert!(
        hand_back(
            &r.store,
            &id,
            &agent(),
            Some("Read round 2; renamed."),
            Some(2)
        )
        .unwrap()
    );
    let review = r.load(&id);
    assert_eq!(review.status, ReviewStatus::Ready);
    assert_eq!(
        review.status_change.unwrap().note.as_deref(),
        Some("Read round 2; renamed.")
    );
    assert_eq!(r.last_if_round(&id), Some(2));
}

#[test]
fn an_agents_writes_after_the_round_dont_say_it_saw_the_round() {
    let (_repo, r, id, thread) = review_with_thread();
    r.request_changes(&id, "Round 1.");
    reply(&r.store, &id, &thread, "On it.", agent(), None).unwrap();
    start_work(&r.store, &id, &agent()).unwrap();
    r.request_changes(&id, "Round 2.");
    assert_eq!(r.load(&id).unseen_round(), Some(2));
    // The agent goes on down the list it read in round 1: its reply moves the review to
    // in_progress again, but may still be about round 1.
    reply(&r.store, &id, &thread, "Renamed too.", agent(), None).unwrap();
    assert!(start_work(&r.store, &id, &agent()).unwrap());
    assert_eq!(r.load(&id).unseen_round(), Some(2));
    assert_eq!(
        unseen(hand_back(&r.store, &id, &agent(), Some("Done."), None)),
        2
    );
    // Named, it hands back; done again (another note) needs no name.
    assert!(hand_back(&r.store, &id, &agent(), Some("Done."), Some(2)).unwrap());
    assert_eq!(r.load(&id).unseen_round(), None);
    assert!(hand_back(&r.store, &id, &agent(), Some("Done, really."), None).unwrap());
    // The next round closes from `ready`: the agent was waiting for it.
    r.request_changes(&id, "Round 3.");
    assert_eq!(r.load(&id).unseen_round(), None);
}

#[test]
fn a_hand_back_racing_an_approval_hears_that_the_review_is_finished() {
    let (_repo, r, id, thread) = review_with_thread();
    r.request_changes(&id, "Round 1.");
    reply(&r.store, &id, &thread, "On it.", agent(), None).unwrap();
    start_work(&r.store, &id, &agent()).unwrap();
    let stale = r.load(&id);
    // The reviewer approves before the agent's append lands.
    finish_round(
        &r.store,
        &r.git,
        &id,
        Some(Verdict::Approve),
        None,
        &reviewer(),
    )
    .unwrap();
    assert!(matches!(
        hand_back_on(&r.store, &id, stale, &agent(), Some("Fixed."), Some(1)),
        Err(OpError::Finished {
            status: ReviewStatus::Approved
        })
    ));
    assert_eq!(r.load(&id).status, ReviewStatus::Approved);
}

#[test]
fn the_usual_hand_backs_need_no_acknowledgement() {
    // Round 1 closes from `open`: the agent hands back without a write (a summary-only round
    // fixed in code alone).
    let (_repo, r, id, thread) = review_with_thread();
    r.request_changes(&id, "Round 1.");
    assert_eq!(r.load(&id).rounds[0].closed_from, Some(ReviewStatus::Open));
    assert!(hand_back(&r.store, &id, &agent(), Some("Fixed."), None).unwrap());
    assert_eq!(r.last_if_round(&id), Some(1));
    // Round 2 closes from `ready`, after a handback: again no write, still no acknowledgement.
    r.request_changes(&id, "Round 2: one more thing in the summary.");
    assert_eq!(r.load(&id).rounds[1].closed_from, Some(ReviewStatus::Ready));
    assert!(hand_back(&r.store, &id, &agent(), Some("Fixed that too."), None).unwrap());
    // Replies in the new round, then done.
    r.request_changes(&id, "Round 3.");
    reply(&r.store, &id, &thread, "Done.", agent(), None).unwrap();
    start_work(&r.store, &id, &agent()).unwrap();
    assert!(hand_back(&r.store, &id, &agent(), None, None).unwrap());
    assert_eq!(r.load(&id).status, ReviewStatus::Ready);
    // `--round` naming the last closed round is always fine; done again with the same note
    // writes nothing.
    assert!(!hand_back(&r.store, &id, &agent(), None, Some(3)).unwrap());
    // Only agents hand back.
    assert!(matches!(
        hand_back(&r.store, &id, &reviewer(), None, None),
        Err(OpError::NotAllowed { .. })
    ));
}

#[test]
fn a_round_that_closes_between_the_check_and_the_append_wins() {
    let (_repo, r, id, thread) = review_with_thread();
    r.request_changes(&id, "Round 1.");
    reply(&r.store, &id, &thread, "On it.", agent(), None).unwrap();
    start_work(&r.store, &id, &agent()).unwrap();
    // The agent's `done` reads the review (in_progress, one round closed)…
    let stale = r.load(&id);
    assert_eq!(stale.status, ReviewStatus::InProgress);
    // …the reviewer finishes round 2 before its append lands…
    r.request_changes(&id, "Round 2.");
    // …and the append, conditional on one closed round, changes nothing where it lands.
    assert_eq!(
        unseen(hand_back_on(
            &r.store,
            &id,
            stale,
            &agent(),
            Some("Round 1 fixed."),
            None
        )),
        2
    );
    let loaded = r.store.load(&id).unwrap();
    assert!(loaded.warnings.is_empty(), "a no-op, not a rejection");
    assert_eq!(loaded.review.status, ReviewStatus::ChangesRequested);
    assert_eq!(r.last_if_round(&id), Some(1), "written, conditional");
    assert_eq!(loaded.review.unseen_round(), Some(2));
}

#[test]
fn a_status_conditional_on_the_closed_rounds_applies_only_with_that_many() {
    let mut folder = Folder::create(
        &ReviewId::generate(),
        &Event::new(EventBody::Created {
            target: Target::Working,
            author: reviewer(),
            worktree: "/w".to_string(),
        }),
    )
    .unwrap();
    let ready = |if_round| {
        Event::new(EventBody::Status {
            status: ReviewStatus::Ready,
            note: None,
            author: Some(agent()),
            if_status: None,
            if_round,
        })
    };
    assert_eq!(folder.apply(&ready(Some(1))).unwrap(), Outcome::Unchanged);
    let round = Event::new(EventBody::RoundClosed {
        n: 1,
        new_oid: oid('b'),
        old_oid: oid('a'),
        verdict: Some(Verdict::RequestChanges),
        body: None,
    });
    folder.apply(&round).unwrap();
    assert_eq!(folder.apply(&ready(Some(0))).unwrap(), Outcome::Unchanged);
    assert_eq!(folder.apply(&ready(Some(1))).unwrap(), Outcome::Changed);
    let review = folder.into_review();
    assert_eq!(review.status, ReviewStatus::Ready);
}
