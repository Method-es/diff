//! Finishing a round (PLAN §2.3, §3 workflow A step 3): snapshot, `round_closed`, status by
//! verdict, and who may.

use crate::git::GitRepo;
use crate::review::ops::{OpError, finish_round, open_review, set_status};
use crate::review::snapshot::{self, SnapshotError};
use crate::testing::git_repo;

use super::store::{DeletesMidway, Trigger};
use super::*;

/// A repo with an edit in its working tree and a `working` review of it.
fn working_review() -> (FixtureRepo, ReviewStore, ReviewId) {
    let (repo, store) = fixture_store();
    repo.write("src/lib.rs", "one\nTWO\nthree\n");
    let opened = open_review(&store, Target::Working, reviewer()).unwrap();
    (repo, store, opened.review.id)
}

fn rev(repo: &FixtureRepo, rev: &str) -> String {
    repo.git(&["rev-parse", rev]).trim().to_string()
}

#[test]
fn finishing_a_round_snapshots_it_and_moves_the_status_by_verdict() {
    let (repo, store, id) = working_review();
    let git = git_repo(&repo);
    let head = rev(&repo, "HEAD");

    let finished = finish_round(
        &store,
        &git,
        &id,
        Some(Verdict::RequestChanges),
        Some("  Rename `TWO`.\n".to_string()),
        &reviewer(),
    )
    .unwrap();
    let round = &finished.round;
    assert_eq!(round.n, 1);
    assert_eq!(round.old_oid.as_str(), head);
    assert_eq!(
        round.new_oid.as_str(),
        rev(&repo, &snapshot::ref_name(&id, 1))
    );
    assert_eq!(round.verdict, Some(Verdict::RequestChanges));
    assert_eq!(round.body.as_deref(), Some("Rename `TWO`."));
    assert_eq!(
        repo.git(&["show", &format!("{}:src/lib.rs", round.new_oid)]),
        "one\nTWO\nthree\n"
    );
    assert_eq!(finished.review.status, ReviewStatus::ChangesRequested);
    // The verdict's status says who set it, and carries no note (the summary is the round's).
    let change = finished.review.status_change.clone().unwrap();
    assert_eq!(
        (change.status, change.note, change.author),
        (ReviewStatus::ChangesRequested, None, Some(reviewer()))
    );
    assert_eq!(finished.review.current_round(), 2);
    // What the log folds to is what the operation returned.
    let loaded = store.load(&id).unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(loaded.review, finished.review);

    // No verdict: a snapshot of the round, the status stays.
    repo.write("src/lib.rs", "one\ntwo\nthree\nfour\n");
    let finished =
        finish_round(&store, &git, &id, None, Some("  ".to_string()), &reviewer()).unwrap();
    assert_eq!(finished.round.n, 2);
    assert_eq!(finished.round.body, None, "a blank summary is no summary");
    assert_eq!(finished.round.verdict, None);
    assert_eq!(finished.review.status, ReviewStatus::ChangesRequested);
    assert_eq!(
        finished.round.new_oid.as_str(),
        rev(&repo, &snapshot::ref_name(&id, 2))
    );

    // Approve finishes the review.
    let finished =
        finish_round(&store, &git, &id, Some(Verdict::Approve), None, &reviewer()).unwrap();
    assert_eq!(finished.round.n, 3);
    assert_eq!(finished.review.status, ReviewStatus::Approved);
    assert_eq!(store.load(&id).unwrap().review, finished.review);
    // No round follows: the review shows the round that finished it.
    assert_eq!(finished.review.current_round(), 4);
    assert_eq!(finished.review.shown_round(), 3);
    assert!(matches!(
        finish_round(&store, &git, &id, None, None, &reviewer()),
        Err(OpError::Finished {
            status: ReviewStatus::Approved
        })
    ));
}

#[test]
fn a_handoff_landing_while_the_round_is_snapshotted_doesnt_outlive_the_verdict() {
    let (repo, store, id) = working_review();
    let git = git_repo(&repo);
    // Changes were requested, and the agent hasn't written since: still `changes_requested`.
    finish_round(
        &store,
        &git,
        &id,
        Some(Verdict::RequestChanges),
        None,
        &reviewer(),
    )
    .unwrap();
    repo.write("src/lib.rs", "one\ntwo\nthree\n");
    // The reviewer finishes the next round asking for more; it reads the review...
    let read = store.load_for_update(&id).unwrap().review;
    assert_eq!(read.status, ReviewStatus::ChangesRequested);
    // ...the agent's `dv review done` lands while the snapshot is taken...
    set_status(&store, &id, ReviewStatus::Ready, &agent(), Some("Done.")).unwrap();
    // ...and the round closes on what it read.
    let finished = crate::review::ops::finish_round_on(
        &store,
        &git,
        &id,
        read,
        Some(Verdict::RequestChanges),
        Some("More, please.".to_string()),
        &reviewer(),
    )
    .unwrap();
    let loaded = store.load(&id).unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(
        loaded.review.status,
        ReviewStatus::ChangesRequested,
        "the verdict wins over the handoff it closed the round after"
    );
    assert_eq!(finished.review.status, ReviewStatus::ChangesRequested);
    assert_eq!(loaded.review.last_round().map(|round| round.n), Some(2));

    // With nothing between, the repeated status is a no-op in the fold.
    let before = loaded.review.status_change.clone();
    repo.write("src/lib.rs", "one\ntwo\nthree\nfour\n");
    let finished = finish_round(
        &store,
        &git,
        &id,
        Some(Verdict::RequestChanges),
        None,
        &reviewer(),
    )
    .unwrap();
    let loaded = store.load(&id).unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(loaded.review, finished.review);
    assert_eq!(loaded.review.status_change, before);
}

#[test]
fn only_the_reviewer_finishes_a_local_round() {
    let (repo, store, id) = working_review();
    let git = git_repo(&repo);
    let events = || store.events(&id).unwrap().events.len();
    let before = events();
    for (verdict, author) in [
        (Some(Verdict::RequestChanges), agent()),
        (None, Author::github("octocat")),
        // A local round approves or requests changes.
        (Some(Verdict::Comment), reviewer()),
    ] {
        let result = finish_round(&store, &git, &id, verdict, None, &author);
        assert!(
            matches!(result, Err(OpError::NotAllowed { .. })),
            "{verdict:?} by {author:?}: {result:?}"
        );
    }
    assert_eq!(events(), before, "a refused round writes nothing");

    // A PR review's rounds close when it is submitted to GitHub.
    let pr = store
        .create(
            Target::Pr {
                owner: "kylekz".to_string(),
                repo: "difftest".to_string(),
                number: 3,
                base_oid: oid('a'),
                head_oid: oid('b'),
            },
            reviewer(),
        )
        .unwrap();
    assert!(matches!(
        finish_round(&store, &git, &pr.id, None, None, &reviewer()),
        Err(OpError::NotAllowed { .. })
    ));

    // A closed review is finished.
    set_status(&store, &id, ReviewStatus::Closed, &reviewer(), None).unwrap();
    assert!(matches!(
        finish_round(&store, &git, &id, None, None, &reviewer()),
        Err(OpError::Finished {
            status: ReviewStatus::Closed
        })
    ));
    assert!(
        repo.git(&["for-each-ref", "refs/dv/"]).is_empty(),
        "no snapshot was taken"
    );
}

#[test]
fn a_failed_snapshot_writes_no_event() {
    let (repo, store) = fixture_store();
    // Conflicts in the index: a staged review has nothing to snapshot.
    repo.branch("other").write("src/lib.rs", "main\n");
    repo.commit("main side");
    repo.checkout("other").write("src/lib.rs", "other\n");
    repo.commit("other side");
    repo.checkout("main");
    let merge = repo
        .git_command()
        .args(["merge", "-q", "other"])
        .output()
        .unwrap();
    assert!(!merge.status.success());
    let id = open_review(&store, Target::Staged, reviewer())
        .unwrap()
        .review
        .id;
    let git = git_repo(&repo);
    let before = store.events(&id).unwrap().events.len();
    assert!(matches!(
        finish_round(
            &store,
            &git,
            &id,
            Some(Verdict::RequestChanges),
            None,
            &reviewer()
        ),
        Err(OpError::Snapshot(SnapshotError::Unmerged { .. }))
    ));
    assert_eq!(store.events(&id).unwrap().events.len(), before);
    let review = store.load(&id).unwrap().review;
    assert_eq!(review.status, ReviewStatus::Open);
    assert!(review.rounds.is_empty());
}

#[test]
fn a_retry_after_a_lost_append_reuses_the_rounds_snapshot() {
    let (repo, store, id) = working_review();
    let git = git_repo(&repo);
    // The snapshot of an attempt whose events never landed.
    let spec = Target::Working.resolve(&git).unwrap();
    let earlier = snapshot::take(&git, &id, 1, &spec, None).unwrap();
    let finished = finish_round(
        &store,
        &git,
        &id,
        Some(Verdict::RequestChanges),
        None,
        &reviewer(),
    )
    .unwrap();
    assert_eq!(finished.round.new_oid, earlier);
}

#[test]
fn a_lost_append_doesnt_block_the_round_once_the_content_changed() {
    let (repo, store, id) = working_review();
    let git = git_repo(&repo);
    // The snapshot of an attempt whose events never landed, then the agent edits on.
    let spec = Target::Working.resolve(&git).unwrap();
    let earlier = snapshot::take(&git, &id, 1, &spec, None).unwrap();
    repo.write("src/lib.rs", "one\nTWO\nthree\nfour\n");
    let finished = finish_round(
        &store,
        &git,
        &id,
        Some(Verdict::RequestChanges),
        None,
        &reviewer(),
    )
    .unwrap();
    assert_ne!(finished.round.new_oid, earlier);
    assert_eq!(
        rev(&repo, &snapshot::ref_name(&id, 1)),
        finished.round.new_oid.as_str()
    );
    assert_eq!(
        repo.git(&["show", &format!("{}:src/lib.rs", finished.round.new_oid)]),
        "one\nTWO\nthree\nfour\n"
    );
    // Round 1's ref is named by a closed round now: round 2 leaves it alone.
    repo.write("src/lib.rs", "five\n");
    let second = finish_round(&store, &git, &id, None, None, &reviewer()).unwrap();
    assert_eq!(second.round.n, 2);
    assert_eq!(
        rev(&repo, &snapshot::ref_name(&id, 1)),
        finished.round.new_oid.as_str()
    );
}

#[test]
fn a_lost_append_at_the_last_rounds_commit_doesnt_block_the_next_round() {
    // A branch target whose branch isn't checked out: its new side is the tip commit.
    let (repo, store) = fixture_store();
    repo.branch("feat");
    repo.checkout("feat");
    repo.write("src/lib.rs", "one\nTWO\nthree\n");
    let first = repo.commit("first");
    repo.checkout("main");
    let target = Target::Branch {
        branch: "feat".to_string(),
        base: Some("main".to_string()),
    };
    let id = open_review(&store, target, reviewer()).unwrap().review.id;
    let git = git_repo(&repo);
    let round_one = finish_round(
        &store,
        &git,
        &id,
        Some(Verdict::RequestChanges),
        None,
        &reviewer(),
    )
    .unwrap();
    assert_eq!(round_one.round.new_oid.as_str(), first);
    // Round 2 finished before the tip moved, and its events never landed: its ref holds the
    // commit round 1 closed at.
    repo.git(&["update-ref", &snapshot::ref_name(&id, 2), &first]);
    repo.checkout("feat");
    repo.write("src/lib.rs", "one\ntwo\nthree\n");
    let second = repo.commit("second");
    repo.checkout("main");
    let round_two = finish_round(&store, &git, &id, None, None, &reviewer()).unwrap();
    assert_eq!(round_two.round.n, 2);
    assert_eq!(round_two.round.new_oid.as_str(), second);
    assert_eq!(rev(&repo, &snapshot::ref_name(&id, 2)), second);
    assert_eq!(rev(&repo, &snapshot::ref_name(&id, 1)), first);
}

#[test]
fn an_unborn_repo_closes_its_round_against_the_empty_tree() {
    let repo = FixtureRepo::new();
    let store = ReviewStore::new(std::sync::Arc::new(crate::testing::open(&repo)));
    repo.write("first.txt", "1\n");
    let id = open_review(&store, Target::Working, reviewer())
        .unwrap()
        .review
        .id;
    let git = git_repo(&repo);
    let finished =
        finish_round(&store, &git, &id, Some(Verdict::Approve), None, &reviewer()).unwrap();
    assert_eq!(finished.round.old_oid, git.empty_tree());
    assert_eq!(
        repo.git(&["log", "--format=%P", finished.round.new_oid.as_str()]),
        "\n"
    );
}

#[test]
fn a_review_deleted_while_its_round_is_snapshotted_leaves_no_ref() {
    let (repo, store, id) = working_review();
    // Another process deletes the review once the snapshot's `git add` has run, so the delete
    // looks for the review's refs before the snapshot makes its own.
    let git = GitRepo::new(Arc::new(DeletesMidway::new(
        &repo,
        Trigger::Git("add"),
        &id,
    )));
    let result = finish_round(
        &store,
        &git,
        &id,
        Some(Verdict::RequestChanges),
        None,
        &reviewer(),
    );
    assert!(
        matches!(
            result,
            Err(OpError::Store(StoreError::UnknownReview { .. }))
        ),
        "{result:?}"
    );
    // The round's snapshot ref, made after the delete looked for refs, went with the round.
    assert_eq!(repo.git(&["for-each-ref", "refs/dv/"]), "");
    assert!(store.list().unwrap().is_empty());
}
