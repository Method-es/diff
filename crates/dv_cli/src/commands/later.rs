//! Commands this version recognises but doesn't run (docs/PLAN.md §16): each is refused with
//! exit 2 and what to do instead, except WSL setup, which is the backend being unavailable
//! (exit 4).

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
        "say what to set up: `dv setup agents` (PATH and WSL setup come in a later version)",
    )
}

/// `dv setup path` (PLAN §12).
pub(crate) fn setup_path() -> CliError {
    CliError::unsupported(
        "`dv setup path` arrives in a later version; add dv's folder to PATH yourself for now",
    )
}

/// `dv setup wsl <distro>` (PLAN §5.2).
pub(crate) fn setup_wsl(distro: &str) -> CliError {
    CliError::unavailable(format!(
        "setting dv up in the WSL distro {distro} arrives in a later version"
    ))
}

/// `dv setup all` (PLAN §12).
pub(crate) fn setup_all() -> CliError {
    CliError::unsupported(
        "`dv setup all` arrives in a later version; `dv setup agents` installs the skill",
    )
}

/// `dv doctor` (PLAN §12).
pub(crate) fn doctor() -> CliError {
    CliError::unsupported("`dv doctor` arrives in a later version")
}

/// `dv backend --stdio`, the Linux build's server for the Windows app (PLAN D6).
pub(crate) fn backend() -> CliError {
    CliError::unsupported(
        "`dv backend` arrives in a later version (the Linux build serves WSL repositories)",
    )
}
