//! A review's folded state (docs/PLAN.md §2) and the value types its events carry (PLAN §7).
//!
//! Everything here is plain data: events build it ([`super::fold`]), the snapshot cache stores
//! it, the UI and the CLI read it. None of these types uses `deny_unknown_fields`, so a newer dv
//! may add optional fields without a version bump (PLAN §7, §15).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use super::id::{CommentId, ItemId, ReviewId, ThreadId};
use crate::git::Oid;
use crate::target::Target;

/// Who wrote something (PLAN §2.5).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Author {
    pub kind: AuthorKind,
    /// The reviewer's git identity, the agent's name (`claude-code`, `codex`, …) or the GitHub
    /// login.
    pub name: String,
}

impl Author {
    pub fn reviewer(name: impl Into<String>) -> Self {
        Self {
            kind: AuthorKind::Reviewer,
            name: name.into(),
        }
    }

    pub fn agent(name: impl Into<String>) -> Self {
        Self {
            kind: AuthorKind::Agent,
            name: name.into(),
        }
    }

    pub fn github(login: impl Into<String>) -> Self {
        Self {
            kind: AuthorKind::Github,
            name: login.into(),
        }
    }
}

/// The three kinds of author (PLAN §2.5): the reviewer resolves, agents address, and `github`
/// marks threads pulled from GitHub, which are read-only in dv (PLAN §9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorKind {
    Reviewer,
    Agent,
    Github,
}

impl AuthorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reviewer => "reviewer",
            Self::Agent => "agent",
            Self::Github => "github",
        }
    }

    /// The kind as the subject of a sentence: "the reviewer", "an agent", "GitHub".
    pub fn noun(self) -> &'static str {
        match self {
            Self::Reviewer => "the reviewer",
            Self::Agent => "an agent",
            Self::Github => "GitHub",
        }
    }
}

/// The verdict a round or a GitHub review closes with (PLAN §2.3, §9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Comment,
    Approve,
    RequestChanges,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Comment => "comment",
            Self::Approve => "approve",
            Self::RequestChanges => "request_changes",
        }
    }
}

/// Where a review stands (PLAN §2.4).
///
/// A local review moves `open` → `changes_requested` ⇄ `in_progress` → `ready` (the agent ran
/// `dv review done`) → `approved` | `closed`; a PR review moves `draft` → `submitted` (or is
/// abandoned: `closed`). `approved`, `closed` and `submitted` are final. The allowed moves are
/// [`super::fold::check_status`]'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    /// A local review nobody has finished a round of yet.
    Open,
    /// The reviewer finished a round asking for changes; the agent's turn.
    ChangesRequested,
    /// The agent is working on the requested changes.
    InProgress,
    /// The agent says the round is addressed (`dv review done`); the reviewer's turn.
    Ready,
    Approved,
    Closed,
    /// A PR review not yet submitted to GitHub.
    Draft,
    /// Submitted to GitHub: read-only from then on (PLAN §2.4).
    Submitted,
}

impl ReviewStatus {
    /// The status a review starts in: `draft` for a PR target, `open` otherwise.
    pub fn initial(target: &Target) -> Self {
        match target {
            Target::Pr { .. } => Self::Draft,
            _ => Self::Open,
        }
    }

    /// Whether the review is still active, which "at most one open review per target" counts
    /// (PLAN §2.4): everything but `approved`, `closed` and `submitted`.
    pub fn is_open(self) -> bool {
        !self.is_final()
    }

    /// Whether no status change can follow.
    pub fn is_final(self) -> bool {
        matches!(self, Self::Approved | Self::Closed | Self::Submitted)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::ChangesRequested => "changes_requested",
            Self::InProgress => "in_progress",
            Self::Ready => "ready",
            Self::Approved => "approved",
            Self::Closed => "closed",
            Self::Draft => "draft",
            Self::Submitted => "submitted",
        }
    }
}

/// A thread's state (PLAN §2.5): `open` (waiting on the agent or author) → `addressed` (the
/// agent says done) → `resolved` (the reviewer confirmed), or back to `open` (reopened). The
/// allowed moves are [`super::fold::check_thread_state`]'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadState {
    Open,
    Addressed,
    Resolved,
}

impl ThreadState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Addressed => "addressed",
            Self::Resolved => "resolved",
        }
    }
}

macro_rules! display_as_str {
    ($($name:ident),*) => {
        $(impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        })*
    };
}
display_as_str!(AuthorKind, Verdict, ReviewStatus, ThreadState);

/// Which side of the diff an anchor's lines are on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorSide {
    Old,
    New,
}

/// Where a thread is attached (PLAN §2.5, §8): a line range on one side of one file in one
/// round, plus the anchored lines' text and up to 3 lines of context either side, so the thread
/// can be relocated when the file moves on and shown with its original code when it can't.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    /// The round the line numbers refer to (1-based).
    pub round: u32,
    /// The file's path on the anchored side, relative to the worktree root, `/`-separated.
    pub path: String,
    /// The file's old path, for a renamed file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub side: AnchorSide,
    /// The first anchored line, 1-based.
    pub start_line: u32,
    /// The last anchored line, inclusive (`start_line` for one line).
    pub end_line: u32,
    /// The anchored lines' text without line endings, one entry per line.
    pub text: Vec<String>,
    /// Up to 3 lines before `start_line`.
    #[serde(default)]
    pub context_before: Vec<String>,
    /// Up to 3 lines after `end_line`.
    #[serde(default)]
    pub context_after: Vec<String>,
}

impl Anchor {
    /// How many lines the anchor spans.
    pub fn line_count(&self) -> u32 {
        self.end_line
            .saturating_sub(self.start_line)
            .saturating_add(1)
    }
}

/// A GitHub pull request a review is linked to (PLAN §9).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrLink {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl PrLink {
    /// Whether two links name the same pull request; GitHub owner and repository names are
    /// case-insensitive.
    pub fn same_pr(&self, other: &PrLink) -> bool {
        self.number == other.number
            && self.owner.eq_ignore_ascii_case(&other.owner)
            && self.repo.eq_ignore_ascii_case(&other.repo)
    }
}

impl fmt::Display for PrLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}#{}", self.owner, self.repo, self.number)
    }
}

/// A GitHub review dv posted for this review (PLAN §9.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission {
    pub github_review_id: u64,
    pub url: String,
    pub event: Verdict,
    /// When it was recorded, in ms since the Unix epoch.
    pub at: u64,
}

/// A closed round (PLAN §2.3): the two sides as they stood when the reviewer handed the review
/// over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Round {
    /// 1-based.
    pub n: u32,
    /// The old side: a commit, or the empty tree.
    pub old_oid: Oid,
    /// The new side: a commit (a snapshot at `refs/dv/reviews/<id>/<n>` for a local target).
    pub new_oid: Oid,
    /// `None` for a plain "Snapshot round".
    pub verdict: Option<Verdict>,
    /// When the round closed, in ms since the Unix epoch.
    pub closed_at: u64,
}

/// The last state change a thread went through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateChange {
    pub state: ThreadState,
    /// The agent's "what I did" or the reviewer's reason.
    pub note: Option<String>,
    pub author: Author,
    pub at: u64,
}

/// An anchored thread (PLAN §2.5): the opening comment, its replies and its state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub id: ThreadId,
    pub anchor: Anchor,
    /// The opening comment's author.
    pub author: Author,
    /// The opening comment, Markdown.
    pub body: String,
    pub created_at: u64,
    /// When the opening comment was last edited.
    pub edited_at: Option<u64>,
    /// When the thread was deleted: a tombstone, so later events naming it are recognised (and
    /// skipped) instead of reported as unknown (PLAN §7). Views hide deleted threads.
    pub deleted_at: Option<u64>,
    pub state: ThreadState,
    /// The change that set `state`; `None` while the thread has never left `open`.
    pub last_change: Option<StateChange>,
    /// Replies in the order they landed, deleted ones included (tombstones).
    pub replies: Vec<Comment>,
}

impl Thread {
    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }

    /// Whether the thread mirrors one on GitHub (read-only in dv, PLAN §9.2).
    pub fn is_from_github(&self) -> bool {
        self.author.kind == AuthorKind::Github
    }

    pub fn reply(&self, id: &CommentId) -> Option<&Comment> {
        self.replies.iter().find(|reply| &reply.id == id)
    }
}

/// A reply in a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: CommentId,
    pub author: Author,
    /// Markdown.
    pub body: String,
    pub created_at: u64,
    pub edited_at: Option<u64>,
    /// A tombstone, like [`Thread::deleted_at`].
    pub deleted_at: Option<u64>,
}

impl Comment {
    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}

/// One review, folded from its event log (PLAN §2.1, §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub id: ReviewId,
    /// Fixed at creation, never changes (PLAN D3).
    pub target: Target,
    /// The worktree root the review was created in, as its backend spells it. A `working` or
    /// `staged` target means that worktree's changes; every worktree of a repo shares one store
    /// (PLAN §7).
    pub worktree: String,
    /// Who created the review.
    pub author: Author,
    /// In ms since the Unix epoch, like every timestamp here.
    pub created_at: u64,
    /// The latest timestamp of an event that changed something.
    pub updated_at: u64,
    pub status: ReviewStatus,
    /// Closed rounds, round 1 first; the round in progress is [`Review::current_round`].
    pub rounds: Vec<Round>,
    /// Per round, the paths the reviewer marked viewed.
    pub viewed: BTreeMap<u32, BTreeSet<String>>,
    /// Threads in the order they were added, deleted ones included (tombstones).
    pub threads: Vec<Thread>,
    /// The pull request this review belongs to: a PR target's own, or one linked later.
    pub pr: Option<PrLink>,
    pub submissions: Vec<Submission>,
}

impl Review {
    /// The round in progress: one past the last closed round.
    pub fn current_round(&self) -> u32 {
        u32::try_from(self.rounds.len())
            .unwrap_or(u32::MAX)
            .saturating_add(1)
    }

    /// Whether this is a PR review, as opposed to a local one: its target is a pull request.
    /// That is fixed at creation (PLAN D3), unlike its status (an abandoned draft is `closed`,
    /// a status local reviews have too), and unlike its PR link (a local review may be linked to
    /// a PR and keeps the local lifecycle, PLAN §2.4).
    pub fn is_pr_review(&self) -> bool {
        matches!(self.target, Target::Pr { .. })
    }

    /// A thread by id, deleted or not.
    pub fn thread(&self, id: &ThreadId) -> Option<&Thread> {
        self.threads.iter().find(|thread| &thread.id == id)
    }

    /// The threads that aren't deleted.
    pub fn live_threads(&self) -> impl Iterator<Item = &Thread> {
        self.threads.iter().filter(|thread| !thread.is_deleted())
    }

    /// Whether `path` is marked viewed in `round`.
    pub fn is_viewed(&self, round: u32, path: &str) -> bool {
        self.viewed
            .get(&round)
            .is_some_and(|paths| paths.contains(path))
    }

    /// The author of a thread's opening comment or of a reply; `None` when there is no such
    /// item.
    pub fn item_author(&self, item: &ItemId) -> Option<&Author> {
        match item {
            ItemId::Thread(id) => self.thread(id).map(|thread| &thread.author),
            ItemId::Comment(id) => self
                .threads
                .iter()
                .find_map(|thread| thread.reply(id))
                .map(|reply| &reply.author),
        }
    }

    /// The list row for this review.
    pub fn summary(&self) -> ReviewSummary {
        let mut threads = ThreadCounts::default();
        for thread in self.live_threads() {
            match thread.state {
                ThreadState::Open => threads.open += 1,
                ThreadState::Addressed => threads.addressed += 1,
                ThreadState::Resolved => threads.resolved += 1,
            }
        }
        ReviewSummary {
            id: self.id.clone(),
            target: self.target.clone(),
            worktree: self.worktree.clone(),
            author: self.author.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            status: self.status,
            current_round: self.current_round(),
            threads,
            pr: self.pr.clone(),
            warnings: 0,
        }
    }
}

/// A review's row in `ReviewStore::list` (PLAN §7, §10.1 `dv review list`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSummary {
    pub id: ReviewId,
    pub target: Target,
    pub worktree: String,
    pub author: Author,
    pub created_at: u64,
    pub updated_at: u64,
    pub status: ReviewStatus,
    pub current_round: u32,
    /// Live (not deleted) threads by state.
    pub threads: ThreadCounts,
    pub pr: Option<PrLink>,
    /// Problems found in the log (`LoadedReview::warnings`); `0` from [`Review::summary`], which
    /// doesn't know them.
    pub warnings: usize,
}

/// Thread counts by state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadCounts {
    pub open: u32,
    pub addressed: u32,
    pub resolved: u32,
}
