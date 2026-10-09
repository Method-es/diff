//! Who the reviewer is (PLAN §2.5): the GUI writes every event as them, and the CLI does when it
//! runs without an agent identity (PLAN §10).

use super::model::Author;
use crate::backend::{Backend, GitOpts};

/// The name events are written under when nobody is configured.
pub const FALLBACK_REVIEWER: &str = "reviewer";

/// The reviewer's name for events they write: their git identity in the backend's repository
/// (`user.name`), or [`FALLBACK_REVIEWER`] when none is configured (PLAN §2.5). Blocking.
pub fn reviewer_identity(backend: &dyn Backend) -> Author {
    let name = backend
        .git(
            &["config", "--get", "user.name"],
            &GitOpts::default().with_ok_codes(&[0, 1]),
        )
        .ok()
        .and_then(|output| output.stdout_utf8().map(|name| name.trim().to_string()))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| FALLBACK_REVIEWER.to_string());
    Author::reviewer(name)
}
