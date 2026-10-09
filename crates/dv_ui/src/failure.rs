//! Errors as views show them (DESIGN §4.4): a one-line headline from the error's `Display`, and
//! the command that failed with what it printed for the disclosure and "Copy diagnostics".
//! Never raw stderr as the headline.

use std::fmt;

use dv_core::diff::DiffError;
use dv_core::git::GitError;
use dv_core::review::{OpError, SnapshotError, StoreError};
use dv_core::{BackendError, TargetError};

/// An error ready for a banner: cheap to clone, so one failure can sit in state and in a view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// One line, fit for a headline.
    pub message: String,
    /// The failed command and its output, when there is more to show.
    pub detail: Option<String>,
}

impl Failure {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            detail: None,
        }
    }

    fn with_backend(message: String, backend: Option<&BackendError>) -> Self {
        Self {
            message,
            detail: backend.and_then(BackendError::detail),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<&BackendError> for Failure {
    fn from(error: &BackendError) -> Self {
        Self::with_backend(error.to_string(), Some(error))
    }
}

impl From<&GitError> for Failure {
    fn from(error: &GitError) -> Self {
        Self::with_backend(error.to_string(), git_backend(error))
    }
}

impl From<&DiffError> for Failure {
    fn from(error: &DiffError) -> Self {
        let backend = match error {
            DiffError::Git(git) => git_backend(git),
            _ => None,
        };
        Self::with_backend(error.to_string(), backend)
    }
}

impl From<&TargetError> for Failure {
    fn from(error: &TargetError) -> Self {
        let backend = match error {
            TargetError::Git(git) => git_backend(git),
            _ => None,
        };
        Self::with_backend(error.to_string(), backend)
    }
}

impl From<&StoreError> for Failure {
    fn from(error: &StoreError) -> Self {
        let backend = match error {
            StoreError::Backend(backend) => Some(backend),
            _ => None,
        };
        Self::with_backend(error.to_string(), backend)
    }
}

impl From<&OpError> for Failure {
    fn from(error: &OpError) -> Self {
        let message = error.to_string();
        match error {
            OpError::Store(store) => Self {
                message,
                ..Self::from(store)
            },
            OpError::Target(target) => Self {
                message,
                ..Self::from(target)
            },
            OpError::Git(git) => Self {
                message,
                ..Self::from(git)
            },
            OpError::Snapshot(SnapshotError::Backend(backend)) => {
                Self::with_backend(message, Some(backend))
            }
            OpError::Snapshot(
                SnapshotError::Unmerged { detail } | SnapshotError::Unreadable { detail, .. },
            ) => Self {
                message,
                detail: Some(detail.clone()),
            },
            _ => Self::new(message),
        }
    }
}

fn git_backend(error: &GitError) -> Option<&BackendError> {
    match error {
        GitError::Backend(backend) => Some(backend),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_headline_is_one_line_and_git_output_goes_to_the_detail() {
        let git = BackendError::Git {
            args: vec!["diff".into(), "--raw".into()],
            code: Some(128),
            stderr: "fatal: bad object HEAD\nmore\n".into(),
        };
        let failure = Failure::from(&DiffError::Git(GitError::Backend(git)));
        assert_eq!(failure.message, "git diff failed (exit code 128)");
        let detail = failure.detail.expect("the command and its stderr");
        assert!(detail.contains("$ git diff --raw"), "{detail}");
        assert!(detail.contains("fatal: bad object HEAD"), "{detail}");

        let plain = Failure::from(&TargetError::NoDefaultBase {
            branch: "feat".into(),
        });
        assert!(!plain.message.contains('\n'));
        assert_eq!(plain.detail, None);
    }
}
