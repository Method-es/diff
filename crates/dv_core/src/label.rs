//! How targets are named (DESIGN §4.1): the title bar's and the switcher's "working tree vs
//! main", the stream's side labels (`main` / `working tree`), and the CLI's review labels
//! (PLAN §10), all from one place so the GUI and the CLI agree.

use crate::git::GitRepo;
use crate::review::home::same_worktree as same_root;
use crate::target::Target;

/// What a target's label needs to know about the repository: the branch checked out (or a
/// short commit id when `HEAD` is detached) and the default branch a `branch` target without a
/// base compares against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LabelContext {
    pub head: Option<String>,
    /// The branch checked out, `None` when `HEAD` is detached: what a `branch` target of the
    /// work in progress names (the switcher's and the source picker's "Branch" row).
    pub branch: Option<String>,
    pub default_base: Option<String>,
}

impl LabelContext {
    /// Reads them from `repo` (blocking). A part that can't be read stays `None`.
    pub fn read(repo: &GitRepo) -> Self {
        let branch = repo.current_branch().ok().flatten();
        let head = branch.clone().or_else(|| {
            repo.head()
                .ok()
                .flatten()
                .map(|oid| oid.short().to_string())
        });
        Self {
            head,
            branch,
            default_base: repo.default_branch().ok().flatten(),
        }
    }

    /// The `branch` target of the work checked out, compared with the default branch: `None`
    /// when `HEAD` is detached, nothing is known to compare with, or the branch is the default
    /// branch itself (nothing to compare).
    pub fn branch_target(&self) -> Option<Target> {
        let (branch, base) = (self.branch.as_ref()?, self.default_base.as_ref()?);
        (branch != base).then(|| Target::Branch {
            branch: branch.clone(),
            base: None,
        })
    }
}

/// What targets are labelled with across a repository's worktrees: a `working` or `staged`
/// review means the changes of the worktree it was made in (PLAN §2.4), so it is named by that
/// worktree's `HEAD` ("working tree vs feature"), whichever worktree reads it; every other
/// target by the worktree read from ([`Labels::here`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Labels {
    /// The worktree read from.
    pub here: LabelContext,
    /// Its root, as reviews record it (`ReviewSummary::worktree`).
    root: String,
    /// The other worktrees' roots and contexts.
    others: Vec<(String, LabelContext)>,
    /// For a worktree that is gone: no `HEAD` to compare with.
    gone: LabelContext,
}

impl Labels {
    /// Reads them from `repo` (blocking: [`LabelContext::read`] and `git worktree list`). A
    /// part that can't be read stays unknown.
    pub fn read(repo: &GitRepo) -> Self {
        let here = LabelContext::read(repo);
        let root = repo.info().toplevel.clone();
        let others = repo
            .worktrees()
            .unwrap_or_default()
            .into_iter()
            .filter(|worktree| !same_root(&worktree.path, &root))
            .map(|worktree| {
                let head = worktree
                    .branch
                    .clone()
                    .or_else(|| worktree.head.as_ref().map(|oid| oid.short().to_string()));
                let context = LabelContext {
                    head,
                    branch: worktree.branch,
                    default_base: here.default_base.clone(),
                };
                (worktree.path, context)
            })
            .collect();
        let gone = LabelContext {
            default_base: here.default_base.clone(),
            ..LabelContext::default()
        };
        Self {
            here,
            root,
            others,
            gone,
        }
    }

    /// Labels for one worktree only, `here` (tests, and callers with no other worktree to
    /// tell apart).
    pub fn only(here: LabelContext) -> Self {
        Self {
            gone: LabelContext {
                default_base: here.default_base.clone(),
                ..LabelContext::default()
            },
            here,
            ..Self::default()
        }
    }

    /// The context a review of `target` made in `worktree` (its root, as the review records
    /// it) is labelled with: that worktree's for a `working` or `staged` target, else
    /// [`Labels::here`].
    pub fn of_review(&self, target: &Target, worktree: &str) -> &LabelContext {
        let bound = matches!(target, Target::Working | Target::Staged);
        if !bound || self.root.is_empty() || same_root(worktree, &self.root) {
            return &self.here;
        }
        self.others
            .iter()
            .find(|(root, _)| same_root(root, worktree))
            .map_or(&self.gone, |(_, context)| context)
    }

    /// [`target_label`] of a review of `target` made in `worktree`.
    pub fn label(&self, target: &Target, worktree: &str) -> String {
        target_label(target, self.of_review(target, worktree))
    }
}

/// A target as the title bar and the switcher name it (DESIGN §4.1: "working tree vs main"):
/// what is being reviewed and what it is compared against.
pub fn target_label(target: &Target, context: &LabelContext) -> String {
    let versus = |what: &str| match &context.head {
        Some(head) => format!("{what} vs {head}"),
        None => what.to_string(),
    };
    match target {
        Target::Working => versus("working tree"),
        Target::Staged => versus("staged"),
        Target::Branch { branch, base } => match base.as_ref().or(context.default_base.as_ref()) {
            Some(base) => format!("{branch} vs {base}"),
            None => branch.clone(),
        },
        Target::Commit { rev } => format!("commit {rev}"),
        Target::Range {
            from,
            to,
            three_dot,
        } => format!("{from}{}{to}", if *three_dot { "..." } else { ".." }),
        Target::Pr {
            owner,
            repo,
            number,
            ..
        } => format!("{owner}/{repo}#{number}"),
    }
}

/// What the stream's side labels call a target's two sides (DESIGN §4.1: `main` / `working
/// tree`): the old side, then the new.
pub fn side_labels(target: &Target, context: &LabelContext) -> (String, String) {
    let head = || context.head.clone().unwrap_or_else(|| "HEAD".to_string());
    match target {
        Target::Working => (head(), "working tree".to_string()),
        Target::Staged => (head(), "staged".to_string()),
        // The old side is the merge base with the base branch; the board names it by the branch.
        Target::Branch { branch, base } => (
            base.as_ref()
                .or(context.default_base.as_ref())
                .cloned()
                .unwrap_or_else(|| "merge base".to_string()),
            branch.clone(),
        ),
        Target::Commit { rev } => (format!("{rev}^"), rev.clone()),
        Target::Range { from, to, .. } => (from.clone(), to.clone()),
        Target::Pr { number, .. } => ("base".to_string(), format!("#{number}")),
    }
}

#[cfg(test)]
mod tests;
