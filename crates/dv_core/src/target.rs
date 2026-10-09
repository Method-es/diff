//! Review targets: what a review compares, and how a target resolves to the two sides git diffs
//! (docs/PLAN.md §2.2).
//!
//! A [`Target`] is the spec a review is created with and keeps for life (PLAN D3). Resolving it
//! yields a [`DiffSpec`]: two [`Side`]s, each a resolved commit id, the empty tree, the index or
//! the working tree. A review records the resolved ids of every round alongside the spec, so a
//! symbolic name (`main`, `HEAD~2`) never silently changes what an old round meant (PLAN §2.2).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::git::{GitError, GitRepo, Oid};
use crate::review::Round;

/// What a review compares (PLAN §2.2).
///
/// Serialised externally tagged in snake_case — `"working"`, `{"branch": {"branch": "feat",
/// "base": null}}`, `{"range": {"from": "v1", "to": "v2", "three_dot": true}}` — a stable shape,
/// because reviews store their target in their event log (PLAN §7).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// `HEAD` against the working tree, untracked (not ignored) files included.
    Working,
    /// `HEAD` against the index.
    Staged,
    /// A branch's work since it left `base`: `merge-base(base, branch)` against the branch tip,
    /// or against the working tree while the branch is checked out in the worktree it resolves
    /// in (a review's home, `review::home`, is that worktree when there is one).
    Branch {
        branch: String,
        /// The branch to compare against; `None` means the repo's default branch
        /// ([`GitRepo::default_branch`]).
        base: Option<String>,
    },
    /// One commit against its first parent (the empty tree for a root commit).
    Commit { rev: String },
    /// `from..to` (two-dot: `from` against `to`) or `from...to` (three-dot:
    /// `merge-base(from, to)` against `to`).
    Range {
        from: String,
        to: String,
        three_dot: bool,
    },
    /// A GitHub pull request, from the base and head commits it was fetched at:
    /// `merge-base(base, head)` against `head`. Fetching is the GitHub layer's job (PLAN §9.1);
    /// resolution only checks that both commits are present.
    Pr {
        owner: String,
        repo: String,
        number: u64,
        base_oid: Oid,
        head_oid: Oid,
    },
}

/// One side of a diff, resolved.
///
/// Serialised externally tagged in snake_case: `{"commit": "<oid>"}`, `"empty_tree"`, `"index"`,
/// `"worktree"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// A commit's tree.
    Commit(Oid),
    /// The empty tree: the old side of a root commit or an unborn `HEAD`.
    EmptyTree,
    /// The index (staging area).
    Index,
    /// The working tree, untracked files included.
    Worktree,
}

impl Side {
    /// Whether this side is a tree (a commit's or the empty one), as opposed to the index or the
    /// working tree.
    pub fn is_tree(&self) -> bool {
        matches!(self, Side::Commit(_) | Side::EmptyTree)
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Side::Commit(oid) => f.write_str(oid.as_str()),
            Side::EmptyTree => f.write_str("the empty tree"),
            Side::Index => f.write_str("the index"),
            Side::Worktree => f.write_str("the working tree"),
        }
    }
}

/// A resolved target: the two sides the diff pipeline compares (PLAN §6).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiffSpec {
    pub old: Side,
    pub new: Side,
}

impl DiffSpec {
    /// The interdiff since `round` (PLAN §2.3, §6 step 6): the round's snapshot — the new side
    /// as it stood when the round closed — against this spec's new side, so only what changed
    /// after the round shows.
    pub fn since(&self, round: &Round) -> DiffSpec {
        DiffSpec {
            old: Side::Commit(round.new_oid.clone()),
            new: self.new.clone(),
        }
    }

    /// [`DiffSpec::since`], once the round's snapshot commit is known to be in `repo`;
    /// [`TargetError::MissingSnapshot`] when it isn't (the GUI then says so and shows all
    /// changes).
    pub fn since_checked(&self, repo: &GitRepo, round: &Round) -> Result<DiffSpec, TargetError> {
        if !repo.has_commit(&round.new_oid)? {
            return Err(TargetError::MissingSnapshot {
                round: round.n,
                oid: round.new_oid.clone(),
            });
        }
        Ok(self.since(round))
    }
}

/// Why a target didn't resolve.
#[derive(Debug, thiserror::Error)]
pub enum TargetError {
    #[error(transparent)]
    Git(#[from] GitError),
    /// The two revisions share no history, so there is no merge base to compare from.
    #[error("{a} and {b} have no common history")]
    NoMergeBase { a: String, b: String },
    /// A `branch` target without a base, in a repo with no `origin/HEAD`, `main` or `master`.
    #[error("no default branch to compare {branch} with (no origin/HEAD, main or master)")]
    NoDefaultBase { branch: String },
    /// A round's snapshot commit isn't in the repository, so nothing can be compared with it.
    #[error("round {round}'s snapshot ({}) is missing from the repository", oid.short())]
    MissingSnapshot { round: u32, oid: Oid },
}

impl Target {
    /// Resolves the target in `repo` (PLAN §2.2): every revision becomes a commit id, so the
    /// returned spec means the same thing however the repo moves on afterwards (except the index
    /// and the working tree, which are live by nature). The index and the working tree are
    /// `repo`'s worktree's, so a review's target is resolved in its home worktree
    /// (`review::home`), whichever worktree reads it.
    pub fn resolve(&self, repo: &GitRepo) -> Result<DiffSpec, TargetError> {
        match self {
            Target::Working => Ok(DiffSpec {
                old: head_side(repo)?,
                new: Side::Worktree,
            }),
            Target::Staged => Ok(DiffSpec {
                old: head_side(repo)?,
                new: Side::Index,
            }),
            Target::Branch { branch, base } => {
                let base = match base {
                    Some(base) => base.clone(),
                    None => repo
                        .default_branch()?
                        .ok_or_else(|| TargetError::NoDefaultBase {
                            branch: branch.clone(),
                        })?,
                };
                // The tip and the checked-out test both follow the one ref the name stands for,
                // however it is spelled (`feat`, `heads/feat`, `refs/heads/feat`), so a tag of
                // the same name can't supply the tip of a branch whose working tree is shown.
                let full = repo.full_ref_name(branch)?;
                let tip = repo.resolve(full.as_deref().unwrap_or(branch))?;
                let old = merge_base(repo, (&base, &base), (tip.as_str(), branch))?;
                let checked_out = match (full.as_deref(), repo.current_branch()?) {
                    (Some(full), Some(current)) => {
                        full.strip_prefix("refs/heads/") == Some(current.as_str())
                    }
                    _ => false,
                };
                Ok(DiffSpec {
                    old: Side::Commit(old),
                    new: if checked_out {
                        Side::Worktree
                    } else {
                        Side::Commit(tip)
                    },
                })
            }
            Target::Commit { rev } => {
                let commit = repo.resolve(rev)?;
                let old = match repo.parents(&commit)?.into_iter().next() {
                    Some(parent) => Side::Commit(parent),
                    None => Side::EmptyTree,
                };
                Ok(DiffSpec {
                    old,
                    new: Side::Commit(commit),
                })
            }
            Target::Range {
                from,
                to,
                three_dot,
            } => {
                let to_oid = repo.resolve(to)?;
                let old = if *three_dot {
                    merge_base(repo, (from, from), (to_oid.as_str(), to))?
                } else {
                    repo.resolve(from)?
                };
                Ok(DiffSpec {
                    old: Side::Commit(old),
                    new: Side::Commit(to_oid),
                })
            }
            Target::Pr {
                base_oid, head_oid, ..
            } => {
                // `resolve` checks that each commit is present (it was fetched).
                let base = repo.resolve(base_oid.as_str())?;
                let head = repo.resolve(head_oid.as_str())?;
                let old = merge_base(
                    repo,
                    (base.as_str(), base.as_str()),
                    (head.as_str(), head.as_str()),
                )?;
                Ok(DiffSpec {
                    old: Side::Commit(old),
                    new: Side::Commit(head),
                })
            }
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Working => f.write_str("working tree"),
            Target::Staged => f.write_str("staged changes"),
            Target::Branch { branch, base: None } => write!(f, "branch {branch}"),
            Target::Branch {
                branch,
                base: Some(base),
            } => write!(f, "branch {branch} vs {base}"),
            Target::Commit { rev } => write!(f, "commit {rev}"),
            Target::Range {
                from,
                to,
                three_dot,
            } => write!(f, "{from}{}{to}", if *three_dot { "..." } else { ".." }),
            Target::Pr {
                owner,
                repo,
                number,
                ..
            } => write!(f, "{owner}/{repo}#{number}"),
        }
    }
}

/// `HEAD`'s commit, or the empty tree on an unborn branch (a repo without commits).
fn head_side(repo: &GitRepo) -> Result<Side, TargetError> {
    Ok(match repo.head()? {
        Some(head) => Side::Commit(head),
        None => Side::EmptyTree,
    })
}

/// The merge base of two revisions, which must exist. Each is a `(revision, label)` pair: the
/// revision git looks up (often an already resolved id) and the name an error shows.
fn merge_base(repo: &GitRepo, a: (&str, &str), b: (&str, &str)) -> Result<Oid, TargetError> {
    repo.merge_base(a.0, b.0)?
        .ok_or_else(|| TargetError::NoMergeBase {
            a: a.1.to_string(),
            b: b.1.to_string(),
        })
}

#[cfg(test)]
mod tests;
