//! Exit codes and errors (docs/PLAN.md §10.3).
//!
//! Every failure a command reports is a [`CliError`]: a snake_case `code` agents can branch on,
//! a one-line message, the exit code, optional diagnostics (the git command that failed and its
//! stderr) and, for a command that couldn't tell which review it means, the candidates
//! (PLAN §10.2). [`crate::output::error`] prints it on stderr and, with `--json`, as
//! `{"schema": 1, "error": {…}}` on stdout.

use dv_core::backend::BackendError;
use dv_core::diff::DiffError;
use dv_core::git::GitError;
use dv_core::review::{OpError, StoreError};
use dv_core::target::TargetError;

use crate::dto::CandidateDto;

/// The process exit code of a command (PLAN §10.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Exit {
    /// Done.
    Ok,
    /// The operation failed: git, the store, a refused change.
    Failure,
    /// The command line is wrong, or it doesn't say which review it means.
    Usage,
    /// `dv review wait --timeout` ran out.
    Timeout,
    /// The repository's backend can't run here (a WSL repository, for now).
    Unavailable,
    /// GitHub (through `gh`) failed.
    GitHub,
}

impl Exit {
    /// The number the process exits with.
    pub fn code(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Failure => 1,
            Self::Usage => 2,
            Self::Timeout => 3,
            Self::Unavailable => 4,
            Self::GitHub => 5,
        }
    }
}

impl From<Exit> for std::process::ExitCode {
    fn from(exit: Exit) -> Self {
        Self::from(exit.code())
    }
}

/// The error codes a JSON error carries (`error.code`), snake_case and stable.
pub mod code {
    /// The command line is wrong.
    pub const USAGE: &str = "usage";
    /// Recognised, but not in this version (`--pr`, `doctor`, …).
    pub const UNSUPPORTED: &str = "unsupported";
    /// Several open reviews match the current work; `error.candidates` lists them.
    pub const AMBIGUOUS_REVIEW: &str = "ambiguous_review";
    /// No open review matches the current work; `error.candidates` lists the open ones.
    pub const NO_REVIEW: &str = "no_review";
    /// The review or thread named doesn't exist.
    pub const NOT_FOUND: &str = "not_found";
    /// The path isn't inside a git working tree.
    pub const NOT_A_REPOSITORY: &str = "not_a_repository";
    /// The change is valid, but not for this author (an agent resolving a thread).
    pub const NOT_ALLOWED: &str = "not_allowed";
    /// The change doesn't apply to the review as it stands.
    pub const REJECTED: &str = "rejected";
    /// The review is approved, closed or submitted.
    pub const FINISHED: &str = "finished";
    /// `dv review done` would hand back a round the reviewer finished while the agent was
    /// working: read it, then `dv review done <id> --round <n>`.
    pub const UNSEEN_ROUND: &str = "unseen_round";
    /// The person at the terminal said no (`dv setup agents`).
    pub const CANCELLED: &str = "cancelled";
    /// The repository's backend can't run here (a WSL repository).
    pub const BACKEND_UNAVAILABLE: &str = "backend_unavailable";
    /// The review's target no longer resolves (a deleted branch), or a round's snapshot is
    /// gone.
    pub const TARGET: &str = "target";
    /// The review's log can't be read.
    pub const CORRUPT: &str = "corrupt";
    /// A git command failed; `error.detail` has the command and its stderr.
    pub const GIT: &str = "git";
    /// Reading or writing a file failed.
    pub const IO: &str = "io";
    /// Anything else that went wrong inside dv.
    pub const INTERNAL: &str = "internal";
}

/// A command's failure (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub exit: Exit,
    /// One of [`code`]'s.
    pub code: &'static str,
    /// One line saying what went wrong.
    pub message: String,
    /// Diagnostics behind the message: the git command line, its exit code and stderr.
    pub detail: Option<String>,
    /// The reviews the command could have meant (PLAN §10.2).
    pub candidates: Option<Vec<CandidateDto>>,
}

impl CliError {
    pub fn new(exit: Exit, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            exit,
            code,
            message: message.into(),
            detail: None,
            candidates: None,
        }
    }

    /// A wrong command line: exit 2.
    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(Exit::Usage, code::USAGE, message)
    }

    /// A command recognised but not in this version: exit 2.
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(Exit::Usage, code::UNSUPPORTED, message)
    }

    /// No such review or thread: exit 1.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(Exit::Failure, code::NOT_FOUND, message)
    }

    /// The repository's backend can't run here: exit 4.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(Exit::Unavailable, code::BACKEND_UNAVAILABLE, message)
    }

    /// An existing review's target that no longer resolves (a deleted branch): exit 1,
    /// `target` (PLAN §10.1), with git's own words. Git that couldn't run at all is no fault of
    /// the target, and keeps its own code.
    pub fn target(error: TargetError) -> Self {
        let detail = match &error {
            TargetError::Git(GitError::Backend(backend)) => match backend {
                BackendError::Git { .. } => backend.detail(),
                _ => return error.into(),
            },
            _ => None,
        };
        Self::new(
            Exit::Failure,
            code::TARGET,
            format!("the review's target no longer resolves: {error}"),
        )
        .with_detail(detail)
    }

    pub fn with_detail(mut self, detail: Option<String>) -> Self {
        self.detail = detail;
        self
    }

    pub fn with_candidates(mut self, candidates: Vec<CandidateDto>) -> Self {
        self.candidates = Some(candidates);
        self
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

impl From<BackendError> for CliError {
    fn from(error: BackendError) -> Self {
        let message = error.to_string();
        let detail = error.detail();
        let (exit, code) = match &error {
            BackendError::Unavailable { .. } => (Exit::Unavailable, code::BACKEND_UNAVAILABLE),
            BackendError::PathNotFound { .. }
            | BackendError::NotARepository { .. }
            | BackendError::BareRepository { .. }
            | BackendError::InsideGitDir { .. }
            | BackendError::UnsafeRepository { .. } => (Exit::Failure, code::NOT_A_REPOSITORY),
            BackendError::Spawn { .. }
            | BackendError::Git { .. }
            | BackendError::Timeout { .. }
            | BackendError::UnexpectedOutput { .. } => (Exit::Failure, code::GIT),
            BackendError::Io { .. } | BackendError::Watch { .. } => (Exit::Failure, code::IO),
            BackendError::InvalidPath { .. } | BackendError::InvalidLine { .. } => {
                (Exit::Failure, code::INTERNAL)
            }
        };
        Self::new(exit, code, message).with_detail(detail)
    }
}

impl From<GitError> for CliError {
    fn from(error: GitError) -> Self {
        match error {
            GitError::Backend(error) => error.into(),
            other => Self::new(Exit::Failure, code::GIT, other.to_string()),
        }
    }
}

impl From<TargetError> for CliError {
    fn from(error: TargetError) -> Self {
        match error {
            TargetError::Git(error) => error.into(),
            other => Self::new(Exit::Failure, code::TARGET, other.to_string()),
        }
    }
}

impl From<DiffError> for CliError {
    fn from(error: DiffError) -> Self {
        match error {
            DiffError::Git(error) => error.into(),
            other => Self::new(Exit::Failure, code::GIT, other.to_string()),
        }
    }
}

impl From<StoreError> for CliError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Backend(error) => error.into(),
            StoreError::UnknownReview { .. } => Self::not_found(error.to_string()),
            StoreError::Corrupt { .. } => {
                Self::new(Exit::Failure, code::CORRUPT, error.to_string())
            }
            other => Self::new(Exit::Failure, code::INTERNAL, other.to_string()),
        }
    }
}

impl From<OpError> for CliError {
    fn from(error: OpError) -> Self {
        match error {
            OpError::Store(error) => error.into(),
            OpError::Target(error) => Self::target(error),
            OpError::Git(error) => error.into(),
            OpError::Rejected(_) => Self::new(Exit::Failure, code::REJECTED, error.to_string()),
            OpError::NotAllowed { .. } => {
                Self::new(Exit::Failure, code::NOT_ALLOWED, error.to_string())
            }
            OpError::Empty { .. } => Self::usage(error.to_string()),
            OpError::Finished { .. } => Self::new(Exit::Failure, code::FINISHED, error.to_string()),
            OpError::UnseenRound { .. } => {
                Self::new(Exit::Failure, code::UNSEEN_ROUND, error.to_string())
            }
            OpError::NotLastRound { .. } => Self::usage(error.to_string()),
            OpError::Snapshot(_) => Self::new(Exit::Failure, code::GIT, error.to_string()),
        }
    }
}
