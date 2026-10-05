//! Reviews as append-only event logs (docs/PLAN.md D5, §2, §7).
//!
//! - [`id`]: `r-`/`t-`/`c-`/`e-` ULID ids, strictly validated before any path is built.
//! - [`event`]: the event enum, one JSON line each, and the forgiving log reader.
//! - [`model`]: the folded state — target, status, rounds, viewed files, threads, PR link.
//! - [`fold`]: events → state, with the status and thread-state rules (PLAN §2.4, §2.5).
//! - [`store`]: the logs, their snapshot cache and refs, over a [`crate::Backend`].
//! - [`snapshot`]: a round's new side kept as a commit under `refs/dv/` (PLAN §2.3).
//! - [`anchor`]: anchors built from a selection of a side's lines (PLAN §2.5).
//! - [`ops`]: checked changes — what the UI and the CLI call.
//!
//! Many writers (the GUI, the CLI, any number of agents, in Windows and inside WSL) append to
//! one log at once without a lock: each event is one line written in one append (PLAN D5), and
//! every reader folds the lines in file order, so no update is ever lost.

pub mod anchor;
pub mod event;
pub mod fold;
pub mod id;
pub mod model;
pub mod ops;
pub mod snapshot;
pub mod store;

pub use anchor::{ANCHOR_CONTEXT, AnchorError, AnchorPlace, anchor_selection, side_lines};
pub use event::{EVENT_VERSION, Event, EventBody, LogWarning, ParsedLog, parse_log};
pub use fold::{FoldError, Folded, Outcome, Rejection, fold};
pub use id::{CommentId, EventId, InvalidId, ItemId, ReviewId, ThreadId};
pub use model::{
    Anchor, AnchorSide, Author, AuthorKind, Comment, PrLink, Review, ReviewStatus, ReviewSummary,
    Round, StateChange, Submission, Thread, ThreadCounts, ThreadState, Verdict,
};
pub use ops::{FinishedRound, OpError, Opened};
pub use snapshot::SnapshotError;
pub use store::{ListEntry, LoadedReview, ReviewStore, StoreError};

#[cfg(test)]
mod tests;
