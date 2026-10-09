//! Commands this version recognises but doesn't run (docs/PLAN.md §16): each is refused with
//! exit 2 and what to do instead.

use crate::error::CliError;

/// `dv review new --pr`, a PR target for `dv open` (PLAN §9).
pub(crate) fn github_prs() -> CliError {
    CliError::unsupported(
        "GitHub PRs arrive in a later version; review local work with `dv review new`",
    )
}

/// `dv setup` alone.
pub(crate) fn setup_what() -> CliError {
    CliError::usage(
        "say what to set up: `dv setup agents` or `dv setup wsl <distro>` (PATH setup comes in \
         a later version)",
    )
}

/// `dv setup path` (PLAN §12).
pub(crate) fn setup_path() -> CliError {
    CliError::unsupported(
        "`dv setup path` arrives in a later version; add dv's folder to PATH yourself for now",
    )
}

/// `dv setup all` (PLAN §12).
pub(crate) fn setup_all() -> CliError {
    CliError::unsupported(
        "`dv setup all` arrives in a later version; `dv setup agents` installs the skill and \
         `dv setup wsl <distro>` dv in a WSL distro",
    )
}

/// `dv doctor` (PLAN §12).
pub(crate) fn doctor() -> CliError {
    CliError::unsupported("`dv doctor` arrives in a later version")
}
