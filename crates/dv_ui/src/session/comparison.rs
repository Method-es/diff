//! What the stream compares (PLAN §2.3, DESIGN §4.1): a review's round n+1 shows the interdiff
//! *round n → now* by default, with a toggle to all changes.
//!
//! The comparison is session state, never persisted. A review opens on *since the last closed
//! round* when it has one, isn't finished and its target is live (`working`, `staged`, a
//! branch: [`comparison_available`]); otherwise on all changes. A fixed target (`commit`,
//! `range`) never offers the toggle, since its interdiff is always empty, and neither does a PR
//! review (its rounds come with GitHub, PLAN §2.3). Closing a round moves the comparison to
//! *since* that round ([`default_comparison`]).

use dv_core::Target;
use dv_core::review::Review;

/// What the stream compares: the interdiff since closed round `n` (that round's snapshot against
/// the target's new side as it is now, `DiffSpec::since`), or all of the target's changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Comparison {
    SinceRound(u32),
    All,
}

impl Comparison {
    /// `"since_round"` or `"all"`, as automation state spells it.
    pub fn key(self) -> &'static str {
        match self {
            Self::SinceRound(_) => "since_round",
            Self::All => "all",
        }
    }

    /// The round an interdiff starts from.
    pub fn round(self) -> Option<u32> {
        match self {
            Self::SinceRound(n) => Some(n),
            Self::All => None,
        }
    }

    pub fn is_interdiff(self) -> bool {
        matches!(self, Self::SinceRound(_))
    }
}

/// Whether `review` offers the "Since round n | All changes" toggle: its target is live, it
/// isn't finished, and it has closed a round.
pub fn comparison_available(review: &Review) -> bool {
    let live = matches!(
        review.target,
        Target::Working | Target::Staged | Target::Branch { .. }
    );
    live && !review.status.is_final() && review.last_round().is_some()
}

/// The comparison `review` opens on, and the one it moves to once a round closes: since its
/// last closed round when the toggle applies, else all changes.
pub fn default_comparison(review: &Review) -> Comparison {
    match review.last_round() {
        Some(round) if comparison_available(review) => Comparison::SinceRound(round.n),
        _ => Comparison::All,
    }
}
