//! Helpers shared by every dv crate's tests, chiefly [`FixtureRepo`]: real git repos built in temp
//! dirs through the `git` CLI, so tests exercise git's own behaviour (docs/PLAN.md §14.2).
//!
//! gpui: **never**. Headless crates' tests depend on it, and `script/check` fails when gpui
//! appears in its dependency tree (PLAN §4, §13.2).
//!
//! These are test helpers: a fixture that can't be built is a broken test, so every method panics
//! with a message naming the step that failed (and git's stderr) instead of returning an error.

mod fixture_repo;

pub use fixture_repo::{AUTHOR_EMAIL, AUTHOR_NAME, EPOCH, FixtureRepo, TICK_SECS};
