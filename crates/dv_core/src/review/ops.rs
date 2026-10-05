//! Checked review operations (docs/PLAN.md §2.4, §2.5): each loads the review, checks the
//! change against its current state and who is asking, and only then appends the event.
//!
//! The checks the log can carry are the fold's own ([`super::fold`]), run on the loaded state
//! before anything is written, so an operation never appends an event the fold would skip given
//! that state. On top of them come the checks that need to know who is asking, which some
//! events don't record (`status`, `body_edited`, `deleted`): agents set only `in_progress` and
//! `ready`, the reviewer the verdict statuses, and only an item's author edits or deletes it.
//!
//! Writers don't lock (PLAN D5), so another writer may append between the load and the append;
//! the fold then applies both in file order under the same rules, and an event that no longer
//! fits is skipped there with a warning (module docs of [`super::fold`]).
//!
//! An operation's load uses a fresh snapshot cache but never writes one
//! (`ReviewStore::load_for_update`): its own append would make it stale at once (PLAN §7).

use super::event::{Event, EventBody};
use super::fold::{Folder, Outcome, Rejection};
use super::id::{CommentId, ItemId, ReviewId, ThreadId};
use super::model::{
    Anchor, Author, AuthorKind, PrLink, Review, ReviewStatus, ReviewSummary, Round, ThreadState,
    Verdict,
};
use super::snapshot::{self, SnapshotError};
use super::store::{ListEntry, ReviewStore, StoreError};
use crate::git::{GitRepo, Oid};
use crate::target::{Side, Target, TargetError};

/// Why an operation was refused.
#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The change doesn't apply to the review as it stands.
    #[error(transparent)]
    Rejected(#[from] Rejection),
    /// The change is valid, but not for this author (PLAN §2.4, §2.5).
    #[error("{} can't {action}", actor.noun())]
    NotAllowed { actor: AuthorKind, action: String },
    #[error("the {what} is empty")]
    Empty { what: &'static str },
    /// The review is `approved`, `closed` or `submitted`: nothing more happens to it.
    #[error("the review is {status}")]
    Finished { status: ReviewStatus },
    /// The review's target no longer resolves (a branch that was deleted, say).
    #[error(transparent)]
    Target(#[from] TargetError),
    #[error("couldn't snapshot the round: {0}")]
    Snapshot(#[from] SnapshotError),
}

/// What [`finish_round`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedRound {
    /// The round that closed: its number, its two sides, its verdict and summary.
    pub round: Round,
    /// The review after the round closed and its status followed the verdict.
    pub review: Review,
}

/// What [`open_review`] found or made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub review: Review,
    /// `false` when an open review of the target already existed and is returned instead.
    pub created: bool,
}

/// Opens the review of `target` in the store's worktree: the open review of that target when
/// there is one ("at most one open review per target", PLAN §2.4), else a new one by `author`.
///
/// A `working` or `staged` target belongs to its worktree, so another worktree's review of
/// one doesn't count; the other targets name the same commits from every worktree. A PR target
/// matches by pull request, whatever head it was fetched at. When several open reviews match
/// (two openers raced), the most recently updated one wins. Logs that fail to load are passed
/// over.
pub fn open_review(store: &ReviewStore, target: Target, author: Author) -> Result<Opened, OpError> {
    let worktree = store.worktree();
    let existing = store
        .list()?
        .into_iter()
        .filter_map(|entry| match entry {
            ListEntry::Ok(summary) => Some(*summary),
            ListEntry::Err { .. } => None,
        })
        .filter(|summary| summary.status.is_open() && is_review_of(summary, &target, worktree))
        .max_by_key(|summary| summary.updated_at);
    if let Some(summary) = existing {
        let review = store.load(&summary.id)?.review;
        return Ok(Opened {
            review,
            created: false,
        });
    }
    Ok(Opened {
        review: store.create(target, author)?,
        created: true,
    })
}

/// Adds a thread anchored at `anchor` with its opening comment; returns its id.
pub fn add_thread(
    store: &ReviewStore,
    review: &ReviewId,
    anchor: Anchor,
    body: &str,
    author: Author,
) -> Result<ThreadId, OpError> {
    check_body(body, "comment")?;
    let id = ThreadId::generate();
    commit(
        store,
        review,
        EventBody::ThreadAdded {
            id: id.clone(),
            anchor,
            body: body.to_string(),
            author,
        },
        no_extra_check,
    )?;
    Ok(id)
}

/// Replies to a thread; returns the reply's id.
pub fn reply(
    store: &ReviewStore,
    review: &ReviewId,
    thread: &ThreadId,
    body: &str,
    author: Author,
) -> Result<CommentId, OpError> {
    check_body(body, "reply")?;
    let id = CommentId::generate();
    commit(
        store,
        review,
        EventBody::ReplyAdded {
            thread: thread.clone(),
            id: id.clone(),
            body: body.to_string(),
            author,
        },
        no_extra_check,
    )?;
    Ok(id)
}

/// Moves a thread to `state` with an optional note (PLAN §2.5: agents address, only the
/// reviewer resolves and reopens; [`super::fold::check_thread_state`]). `false` when the thread
/// was already in that state and nothing was written.
pub fn set_thread_state(
    store: &ReviewStore,
    review: &ReviewId,
    thread: &ThreadId,
    state: ThreadState,
    note: Option<&str>,
    author: Author,
) -> Result<bool, OpError> {
    let note = note.map(str::trim).filter(|note| !note.is_empty());
    let outcome = commit(
        store,
        review,
        EventBody::ThreadState {
            thread: thread.clone(),
            state,
            note: note.map(str::to_string),
            author,
        },
        no_extra_check,
    )?;
    Ok(outcome == Outcome::Changed)
}

/// Marks `path` viewed (`on`) or not viewed in the round in progress. `false` when it already
/// was and nothing was written.
pub fn mark_viewed(
    store: &ReviewStore,
    review: &ReviewId,
    path: &str,
    on: bool,
) -> Result<bool, OpError> {
    let loaded = store.load_for_update(review)?;
    let round = loaded.review.current_round();
    let outcome = apply_and_append(
        store,
        review,
        loaded.review,
        EventBody::Viewed {
            round,
            path: path.to_string(),
            on,
        },
    )?;
    Ok(outcome == Outcome::Changed)
}

/// Sets the review's status on behalf of `actor` (PLAN §2.4): an agent sets `in_progress` and
/// `ready` (`dv review done`); the reviewer sets `changes_requested`, `approved` and `closed`;
/// GitHub sets nothing. The move itself must be allowed ([`super::fold::check_status`]).
/// `false` when the review already had that status and nothing was written.
pub fn set_status(
    store: &ReviewStore,
    review: &ReviewId,
    status: ReviewStatus,
    actor: AuthorKind,
) -> Result<bool, OpError> {
    use ReviewStatus::{Approved, ChangesRequested, Closed, InProgress, Ready};
    let allowed = match actor {
        AuthorKind::Agent => matches!(status, InProgress | Ready),
        AuthorKind::Reviewer => matches!(status, ChangesRequested | Approved | Closed),
        AuthorKind::Github => false,
    };
    if !allowed {
        return Err(OpError::NotAllowed {
            actor,
            action: format!("set a review's status to {status}"),
        });
    }
    let outcome = commit(store, review, EventBody::Status { status }, no_extra_check)?;
    Ok(outcome == Outcome::Changed)
}

/// Links the review to a pull request (a PR target is already linked to its own). `false` when
/// it already was.
pub fn link_pr(store: &ReviewStore, review: &ReviewId, pr: PrLink) -> Result<bool, OpError> {
    let outcome = commit(
        store,
        review,
        EventBody::PrLinked {
            owner: pr.owner,
            repo: pr.repo,
            number: pr.number,
        },
        no_extra_check,
    )?;
    Ok(outcome == Outcome::Changed)
}

/// Records that a draft PR review was posted to GitHub (PLAN §9.3). The review is read-only
/// from then on.
pub fn record_submission(
    store: &ReviewStore,
    review: &ReviewId,
    github_review_id: u64,
    url: &str,
    event: Verdict,
) -> Result<(), OpError> {
    commit(
        store,
        review,
        EventBody::Submitted {
            github_review_id,
            url: url.to_string(),
            event,
        },
        no_extra_check,
    )?;
    Ok(())
}

/// Closes the round in progress at these two sides (PLAN §2.3); returns its number. Taking the
/// snapshot the new side names is the caller's job.
pub fn close_round(
    store: &ReviewStore,
    review: &ReviewId,
    old_oid: Oid,
    new_oid: Oid,
    verdict: Option<Verdict>,
) -> Result<u32, OpError> {
    let loaded = store.load_for_update(review)?;
    let n = loaded.review.current_round();
    apply_and_append(
        store,
        review,
        loaded.review,
        EventBody::RoundClosed {
            n,
            new_oid,
            old_oid,
            verdict,
            body: None,
        },
    )?;
    Ok(n)
}

/// Finishes the round in progress of a local review (PLAN §2.3, §3 workflow A step 3; the
/// "Finish round" sheet, DESIGN §4.2): resolves the target as it stands now, snapshots its new
/// side at `refs/dv/reviews/<id>/<n>` ([`snapshot::take`]), appends `round_closed` with the
/// resolved old side, the snapshot, the verdict and the summary `body`, then moves the status
/// by the verdict: `request_changes` to `changes_requested`, `approve` to `approved`, none
/// leaves it as it is.
///
/// Only the reviewer finishes a round, never of a finished review and never of a PR review,
/// whose rounds close when it is submitted to GitHub (PLAN §2.3, §9.3); a local round's verdict
/// approves or requests changes, never `comment`. Both events are checked before either is
/// written, and the snapshot is taken before both, so a failed snapshot leaves the log as it
/// was. Retrying after a failed append finds the round's ref already there: it is reused when
/// the content is unchanged, and replaced otherwise, also when it holds the commit an earlier
/// round closed at (a branch that hadn't moved), since the round it belongs to never closed
/// ([`snapshot::take`]).
pub fn finish_round(
    store: &ReviewStore,
    repo: &GitRepo,
    review: &ReviewId,
    verdict: Option<Verdict>,
    body: Option<String>,
    author: &Author,
) -> Result<FinishedRound, OpError> {
    let not_allowed = |action: &str| OpError::NotAllowed {
        actor: author.kind,
        action: action.to_string(),
    };
    if author.kind != AuthorKind::Reviewer {
        return Err(not_allowed("finish a round"));
    }
    if verdict == Some(Verdict::Comment) {
        return Err(not_allowed(
            "finish a local round with a comment verdict (approve or request changes)",
        ));
    }
    let loaded = store.load_for_update(review)?;
    let state = loaded.review;
    if state.status.is_final() {
        return Err(OpError::Finished {
            status: state.status,
        });
    }
    if state.is_pr_review() {
        return Err(not_allowed(
            "finish a PR review's round here (it closes when submitted to GitHub)",
        ));
    }

    let spec = state.target.resolve(repo)?;
    let old_oid = match &spec.old {
        Side::Commit(oid) => oid.clone(),
        Side::EmptyTree => repo.empty_tree(),
        // Targets resolve their old side to a tree; the snapshot refuses anything else.
        Side::Index | Side::Worktree => {
            return Err(SnapshotError::UnsupportedSide {
                side: spec.old.to_string(),
            }
            .into());
        }
    };
    // The round in progress never closed, so its ref holds at most the snapshot of an attempt
    // whose events didn't land.
    let n = state.current_round();
    let new_oid = snapshot::take(repo, review, n, &spec, None)?;

    let body = body
        .map(|body| body.trim().to_string())
        .filter(|body| !body.is_empty());
    let round_closed = Event::new(EventBody::RoundClosed {
        n,
        new_oid,
        old_oid,
        verdict,
        body,
    });
    let status = match verdict {
        Some(Verdict::RequestChanges) => Some(ReviewStatus::ChangesRequested),
        Some(Verdict::Approve) => Some(ReviewStatus::Approved),
        Some(Verdict::Comment) | None => None,
    };
    let status = status.map(|status| Event::new(EventBody::Status { status }));
    let mut folder = Folder::from_review(state);
    folder.apply(&round_closed)?;
    let status = match status {
        Some(event) => (folder.apply(&event)? == Outcome::Changed).then_some(event),
        None => None,
    };
    store.append_event(review, &round_closed)?;
    if let Some(event) = &status {
        store.append_event(review, event)?;
    }
    let review = folder.into_review();
    let round = review
        .rounds
        .last()
        .cloned()
        .expect("applying round_closed appended the round");
    Ok(FinishedRound { round, review })
}

/// Replaces a thread's opening comment or a reply. Only its author may.
pub fn edit_body(
    store: &ReviewStore,
    review: &ReviewId,
    item: &ItemId,
    body: &str,
    editor: &Author,
) -> Result<(), OpError> {
    check_body(body, "comment")?;
    commit(
        store,
        review,
        EventBody::BodyEdited {
            target_id: item.clone(),
            body: body.to_string(),
        },
        |state| check_item_author(state, item, editor, "edit"),
    )?;
    Ok(())
}

/// Deletes a thread or a reply (a tombstone). Only its author may.
pub fn delete_item(
    store: &ReviewStore,
    review: &ReviewId,
    item: &ItemId,
    by: &Author,
) -> Result<(), OpError> {
    commit(
        store,
        review,
        EventBody::Deleted {
            target_id: item.clone(),
        },
        |state| check_item_author(state, item, by, "delete"),
    )?;
    Ok(())
}

/// Loads the review, runs `extra` on it, then checks and appends `body`.
fn commit(
    store: &ReviewStore,
    review: &ReviewId,
    body: EventBody,
    extra: impl FnOnce(&Review) -> Result<(), OpError>,
) -> Result<Outcome, OpError> {
    let loaded = store.load_for_update(review)?;
    extra(&loaded.review)?;
    apply_and_append(store, review, loaded.review, body)
}

/// Checks `body` against `state` with the fold's own rules and appends it when it changes
/// something.
fn apply_and_append(
    store: &ReviewStore,
    review: &ReviewId,
    state: Review,
    body: EventBody,
) -> Result<Outcome, OpError> {
    let event = Event::new(body);
    let outcome = Folder::from_review(state).apply(&event)?;
    if outcome == Outcome::Changed {
        store.append_event(review, &event)?;
    }
    Ok(outcome)
}

fn no_extra_check(_: &Review) -> Result<(), OpError> {
    Ok(())
}

/// Only an item's author edits or deletes it; GitHub's items are read-only (PLAN §9.2). An item
/// that doesn't exist is left to the fold's check, which says so.
fn check_item_author(
    state: &Review,
    item: &ItemId,
    actor: &Author,
    verb: &str,
) -> Result<(), OpError> {
    match state.item_author(item) {
        Some(author) if author != actor => Err(OpError::NotAllowed {
            actor: actor.kind,
            action: format!("{verb} {item}, written by {} {}", author.kind, author.name),
        }),
        _ => Ok(()),
    }
}

fn check_body(body: &str, what: &'static str) -> Result<(), OpError> {
    if body.trim().is_empty() {
        Err(OpError::Empty { what })
    } else {
        Ok(())
    }
}

/// Whether `summary` reviews `target` as opened from `worktree` (the worktree root, as
/// `ReviewStore::worktree` names it), open or not: the rule [`open_review`] resumes by, which
/// the switcher also uses to show a target's open review beside it.
pub fn is_review_of(summary: &ReviewSummary, target: &Target, worktree: &str) -> bool {
    same_target(&summary.target, target)
        && (!is_worktree_bound(target) || summary.worktree == worktree)
}

/// Whether two targets are the same for "at most one open review per target": the same spec,
/// except that a PR is the same PR whatever commits it was fetched at.
fn same_target(a: &Target, b: &Target) -> bool {
    match (a, b) {
        (
            Target::Pr {
                owner: owner_a,
                repo: repo_a,
                number: number_a,
                ..
            },
            Target::Pr {
                owner: owner_b,
                repo: repo_b,
                number: number_b,
                ..
            },
        ) => {
            number_a == number_b
                && owner_a.eq_ignore_ascii_case(owner_b)
                && repo_a.eq_ignore_ascii_case(repo_b)
        }
        _ => a == b,
    }
}

/// Whether a target means different changes in different worktrees of one repo.
fn is_worktree_bound(target: &Target) -> bool {
    matches!(target, Target::Working | Target::Staged)
}
