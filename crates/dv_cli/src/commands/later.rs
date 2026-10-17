//! Commands this version recognises but doesn't run (docs/PLAN.md §16): each is refused with
//! exit 2 and what to do instead.

use crate::error::CliError;

/// `dv review new --pr`, a PR target for `dv open` (PLAN §9).
pub(crate) fn github_prs() -> CliError {
    CliError::unsupported(
        "GitHub PRs arrive in a later version; review local work with `dv review new`",
    )
}
