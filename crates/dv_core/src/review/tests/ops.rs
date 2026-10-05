//! The checked operations (PLAN §2.4, §2.5): one open review per target, thread and status
//! rules by author, read-only submitted reviews.

use super::*;
use crate::review::ops::{
    add_thread, close_round, delete_item, edit_body, is_review_of, link_pr, mark_viewed,
    open_review, record_submission, reply, set_status, set_thread_state,
};
use crate::testing::open_at;

fn pr_target(head: char) -> Target {
    Target::Pr {
        owner: "kylekz".to_string(),
        repo: "difftest".to_string(),
        number: 12,
        base_oid: oid('a'),
        head_oid: oid(head),
    }
}

/// A store with one open `working` review.
fn with_review() -> (FixtureRepo, ReviewStore, ReviewId) {
    let (repo, store) = fixture_store();
    let opened = open_review(&store, Target::Working, reviewer()).unwrap();
    assert!(opened.created);
    (repo, store, opened.review.id)
}

fn load(store: &ReviewStore, id: &ReviewId) -> Review {
    let loaded = store.load(id).unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    loaded.review
}

fn rejected(result: Result<impl std::fmt::Debug, OpError>) -> Rejection {
    match result {
        Err(OpError::Rejected(reason)) => reason,
        other => panic!("expected a rejection, got {other:?}"),
    }
}

fn not_allowed(result: Result<impl std::fmt::Debug, OpError>) -> String {
    match result {
        Err(err @ OpError::NotAllowed { .. }) => err.to_string(),
        other => panic!("expected NotAllowed, got {other:?}"),
    }
}

#[test]
fn opening_a_target_resumes_its_open_review() {
    let (repo, store) = fixture_store();
    let first = open_review(&store, Target::Working, reviewer()).unwrap();
    assert!(first.created);
    let again = open_review(&store, Target::Working, agent()).unwrap();
    assert!(!again.created);
    assert_eq!(again.review.id, first.review.id);
    assert_eq!(again.review.author, reviewer());

    // Another target is another review.
    let staged = open_review(&store, Target::Staged, reviewer()).unwrap();
    assert!(staged.created);
    assert_ne!(staged.review.id, first.review.id);

    // A finished review doesn't count: the next open starts a new one.
    set_status(
        &store,
        &first.review.id,
        ReviewStatus::Approved,
        AuthorKind::Reviewer,
    )
    .unwrap();
    let next = open_review(&store, Target::Working, reviewer()).unwrap();
    assert!(next.created);
    assert_ne!(next.review.id, first.review.id);
    assert!(
        !open_review(&store, Target::Working, reviewer())
            .unwrap()
            .created
    );

    // An unreadable log among the reviews doesn't stop opening.
    store
        .backend()
        .store_append(&format!("reviews/{}.jsonl", ReviewId::generate()), b"junk")
        .unwrap();
    assert_eq!(
        open_review(&store, Target::Working, reviewer())
            .unwrap()
            .review
            .id,
        next.review.id
    );
    drop(repo);
}

#[test]
fn working_reviews_are_per_worktree_and_commit_reviews_are_shared() {
    let (repo, main) = fixture_store();
    let worktree = repo.add_worktree("wt", "feat");
    let linked = ReviewStore::new(Arc::new(open_at(&repo, &worktree)));

    let main_working = open_review(&main, Target::Working, reviewer()).unwrap();
    let linked_working = open_review(&linked, Target::Working, reviewer()).unwrap();
    assert!(linked_working.created, "another worktree's changes");
    assert_ne!(linked_working.review.id, main_working.review.id);
    assert_eq!(
        open_review(&main, Target::Working, reviewer())
            .unwrap()
            .review
            .id,
        main_working.review.id
    );

    let commit = Target::Commit {
        rev: "HEAD".to_string(),
    };
    let from_main = open_review(&main, commit.clone(), reviewer()).unwrap();
    let from_linked = open_review(&linked, commit.clone(), reviewer()).unwrap();
    assert!(!from_linked.created, "the same commits from any worktree");
    assert_eq!(from_linked.review.id, from_main.review.id);

    // The rule open_review resumes by, as the switcher asks it of a listed summary.
    let summary = main_working.review.summary();
    assert!(is_review_of(&summary, &Target::Working, main.worktree()));
    assert!(!is_review_of(&summary, &Target::Working, linked.worktree()));
    assert!(!is_review_of(&summary, &Target::Staged, main.worktree()));
    let summary = from_main.review.summary();
    assert!(is_review_of(&summary, &commit, linked.worktree()));
}

#[test]
fn a_pr_review_resumes_by_pr_whatever_head_it_was_fetched_at() {
    let (_repo, store) = fixture_store();
    let first = open_review(&store, pr_target('b'), reviewer()).unwrap();
    assert_eq!(first.review.status, ReviewStatus::Draft);
    let pushed = open_review(&store, pr_target('c'), reviewer()).unwrap();
    assert!(!pushed.created);
    assert_eq!(pushed.review.id, first.review.id);

    // After submission the next round is a new draft for the same PR (PLAN §2.4).
    record_submission(
        &store,
        &first.review.id,
        5,
        "https://x.invalid/5",
        Verdict::Comment,
    )
    .unwrap();
    let round_two = open_review(&store, pr_target('c'), reviewer()).unwrap();
    assert!(round_two.created);
    assert_eq!(round_two.review.status, ReviewStatus::Draft);
}

#[test]
fn threads_and_replies() {
    let (_repo, store, id) = with_review();
    let thread = add_thread(
        &store,
        &id,
        anchor("src/lib.rs", 2),
        "Rename this?",
        reviewer(),
    )
    .unwrap();
    let answer = reply(&store, &id, &thread, "Renamed.", agent()).unwrap();
    let review = load(&store, &id);
    let t = review.thread(&thread).unwrap();
    assert_eq!(t.body, "Rename this?");
    assert_eq!(t.replies[0].id, answer);
    assert_eq!(t.replies[0].author, agent());

    // Bodies can't be blank, anchors must be well formed, threads must exist.
    assert!(matches!(
        add_thread(&store, &id, anchor("src/lib.rs", 2), "  \n", reviewer()),
        Err(OpError::Empty { .. })
    ));
    assert!(matches!(
        reply(&store, &id, &thread, "", agent()),
        Err(OpError::Empty { .. })
    ));
    let mut bad = anchor("src/lib.rs", 2);
    bad.end_line = 3;
    assert!(matches!(
        rejected(add_thread(&store, &id, bad, "x", reviewer())),
        Rejection::InvalidAnchor { .. }
    ));
    let mut later = anchor("src/lib.rs", 2);
    later.round = 2;
    assert_eq!(
        rejected(add_thread(&store, &id, later, "x", reviewer())),
        Rejection::UnknownRound {
            round: 2,
            current: 1
        }
    );
    let ghost = ThreadId::generate();
    assert_eq!(
        rejected(reply(&store, &id, &ghost, "x", agent())),
        Rejection::UnknownThread { thread: ghost }
    );
    assert!(matches!(
        add_thread(
            &store,
            &ReviewId::generate(),
            anchor("a", 1),
            "x",
            reviewer()
        ),
        Err(OpError::Store(StoreError::UnknownReview { .. }))
    ));
    // Nothing refused was written.
    assert_eq!(store.events(&id).unwrap().events.len(), 3);
}

#[test]
fn agents_address_and_only_the_reviewer_resolves_or_reopens() {
    let (_repo, store, id) = with_review();
    let thread = add_thread(&store, &id, anchor("src/lib.rs", 1), "Fix", reviewer()).unwrap();
    let set = |state, note, author| set_thread_state(&store, &id, &thread, state, note, author);

    assert!(matches!(
        rejected(set(ThreadState::Resolved, None, agent())),
        Rejection::ThreadTransition { .. }
    ));
    assert!(set(ThreadState::Addressed, Some(" did it "), agent()).unwrap());
    let t = load(&store, &id).threads[0].clone();
    assert_eq!(t.state, ThreadState::Addressed);
    let change = t.last_change.unwrap();
    assert_eq!(change.note.as_deref(), Some("did it"));
    assert_eq!(change.author, agent());
    // Addressing again replaces the note.
    assert!(set(ThreadState::Addressed, Some("did it properly"), agent()).unwrap());
    assert!(matches!(
        rejected(set(ThreadState::Open, None, agent())),
        Rejection::ThreadTransition { .. }
    ));
    assert!(set(ThreadState::Open, Some("not quite"), reviewer()).unwrap());
    assert!(!set(ThreadState::Open, None, reviewer()).unwrap(), "no-op");
    assert!(set(ThreadState::Resolved, None, reviewer()).unwrap());
    assert!(matches!(
        rejected(set(ThreadState::Addressed, None, agent())),
        Rejection::ThreadTransition { .. }
    ));
    assert!(matches!(
        rejected(set(ThreadState::Resolved, None, Author::github("octocat"))),
        Rejection::GithubOnLocalThread { .. }
    ));
    assert_eq!(load(&store, &id).threads[0].state, ThreadState::Resolved);
    // The no-op and the refusals wrote nothing: `created`, the thread and four changes.
    assert_eq!(store.events(&id).unwrap().events.len(), 1 + 1 + 4);
}

#[test]
fn github_threads_are_read_only() {
    let (_repo, store, id) = with_review();
    let mirrored = add_thread(
        &store,
        &id,
        anchor("src/lib.rs", 1),
        "From GitHub",
        Author::github("octocat"),
    )
    .unwrap();
    assert!(matches!(
        rejected(reply(&store, &id, &mirrored, "x", reviewer())),
        Rejection::ReadOnlyThread { .. }
    ));
    assert!(matches!(
        rejected(set_thread_state(
            &store,
            &id,
            &mirrored,
            ThreadState::Resolved,
            None,
            reviewer()
        )),
        Rejection::ReadOnlyThread { .. }
    ));
    let item = ItemId::Thread(mirrored);
    not_allowed(edit_body(&store, &id, &item, "x", &reviewer()));
    not_allowed(delete_item(&store, &id, &item, &reviewer()));
}

#[test]
fn status_changes_by_who_may_make_them() {
    let (_repo, store, id) = with_review();
    let message = not_allowed(set_status(
        &store,
        &id,
        ReviewStatus::Approved,
        AuthorKind::Agent,
    ));
    assert_eq!(message, "an agent can't set a review's status to approved");
    not_allowed(set_status(
        &store,
        &id,
        ReviewStatus::Ready,
        AuthorKind::Reviewer,
    ));
    not_allowed(set_status(
        &store,
        &id,
        ReviewStatus::Closed,
        AuthorKind::Github,
    ));
    for status in [
        ReviewStatus::Open,
        ReviewStatus::Draft,
        ReviewStatus::Submitted,
    ] {
        for actor in [AuthorKind::Reviewer, AuthorKind::Agent] {
            not_allowed(set_status(&store, &id, status, actor));
        }
    }

    // The core loop (PLAN §3 A): request changes, the agent works and is done, approve.
    let steps = [
        (ReviewStatus::ChangesRequested, AuthorKind::Reviewer),
        (ReviewStatus::InProgress, AuthorKind::Agent),
        (ReviewStatus::Ready, AuthorKind::Agent),
        (ReviewStatus::ChangesRequested, AuthorKind::Reviewer),
        (ReviewStatus::Ready, AuthorKind::Agent),
        (ReviewStatus::Approved, AuthorKind::Reviewer),
    ];
    for (status, actor) in steps {
        assert!(set_status(&store, &id, status, actor).unwrap(), "{status}");
        assert_eq!(load(&store, &id).status, status);
    }
    assert!(!set_status(&store, &id, ReviewStatus::Approved, AuthorKind::Reviewer).unwrap());
    assert_eq!(
        rejected(set_status(
            &store,
            &id,
            ReviewStatus::Ready,
            AuthorKind::Agent
        )),
        Rejection::StatusTransition {
            from: ReviewStatus::Approved,
            to: ReviewStatus::Ready
        }
    );
}

#[test]
fn a_draft_pr_review_is_abandoned_with_closed_not_with_local_statuses() {
    let (_repo, store) = fixture_store();
    let id = open_review(&store, pr_target('b'), reviewer())
        .unwrap()
        .review
        .id;
    assert!(matches!(
        rejected(set_status(
            &store,
            &id,
            ReviewStatus::ChangesRequested,
            AuthorKind::Reviewer
        )),
        Rejection::StatusTransition { .. }
    ));
    assert!(set_status(&store, &id, ReviewStatus::Closed, AuthorKind::Reviewer).unwrap());
    // Abandoned, it is still a PR review: that is its target's kind, not its status.
    let abandoned = load(&store, &id);
    assert_eq!(abandoned.status, ReviewStatus::Closed);
    assert!(abandoned.is_pr_review());
}

#[test]
fn operations_fold_without_rewriting_the_snapshot_cache() {
    // An operation's own append makes a snapshot of the state it loaded stale at once, so it
    // folds without writing one (PLAN §7); a fresh snapshot still answers its load.
    let (_repo, store, id) = with_review();
    let rel = format!("reviews/{id}.state.json");
    let cache = || store.backend().store_read(&rel).unwrap();
    assert_eq!(cache(), None);
    add_thread(&store, &id, anchor("src/lib.rs", 1), "Why?", reviewer()).unwrap();
    assert!(mark_viewed(&store, &id, "src/lib.rs", true).unwrap());
    assert!(set_status(&store, &id, ReviewStatus::Ready, AuthorKind::Agent).unwrap());
    assert_eq!(cache(), None, "no operation wrote the snapshot cache");

    // A load (the list, the UI) writes it; operations leave it as it is.
    assert!(!store.load(&id).unwrap().from_cache);
    let written = cache().expect("a load writes the cache");
    let mut doctored: serde_json::Value = serde_json::from_slice(&written).unwrap();
    doctored["review"]["status"] = "approved".into();
    let doctored = doctored.to_string().into_bytes();
    store.backend().store_write_atomic(&rel, &doctored).unwrap();
    // The operation's load came from the (doctored) cache: from `approved` nothing may move.
    assert!(matches!(
        rejected(set_status(
            &store,
            &id,
            ReviewStatus::ChangesRequested,
            AuthorKind::Reviewer
        )),
        Rejection::StatusTransition { .. }
    ));
    assert!(set_status(&store, &id, ReviewStatus::InProgress, AuthorKind::Agent).is_err());
    store.backend().store_remove(&rel).unwrap();
    assert!(set_status(&store, &id, ReviewStatus::InProgress, AuthorKind::Agent).unwrap());
    assert_eq!(cache(), None);
    assert_eq!(load(&store, &id).status, ReviewStatus::InProgress);
}

#[test]
fn viewed_files_belong_to_the_round_in_progress() {
    let (_repo, store, id) = with_review();
    assert!(mark_viewed(&store, &id, "src/lib.rs", true).unwrap());
    assert!(!mark_viewed(&store, &id, "src/lib.rs", true).unwrap());
    assert!(matches!(
        rejected(mark_viewed(&store, &id, "../escape", true)),
        Rejection::InvalidPath { .. }
    ));
    assert_eq!(
        close_round(
            &store,
            &id,
            oid('a'),
            oid('b'),
            Some(Verdict::RequestChanges)
        )
        .unwrap(),
        1
    );
    assert!(mark_viewed(&store, &id, "other.rs", true).unwrap());
    assert!(!mark_viewed(&store, &id, "src/lib.rs", false).unwrap());
    let review = load(&store, &id);
    assert!(review.is_viewed(1, "src/lib.rs"));
    assert!(review.is_viewed(2, "other.rs") && !review.is_viewed(2, "src/lib.rs"));
    assert_eq!(review.rounds[0].verdict, Some(Verdict::RequestChanges));
    assert_eq!(
        close_round(&store, &id, oid('b'), oid('c'), None).unwrap(),
        2
    );
    assert_eq!(load(&store, &id).current_round(), 3);
}

#[test]
fn pr_links_and_submissions() {
    let (_repo, store, id) = with_review();
    let pr = PrLink {
        owner: "kylekz".to_string(),
        repo: "difftest".to_string(),
        number: 3,
    };
    assert!(link_pr(&store, &id, pr.clone()).unwrap());
    assert!(!link_pr(&store, &id, pr.clone()).unwrap());
    assert_eq!(load(&store, &id).pr, Some(pr));
    // Linked to a PR, a local review is still a local review.
    assert!(!load(&store, &id).is_pr_review());
    // Only a draft PR review is submitted.
    assert!(matches!(
        rejected(record_submission(&store, &id, 1, "u", Verdict::Approve)),
        Rejection::NotSubmittable { .. }
    ));

    let (_repo, store) = fixture_store();
    let id = open_review(&store, pr_target('b'), reviewer())
        .unwrap()
        .review
        .id;
    let thread = add_thread(&store, &id, anchor("src/lib.rs", 1), "Nit", reviewer()).unwrap();
    let mismatch = PrLink {
        owner: "kylekz".to_string(),
        repo: "difftest".to_string(),
        number: 13,
    };
    assert!(matches!(
        rejected(link_pr(&store, &id, mismatch)),
        Rejection::PrMismatch { .. }
    ));
    record_submission(
        &store,
        &id,
        900,
        "https://x.invalid/900",
        Verdict::RequestChanges,
    )
    .unwrap();
    let review = load(&store, &id);
    assert_eq!(review.status, ReviewStatus::Submitted);
    assert_eq!(review.submissions[0].github_review_id, 900);

    // Read-only from here on (PLAN §2.4).
    assert_eq!(
        rejected(reply(&store, &id, &thread, "late", agent())),
        Rejection::Submitted
    );
    assert_eq!(
        rejected(add_thread(
            &store,
            &id,
            anchor("src/lib.rs", 1),
            "x",
            reviewer()
        )),
        Rejection::Submitted
    );
    assert_eq!(
        rejected(mark_viewed(&store, &id, "src/lib.rs", true)),
        Rejection::Submitted
    );
    assert_eq!(
        rejected(set_status(
            &store,
            &id,
            ReviewStatus::Closed,
            AuthorKind::Reviewer
        )),
        Rejection::Submitted
    );
    assert_eq!(
        rejected(record_submission(&store, &id, 901, "u", Verdict::Approve)),
        Rejection::Submitted
    );
    assert_eq!(
        rejected(edit_body(
            &store,
            &id,
            &ItemId::Thread(thread.clone()),
            "x",
            &reviewer()
        )),
        Rejection::Submitted
    );
    assert_eq!(
        rejected(close_round(&store, &id, oid('a'), oid('b'), None)),
        Rejection::Submitted
    );
}

#[test]
fn only_an_items_author_edits_or_deletes_it() {
    let (_repo, store, id) = with_review();
    let thread = add_thread(&store, &id, anchor("src/lib.rs", 1), "Typo", reviewer()).unwrap();
    let answer = reply(&store, &id, &thread, "Fixed", agent()).unwrap();
    let thread_item = ItemId::Thread(thread.clone());
    let reply_item = ItemId::Comment(answer.clone());

    not_allowed(edit_body(&store, &id, &thread_item, "x", &agent()));
    not_allowed(edit_body(&store, &id, &reply_item, "x", &reviewer()));
    not_allowed(edit_body(
        &store,
        &id,
        &reply_item,
        "x",
        &Author::agent("codex"),
    ));
    not_allowed(delete_item(&store, &id, &reply_item, &reviewer()));
    assert!(matches!(
        edit_body(&store, &id, &reply_item, " ", &agent()),
        Err(OpError::Empty { .. })
    ));

    edit_body(&store, &id, &thread_item, "Typo here", &reviewer()).unwrap();
    edit_body(&store, &id, &reply_item, "Fixed in 3f2a", &agent()).unwrap();
    delete_item(&store, &id, &reply_item, &agent()).unwrap();
    assert!(matches!(
        rejected(delete_item(&store, &id, &reply_item, &agent())),
        Rejection::Deleted { .. }
    ));
    let review = load(&store, &id);
    let t = review.thread(&thread).unwrap();
    assert_eq!(t.body, "Typo here");
    assert_eq!(t.replies[0].body, "Fixed in 3f2a");
    assert!(t.replies[0].is_deleted());

    delete_item(&store, &id, &thread_item, &reviewer()).unwrap();
    assert_eq!(load(&store, &id).live_threads().count(), 0);
    assert!(matches!(
        rejected(reply(&store, &id, &thread, "x", agent())),
        Rejection::Deleted { .. }
    ));
    let ghost = ItemId::Comment(CommentId::generate());
    assert!(matches!(
        rejected(delete_item(&store, &id, &ghost, &agent())),
        Rejection::UnknownItem { .. }
    ));
}
