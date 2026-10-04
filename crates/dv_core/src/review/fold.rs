//! Folding a review's events into its state (docs/PLAN.md D5, §7), and the rules every event is
//! checked against on the way (PLAN §2.4, §2.5).
//!
//! The file order of the log is the total order. Writers append without a lock, each having
//! validated its event against the state it last read, so two of them can race (the reviewer
//! resolves a thread while an agent addresses it). The fold is the final word: it applies the
//! same rules in file order, and an event that doesn't apply to the state it lands on is skipped
//! with a [`LogWarning::Rejected`] instead of corrupting the state. The operation layer
//! (`review::ops`) checks a new event with this same code before appending it, so the two never
//! disagree about what is valid.
//!
//! An event that changes nothing (marking a viewed file viewed again, a status it already has)
//! applies silently as a no-op ([`Outcome::Unchanged`]).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::event::{Event, EventBody, LogWarning};
use super::id::{CommentId, EventId, ItemId, ReviewId, ThreadId};
use super::model::{
    Anchor, Author, AuthorKind, Comment, PrLink, Review, ReviewStatus, Round, StateChange,
    Submission, Thread, ThreadState,
};
use crate::backend::check_relative;
use crate::target::Target;

/// Why an event doesn't apply to the review state it lands on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "rejection", rename_all = "snake_case")]
pub enum Rejection {
    #[error("the log has no created event before it")]
    NotCreated,
    #[error("the review was already created")]
    AlreadyCreated,
    /// The same event id appears again: a line that landed twice.
    #[error("the event appears twice")]
    DuplicateEvent,
    #[error("no thread {thread}")]
    UnknownThread { thread: ThreadId },
    #[error("no comment {item}")]
    UnknownItem { item: ItemId },
    #[error("{id} already exists")]
    DuplicateId { id: String },
    #[error("{item} was deleted")]
    Deleted { item: ItemId },
    #[error("thread {thread} is mirrored from GitHub and read-only")]
    ReadOnlyThread { thread: ThreadId },
    #[error("a GitHub author can only write to threads mirrored from GitHub, not {thread}")]
    GithubOnLocalThread { thread: ThreadId },
    #[error("{} can't move a thread from {from} to {to}", author.noun())]
    ThreadTransition {
        from: ThreadState,
        to: ThreadState,
        author: AuthorKind,
    },
    #[error("the review can't go from {from} to {to}")]
    StatusTransition {
        from: ReviewStatus,
        to: ReviewStatus,
    },
    #[error("the review was submitted to GitHub and is read-only")]
    Submitted,
    #[error("only a draft PR review can be submitted (this one is {status})")]
    NotSubmittable { status: ReviewStatus },
    #[error("round {got} can't close next (round {expected} is in progress)")]
    RoundOrder { expected: u32, got: u32 },
    #[error("round {round} doesn't exist (round {current} is in progress)")]
    UnknownRound { round: u32, current: u32 },
    #[error("invalid anchor: {reason}")]
    InvalidAnchor { reason: String },
    #[error("invalid path {path:?}")]
    InvalidPath { path: String },
    #[error("the review targets {target}, not {linked}")]
    PrMismatch { target: PrLink, linked: PrLink },
}

/// What applying a valid event did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Changed,
    /// The state already said so: a no-op.
    Unchanged,
}

/// Why a log has no review in it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FoldError {
    #[error("the log has no valid created event")]
    NotCreated,
}

/// A folded review and the events that were skipped on the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folded {
    pub review: Review,
    /// [`LogWarning::Rejected`] for each skipped event, in file order.
    pub warnings: Vec<LogWarning>,
}

/// Folds a review's events, in file order, into its state (module docs). The review exists from
/// its `created` event on: events before it are skipped with a warning, and a log without one
/// is [`FoldError::NotCreated`].
pub fn fold(id: &ReviewId, events: &[Event]) -> Result<Folded, FoldError> {
    let mut folder: Option<Folder> = None;
    let mut warnings = Vec::new();
    let mut seen: HashSet<&EventId> = HashSet::with_capacity(events.len());
    for event in events {
        let result = if !seen.insert(&event.eid) {
            Err(Rejection::DuplicateEvent)
        } else {
            match &mut folder {
                Some(folder) => folder.apply(event).map(drop),
                None => Folder::create(id, event).map(|created| folder = Some(created)),
            }
        };
        if let Err(reason) = result {
            warnings.push(LogWarning::Rejected {
                eid: event.eid.clone(),
                kind: event.body.kind().to_string(),
                reason,
            });
        }
    }
    let review = folder.ok_or(FoldError::NotCreated)?.into_review();
    Ok(Folded { review, warnings })
}

/// The moves a thread's state may make (PLAN §2.5), for a thread opened by `thread_author` and
/// a change by `by`:
///
/// - Agents address; the reviewer may address too (say, after fixing something themselves).
///   Addressing an addressed thread again replaces the note. A resolved thread must be reopened
///   before it can be addressed.
/// - Only the reviewer resolves (from `open` or `addressed`) and reopens (back to `open`).
/// - A thread mirrored from GitHub changes only through its GitHub author's events (PLAN §9.2),
///   and a GitHub author changes no other thread.
///
/// `Ok(Outcome::Unchanged)` when the thread is already in `to` (except `addressed`, above).
pub fn check_thread_state(
    thread: &ThreadId,
    thread_author: AuthorKind,
    from: ThreadState,
    to: ThreadState,
    by: AuthorKind,
) -> Result<Outcome, Rejection> {
    use AuthorKind::{Agent, Github, Reviewer};
    use ThreadState::{Addressed, Open, Resolved};
    match (thread_author == Github, by == Github) {
        (true, false) => {
            return Err(Rejection::ReadOnlyThread {
                thread: thread.clone(),
            });
        }
        (false, true) => {
            return Err(Rejection::GithubOnLocalThread {
                thread: thread.clone(),
            });
        }
        (true, true) => {
            return Ok(if from == to {
                Outcome::Unchanged
            } else {
                Outcome::Changed
            });
        }
        (false, false) => {}
    }
    match (from, to, by) {
        (Open, Open, _) | (Resolved, Resolved, _) => Ok(Outcome::Unchanged),
        (Open | Addressed, Addressed, Agent | Reviewer)
        | (Open | Addressed, Resolved, Reviewer)
        | (Addressed | Resolved, Open, Reviewer) => Ok(Outcome::Changed),
        _ => Err(Rejection::ThreadTransition {
            from,
            to,
            author: by,
        }),
    }
}

/// The moves a review's status may make (PLAN §2.4). `status` events carry no author, so this
/// checks only the move; the operation layer checks who asks for it.
///
/// - A local review in `open`, `changes_requested`, `in_progress` or `ready` may move to any
///   other of `changes_requested`, `in_progress`, `ready`, `approved` and `closed`.
/// - A PR review in `draft` may only be abandoned (`closed`) this way; `submitted` comes from
///   the `submitted` event.
/// - `approved`, `closed` and `submitted` are final, and nothing moves back to `open` or
///   `draft`.
///
/// `Ok(Outcome::Unchanged)` when the review already has `to`.
pub fn check_status(from: ReviewStatus, to: ReviewStatus) -> Result<Outcome, Rejection> {
    use ReviewStatus::{
        Approved, ChangesRequested, Closed, Draft, InProgress, Open, Ready, Submitted,
    };
    if from == to {
        return Ok(Outcome::Unchanged);
    }
    let allowed = match from {
        Open | ChangesRequested | InProgress | Ready => {
            matches!(
                to,
                ChangesRequested | InProgress | Ready | Approved | Closed
            )
        }
        Draft => to == Closed,
        Approved | Closed | Submitted => false,
    };
    if allowed {
        Ok(Outcome::Changed)
    } else {
        Err(Rejection::StatusTransition { from, to })
    }
}

/// Checks an anchor's shape: valid paths, a 1-based line range in order, one `text` entry per
/// anchored line, and a round that exists (`1..=current`).
pub fn check_anchor(anchor: &Anchor, current_round: u32) -> Result<(), Rejection> {
    let invalid = |reason: String| Err(Rejection::InvalidAnchor { reason });
    if check_relative(&anchor.path).is_err() {
        return invalid(format!(
            "path {:?} is not a repo-relative path",
            anchor.path
        ));
    }
    if let Some(old_path) = &anchor.old_path
        && check_relative(old_path).is_err()
    {
        return invalid(format!("old path {old_path:?} is not a repo-relative path"));
    }
    if anchor.start_line == 0 {
        return invalid("lines are 1-based".to_string());
    }
    if anchor.end_line < anchor.start_line {
        return invalid(format!(
            "line range {}..{} is reversed",
            anchor.start_line, anchor.end_line
        ));
    }
    if anchor.text.len() as u64 != u64::from(anchor.line_count()) {
        return invalid(format!(
            "{} lines anchored but {} lines of text",
            anchor.line_count(),
            anchor.text.len()
        ));
    }
    check_round(anchor.round, current_round)
}

fn check_round(round: u32, current: u32) -> Result<(), Rejection> {
    if (1..=current).contains(&round) {
        Ok(())
    } else {
        Err(Rejection::UnknownRound { round, current })
    }
}

/// A review being folded, with indexes so each event finds its thread or reply directly.
#[derive(Debug, Clone)]
pub(crate) struct Folder {
    review: Review,
    /// Thread id → index in `review.threads`.
    threads: HashMap<ThreadId, usize>,
    /// Reply id → (thread index, reply index).
    comments: HashMap<CommentId, (usize, usize)>,
}

impl Folder {
    /// Starts a review from its `created` event.
    pub(crate) fn create(id: &ReviewId, event: &Event) -> Result<Self, Rejection> {
        let EventBody::Created {
            target,
            author,
            worktree,
        } = &event.body
        else {
            return Err(Rejection::NotCreated);
        };
        Ok(Self::from_review(new_review(
            id.clone(),
            event.at,
            target.clone(),
            author.clone(),
            worktree.clone(),
        )))
    }

    /// Continues from an already folded review (a cached one, for the operation layer).
    pub(crate) fn from_review(review: Review) -> Self {
        let mut threads = HashMap::with_capacity(review.threads.len());
        let mut comments = HashMap::new();
        for (t, thread) in review.threads.iter().enumerate() {
            threads.insert(thread.id.clone(), t);
            for (c, reply) in thread.replies.iter().enumerate() {
                comments.insert(reply.id.clone(), (t, c));
            }
        }
        Self {
            review,
            threads,
            comments,
        }
    }

    pub(crate) fn into_review(self) -> Review {
        self.review
    }

    /// Applies one event. Every check runs before anything changes, so a rejected event leaves
    /// the state exactly as it was.
    pub(crate) fn apply(&mut self, event: &Event) -> Result<Outcome, Rejection> {
        if matches!(event.body, EventBody::Created { .. }) {
            return Err(Rejection::AlreadyCreated);
        }
        // A submitted review belongs to GitHub from then on (PLAN §2.4).
        if self.review.status == ReviewStatus::Submitted {
            return Err(Rejection::Submitted);
        }
        let at = event.at;
        let outcome = match &event.body {
            EventBody::Created { .. } => return Err(Rejection::AlreadyCreated),
            EventBody::RoundClosed {
                n,
                new_oid,
                old_oid,
                verdict,
            } => {
                let expected = self.review.current_round();
                if *n != expected {
                    return Err(Rejection::RoundOrder { expected, got: *n });
                }
                self.review.rounds.push(Round {
                    n: *n,
                    old_oid: old_oid.clone(),
                    new_oid: new_oid.clone(),
                    verdict: *verdict,
                    closed_at: at,
                });
                Outcome::Changed
            }
            EventBody::Viewed { round, path, on } => {
                check_round(*round, self.review.current_round())?;
                check_path(path)?;
                let paths = self.review.viewed.entry(*round).or_default();
                let changed = if *on {
                    paths.insert(path.clone())
                } else {
                    paths.remove(path)
                };
                if paths.is_empty() {
                    self.review.viewed.remove(round);
                }
                if changed {
                    Outcome::Changed
                } else {
                    Outcome::Unchanged
                }
            }
            EventBody::ThreadAdded {
                id,
                anchor,
                body,
                author,
            } => {
                if self.threads.contains_key(id) {
                    return Err(Rejection::DuplicateId { id: id.to_string() });
                }
                check_anchor(anchor, self.review.current_round())?;
                self.threads.insert(id.clone(), self.review.threads.len());
                self.review.threads.push(Thread {
                    id: id.clone(),
                    anchor: anchor.clone(),
                    author: author.clone(),
                    body: body.clone(),
                    created_at: at,
                    edited_at: None,
                    deleted_at: None,
                    state: ThreadState::Open,
                    last_change: None,
                    replies: Vec::new(),
                });
                Outcome::Changed
            }
            EventBody::ReplyAdded {
                thread,
                id,
                body,
                author,
            } => {
                let t = self.live_thread(thread)?;
                check_thread_author(&self.review.threads[t], author.kind)?;
                if self.comments.contains_key(id) {
                    return Err(Rejection::DuplicateId { id: id.to_string() });
                }
                let replies = &mut self.review.threads[t].replies;
                self.comments.insert(id.clone(), (t, replies.len()));
                replies.push(Comment {
                    id: id.clone(),
                    author: author.clone(),
                    body: body.clone(),
                    created_at: at,
                    edited_at: None,
                    deleted_at: None,
                });
                Outcome::Changed
            }
            EventBody::ThreadState {
                thread,
                state,
                note,
                author,
            } => {
                let t = self.live_thread(thread)?;
                let current = &self.review.threads[t];
                let outcome = check_thread_state(
                    thread,
                    current.author.kind,
                    current.state,
                    *state,
                    author.kind,
                )?;
                if outcome == Outcome::Changed {
                    let current = &mut self.review.threads[t];
                    current.state = *state;
                    current.last_change = Some(StateChange {
                        state: *state,
                        note: note.clone(),
                        author: author.clone(),
                        at,
                    });
                }
                outcome
            }
            EventBody::BodyEdited { target_id, body } => {
                match self.live_item(target_id)? {
                    (t, None) => {
                        let thread = &mut self.review.threads[t];
                        thread.body = body.clone();
                        thread.edited_at = Some(at);
                    }
                    (t, Some(c)) => {
                        let reply = &mut self.review.threads[t].replies[c];
                        reply.body = body.clone();
                        reply.edited_at = Some(at);
                    }
                }
                Outcome::Changed
            }
            EventBody::Deleted { target_id } => {
                match self.live_item(target_id)? {
                    (t, None) => self.review.threads[t].deleted_at = Some(at),
                    (t, Some(c)) => self.review.threads[t].replies[c].deleted_at = Some(at),
                }
                Outcome::Changed
            }
            EventBody::PrLinked {
                owner,
                repo,
                number,
            } => {
                let linked = PrLink {
                    owner: owner.clone(),
                    repo: repo.clone(),
                    number: *number,
                };
                if let Target::Pr {
                    owner,
                    repo,
                    number,
                    ..
                } = &self.review.target
                {
                    let target = PrLink {
                        owner: owner.clone(),
                        repo: repo.clone(),
                        number: *number,
                    };
                    if !target.same_pr(&linked) {
                        return Err(Rejection::PrMismatch { target, linked });
                    }
                }
                if self
                    .review
                    .pr
                    .as_ref()
                    .is_some_and(|pr| pr.same_pr(&linked))
                {
                    Outcome::Unchanged
                } else {
                    self.review.pr = Some(linked);
                    Outcome::Changed
                }
            }
            EventBody::Submitted {
                github_review_id,
                url,
                event: verdict,
            } => {
                if self.review.status != ReviewStatus::Draft {
                    return Err(Rejection::NotSubmittable {
                        status: self.review.status,
                    });
                }
                self.review.status = ReviewStatus::Submitted;
                self.review.submissions.push(Submission {
                    github_review_id: *github_review_id,
                    url: url.clone(),
                    event: *verdict,
                    at,
                });
                Outcome::Changed
            }
            EventBody::Status { status } => {
                let outcome = check_status(self.review.status, *status)?;
                self.review.status = *status;
                outcome
            }
        };
        if outcome == Outcome::Changed {
            self.review.updated_at = self.review.updated_at.max(at);
        }
        Ok(outcome)
    }

    /// The index of a thread that exists and isn't deleted.
    fn live_thread(&self, id: &ThreadId) -> Result<usize, Rejection> {
        let &t = self
            .threads
            .get(id)
            .ok_or_else(|| Rejection::UnknownThread { thread: id.clone() })?;
        if self.review.threads[t].is_deleted() {
            return Err(Rejection::Deleted {
                item: ItemId::Thread(id.clone()),
            });
        }
        Ok(t)
    }

    /// Where a thread's opening comment (`(t, None)`) or a reply (`(t, Some(c))`) is, when it
    /// exists and neither it nor its thread is deleted.
    fn live_item(&self, item: &ItemId) -> Result<(usize, Option<usize>), Rejection> {
        match item {
            ItemId::Thread(id) => self.live_thread(id).map(|t| (t, None)),
            ItemId::Comment(id) => {
                let &(t, c) = self
                    .comments
                    .get(id)
                    .ok_or_else(|| Rejection::UnknownItem { item: item.clone() })?;
                let thread = &self.review.threads[t];
                if thread.is_deleted() {
                    return Err(Rejection::Deleted {
                        item: ItemId::Thread(thread.id.clone()),
                    });
                }
                if thread.replies[c].is_deleted() {
                    return Err(Rejection::Deleted { item: item.clone() });
                }
                Ok((t, Some(c)))
            }
        }
    }
}

/// A review as its `created` event makes it.
fn new_review(id: ReviewId, at: u64, target: Target, author: Author, worktree: String) -> Review {
    let pr = match &target {
        Target::Pr {
            owner,
            repo,
            number,
            ..
        } => Some(PrLink {
            owner: owner.clone(),
            repo: repo.clone(),
            number: *number,
        }),
        _ => None,
    };
    Review {
        id,
        status: ReviewStatus::initial(&target),
        target,
        worktree,
        author,
        created_at: at,
        updated_at: at,
        rounds: Vec::new(),
        viewed: Default::default(),
        threads: Vec::new(),
        pr,
        submissions: Vec::new(),
    }
}

/// Replies to a thread follow the same GitHub rule as its state ([`check_thread_state`]).
fn check_thread_author(thread: &Thread, by: AuthorKind) -> Result<(), Rejection> {
    match (thread.is_from_github(), by == AuthorKind::Github) {
        (true, false) => Err(Rejection::ReadOnlyThread {
            thread: thread.id.clone(),
        }),
        (false, true) => Err(Rejection::GithubOnLocalThread {
            thread: thread.id.clone(),
        }),
        _ => Ok(()),
    }
}

fn check_path(path: &str) -> Result<(), Rejection> {
    check_relative(path).map_err(|_| Rejection::InvalidPath {
        path: path.to_string(),
    })
}
