//! Where a review's target resolves: its home worktree (docs/PLAN.md §2.2, §2.4).
//!
//! Every worktree of a repository shares one review store (PLAN §7), but two kinds of target
//! mean different things in different worktrees: a `working` or `staged` review means the
//! changes of the worktree it was made in, and a `branch` review includes the files on disk
//! wherever that branch is checked out. That worktree is the review's **home**. git checks a
//! branch out in one worktree only, unless told otherwise (`git worktree add --force`); when
//! several have it, the home is the worktree asking, else the one the review was made in, else
//! the first git lists ([`Worktrees::home`]). The GUI and the CLI both resolve a review's target in its
//! home, whichever worktree they were started in, so a review lists the same files, places its
//! threads at the same lines and snapshots the same content from everywhere. A target without a
//! home (`commit`, `range`, `pr`, and a branch checked out nowhere, which is its tip) names the
//! same commits from every worktree.
//!
//! A `working` or `staged` review whose worktree is gone (removed, moved, or deleted and
//! waiting for `git worktree prune`) has no home: [`HomeError::Gone`], which the GUI shows as
//! a banner and the CLI as exit 1, rather than resolving the target in some other worktree.

use crate::git::{GitError, GitRepo, Worktree};
use crate::location::Location;
use crate::target::Target;

/// Where a review's target resolves (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Home {
    /// In this worktree: its root, as the review records it or as git lists it.
    Worktree(String),
    /// From any worktree alike.
    Anywhere,
}

impl Home {
    /// Whether this home is the worktree whose root is `root`, a worktree of the repository at
    /// `location` ([`same_worktree`]).
    pub fn is(&self, location: &Location, root: &str) -> bool {
        match self {
            Self::Worktree(home) => same_worktree(location, home, root),
            Self::Anywhere => false,
        }
    }
}

/// Why a review has no home to resolve in.
#[derive(Debug, thiserror::Error)]
pub enum HomeError {
    #[error(transparent)]
    Git(#[from] GitError),
    /// The worktree a `working` or `staged` review belongs to is no longer one of the
    /// repository's.
    #[error(
        "its worktree {worktree} is no longer one of the repository's (removed, moved or deleted)"
    )]
    Gone { worktree: String },
}

/// A repository's live worktrees (`git worktree list`, less the bare repository and the
/// worktrees whose directory is gone), and the branch checked out in the worktree that read
/// them, read once to find the homes of several reviews.
#[derive(Debug, Clone, Default)]
pub struct Worktrees {
    live: Vec<Worktree>,
    /// The branch checked out in the worktree that read the list; `None` when `HEAD` is
    /// detached.
    here: Option<String>,
}

impl Worktrees {
    /// Reads them from `repo` (blocking: two git calls). In a repository whose git dir is kept
    /// apart from its files (`git init --separate-git-dir`, a submodule), git lists the main
    /// worktree by its git dir, which holds no files; that entry is left out too.
    pub fn read(repo: &GitRepo) -> Result<Self, GitError> {
        let (location, common_dir) = (&repo.info().location, &repo.info().common_dir);
        let live = repo
            .worktrees()?
            .into_iter()
            .filter(|worktree| {
                !worktree.bare
                    && !worktree.prunable
                    && !same_worktree(location, &worktree.path, common_dir)
            })
            .collect();
        Ok(Self {
            live,
            here: repo.current_branch()?,
        })
    }

    /// The home of a review of `target` made in `worktree` (its root as the review records
    /// it), read through `repo`, a worktree of the same repository (blocking: a `branch`
    /// target looks up the ref its name stands for).
    pub fn home(&self, repo: &GitRepo, target: &Target, worktree: &str) -> Result<Home, HomeError> {
        let location = &repo.info().location;
        match target {
            Target::Working | Target::Staged => {
                if same_worktree(location, worktree, &repo.info().toplevel)
                    || self
                        .live
                        .iter()
                        .any(|live| same_worktree(location, &live.path, worktree))
                {
                    Ok(Home::Worktree(worktree.to_string()))
                } else {
                    Err(HomeError::Gone {
                        worktree: worktree.to_string(),
                    })
                }
            }
            Target::Branch { branch, .. } => {
                // The one ref the name stands for, as resolving the target follows it (PLAN
                // §2.2). A branch that no longer exists is checked out nowhere; resolving the
                // target then says what is wrong.
                let full = match repo.full_ref_name(branch) {
                    Ok(full) => full,
                    Err(GitError::UnknownRevision { .. }) => None,
                    Err(error) => return Err(error.into()),
                };
                let Some(local) = full
                    .as_deref()
                    .and_then(|full| full.strip_prefix("refs/heads/"))
                else {
                    return Ok(Home::Anywhere);
                };
                // The worktree asking, then the one the review was made in, then the first git
                // lists: a branch checked out in several (`--force`) is homed where the review
                // is being worked on.
                if self.here.as_deref() == Some(local) {
                    return Ok(Home::Worktree(repo.info().toplevel.clone()));
                }
                let has_it = |live: &&Worktree| live.branch.as_deref() == Some(local);
                let checked_out = self
                    .live
                    .iter()
                    .filter(has_it)
                    .find(|live| same_worktree(location, &live.path, worktree))
                    .or_else(|| self.live.iter().find(has_it));
                Ok(checked_out.map_or(Home::Anywhere, |live| Home::Worktree(live.path.clone())))
            }
            Target::Commit { .. } | Target::Range { .. } | Target::Pr { .. } => Ok(Home::Anywhere),
        }
    }
}

/// The home of a review of `target` made in `worktree`, read through `repo` (blocking:
/// [`Worktrees::read`] when it takes one, then [`Worktrees::home`]).
pub fn home(repo: &GitRepo, target: &Target, worktree: &str) -> Result<Home, HomeError> {
    let needs_list = match target {
        Target::Working | Target::Staged => {
            !same_worktree(&repo.info().location, worktree, &repo.info().toplevel)
        }
        Target::Branch { .. } => true,
        Target::Commit { .. } | Target::Range { .. } | Target::Pr { .. } => false,
    };
    let worktrees = if needs_list {
        Worktrees::read(repo)?
    } else {
        Worktrees::default()
    };
    worktrees.home(repo, target, worktree)
}

/// Whether two spellings of a worktree root of the repository at `location` name the same
/// folder: `\` and `/` alike, a trailing separator ignored, and case too where the repository's
/// file system ignores it ([`Location::folds_case`]: a local repository on Windows or macOS,
/// never a WSL one, whose roots the Windows app compares as the distro spells them).
pub fn same_worktree(location: &Location, a: &str, b: &str) -> bool {
    same_root(location.folds_case(), a, b)
}

/// [`same_worktree`] with the repository's case rule given: `fold_case` ignores ASCII case.
pub(crate) fn same_root(fold_case: bool, a: &str, b: &str) -> bool {
    let norm = |path: &str| path.replace('\\', "/").trim_end_matches('/').to_string();
    let (a, b) = (norm(a), norm(b));
    if fold_case {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}
