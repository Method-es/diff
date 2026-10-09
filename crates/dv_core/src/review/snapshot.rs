//! Round snapshots (docs/PLAN.md §2.3): the new side of a review's target as it stood when the
//! reviewer finished a round, kept as a commit at `refs/dv/reviews/<id>/<n>`.
//!
//! A commit side is kept as it is. The index and the working tree have no commit, so one is
//! made without touching the user's index (PLAN §2.4):
//!
//! 1. The worktree's index is copied byte for byte into the store
//!    (`<git-common-dir>/dv/tmp/index-<ulid>`, `crate::git::temp_index`). The copy keeps the
//!    stat data git cached, so for the working tree only files that changed since the last
//!    `git add` or `git status` are read and hashed again.
//! 2. For the working tree, `git add -A` stages into the copy (`GIT_INDEX_FILE`,
//!    [`GitOpts::index_file`]): tracked and untracked files, ignores honoured, through the same
//!    clean filters and end-of-line conversion `git diff` applies, so the snapshot holds what
//!    the reviewer was shown. Unresolved conflicts are staged as the files stand (markers
//!    included), as `git add` does. A nested repository without a commit, which git can't
//!    stage, is left out rather than failing the round (`--ignore-errors`); any other file git
//!    couldn't read (one another program holds open without sharing it) fails the round with
//!    [`SnapshotError::Unreadable`], because the copy would keep a version of it the reviewer
//!    wasn't shown.
//! 3. `git write-tree` on the copy gives the tree; an index side with unresolved conflicts has
//!    none ([`SnapshotError::Unmerged`]).
//! 4. `git commit-tree` makes the commit, as `dv <dv@dv.invalid>` (`-c user.name -c user.email`,
//!    so it works when the user has no identity configured; `GIT_AUTHOR_*` and
//!    `GIT_COMMITTER_*` variables in git's environment still win, as they win over any
//!    configuration), with the old side's commit as its parent when the old side is a commit,
//!    and the message `dv review <id> round <n>`.
//!
//! The ref is created with `git update-ref <ref> <new> <zero-oid>`, which never overwrites: an
//! existing ref with the same commit, or with a commit of the same tree and parent (made by an
//! earlier attempt whose round never closed), is reused. Any other commit at the ref of a round
//! that isn't closed is also the leftover of such an attempt, made before the content changed
//! (or before a branch moved on from the commit an earlier round closed at): it is replaced with
//! a compare-and-swap (`git update-ref <ref> <new> <old>`), never blindly. The ref of a closed
//! round is never replaced: another commit there is [`SnapshotError::RefExists`].
//!
//! The temp index and its `.lock` are removed afterwards, also when a step fails; one a
//! process killed mid-snapshot left is swept after [`STALE_TEMP`].
//!
//! The objects these steps write are reachable only through `refs/dv/**`; deleting the review
//! removes its refs (`ReviewStore::delete`) and `git gc` the objects (PLAN §17).

use std::collections::HashSet;

use super::id::ReviewId;
use crate::backend::{Backend, BackendError, GitOpts, GitOutput};
pub use crate::git::temp_index::STALE_TEMP;
use crate::git::temp_index::{TEMP_INDEX_CONFIG, TempIndex};
use crate::git::{GitRepo, Oid};
use crate::target::{DiffSpec, Side};

/// The identity snapshot commits are made with, whatever the user has configured.
const IDENTITY: [&str; 4] = ["-c", "user.name=dv", "-c", "user.email=dv@dv.invalid"];

/// Why a round couldn't be snapshotted.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error(transparent)]
    Backend(#[from] BackendError),
    /// The index has unresolved merge conflicts, so it holds no single tree to snapshot.
    #[error("the index has unresolved merge conflicts")]
    Unmerged {
        /// What git said, for a disclosure.
        detail: String,
    },
    /// The round's ref already points at another commit: the commit the round closed at, or
    /// one another writer put there in between.
    #[error("{name} already exists at {existing}")]
    RefExists { name: String, existing: Oid },
    /// git couldn't read these working tree files, so the snapshot can't hold what the reviewer
    /// was shown (module docs).
    #[error("git couldn't read {} to snapshot the round", describe_paths(.paths))]
    Unreadable {
        paths: Vec<String>,
        /// What git said, for a disclosure.
        detail: String,
    },
    /// The old side of a snapshot is never the index or the working tree.
    #[error("can't snapshot a round whose old side is {side}")]
    UnsupportedSide { side: String },
    #[error("unexpected output from git {command}: {detail}")]
    UnexpectedOutput {
        command: &'static str,
        detail: String,
    },
}

/// The ref that keeps round `n` of a review (PLAN §2.3).
pub fn ref_name(review: &ReviewId, n: u32) -> String {
    format!("refs/dv/reviews/{review}/{n}")
}

/// Removes round `n`'s ref of `review` while it still points at `commit` (a compare-and-swap
/// delete, `git update-ref -d <ref> <commit>`): the snapshot of a round whose events couldn't
/// land because the review was deleted while it was taken. Best effort: a failure is logged,
/// and deleting the review again removes the ref anyway.
pub(crate) fn remove_ref(repo: &GitRepo, review: &ReviewId, n: u32, commit: &Oid) {
    let name = ref_name(review, n);
    let removed = repo.backend().git(
        &["update-ref", "-d", &name, commit.as_str()],
        &GitOpts::new(),
    );
    if let Err(error) = removed {
        log::warn!("review {review}: couldn't remove {name}: {error}");
    }
}

/// `a.txt`, `a.txt and b.txt`, `a.txt and 2 more files`.
fn describe_paths(paths: &[String]) -> String {
    match paths {
        [] => "a file".to_string(),
        [one] => one.clone(),
        [one, two] => format!("{one} and {two}"),
        [one, rest @ ..] => format!("{one} and {} more files", rest.len()),
    }
}

/// Snapshots `spec`'s new side as round `n` of `review` and returns the commit
/// [`ref_name`] now points at (module docs). `closed` is the commit round `n` closed at, when it
/// is closed: that ref is never replaced, while the ref of a round in progress holds at most a
/// leftover, which may be. Writes nothing outside the object store, the round's ref and the
/// store's temp files, which are removed before it returns.
pub fn take(
    repo: &GitRepo,
    review: &ReviewId,
    n: u32,
    spec: &DiffSpec,
    closed: Option<&Oid>,
) -> Result<Oid, SnapshotError> {
    let parent = match &spec.old {
        Side::Commit(oid) => Some(oid),
        Side::EmptyTree => None,
        side @ (Side::Index | Side::Worktree) => {
            return Err(SnapshotError::UnsupportedSide {
                side: side.to_string(),
            });
        }
    };
    let message = format!("dv review {review} round {n}");
    // The commit, and the tree when this call made it.
    let (commit, tree) = match &spec.new {
        Side::Commit(oid) => (oid.clone(), None),
        Side::EmptyTree => {
            let tree = repo.empty_tree();
            (commit_tree(repo, &tree, parent, &message)?, Some(tree))
        }
        Side::Index => {
            let tree = temp_index_tree(repo, false)?;
            (commit_tree(repo, &tree, parent, &message)?, Some(tree))
        }
        Side::Worktree => {
            let tree = temp_index_tree(repo, true)?;
            (commit_tree(repo, &tree, parent, &message)?, Some(tree))
        }
    };
    let name = ref_name(review, n);
    match create_ref(repo, &name, &commit) {
        Ok(()) => Ok(commit),
        // A commit made now differs from one made earlier by its date alone when the content
        // is the same: an earlier attempt whose round never closed (its append failed), which
        // this one may reuse.
        Err(SnapshotError::RefExists { existing, .. })
            if tree.as_ref().is_some_and(|tree| {
                same_content(repo, &existing, tree, parent).unwrap_or(false)
            }) =>
        {
            Ok(existing)
        }
        // Another commit at the ref of a round that isn't closed: an earlier attempt whose
        // round never closed, made before the content changed. It may be the very commit an
        // earlier round closed at (a branch that hadn't moved since), which that round's own
        // ref keeps.
        Err(SnapshotError::RefExists { existing, .. }) if closed != Some(&existing) => {
            replace_ref(repo, &name, &commit, &existing)?;
            Ok(commit)
        }
        Err(err) => Err(err),
    }
}

/// Whether `commit` has `tree` and exactly `parent` as its parents.
fn same_content(
    repo: &GitRepo,
    commit: &Oid,
    tree: &Oid,
    parent: Option<&Oid>,
) -> Result<bool, SnapshotError> {
    let peeled = format!("{commit}^{{tree}}");
    let output = repo.backend().git(
        &["rev-parse", "--verify", "--end-of-options", &peeled],
        &GitOpts::new(),
    )?;
    if parse_oid("rev-parse", &output)? != *tree {
        return Ok(false);
    }
    let parents = repo.parents(commit).map_err(|err| match err {
        crate::git::GitError::Backend(err) => SnapshotError::Backend(err),
        other => SnapshotError::UnexpectedOutput {
            command: "rev-list",
            detail: other.to_string(),
        },
    })?;
    Ok(parents.iter().eq(parent))
}

/// Copies the index into the store, stages the working tree into the copy when `add_worktree`
/// is set, and returns the copy's tree. The copy and its lock are removed whatever happens.
fn temp_index_tree(repo: &GitRepo, add_worktree: bool) -> Result<Oid, SnapshotError> {
    let backend = repo.backend().as_ref();
    let index = TempIndex::copy(backend)?;
    if add_worktree {
        stage_worktree(backend, index.rel())?;
    }
    write_tree(backend, index.rel())
}

/// `git add -A` into the temp index. Exit code 1 is `--ignore-errors` reporting files it
/// couldn't index and left out: a nested repository without a commit may be left out, anything
/// else is [`SnapshotError::Unreadable`] (module docs). A real failure exits 128.
fn stage_worktree(backend: &dyn Backend, index: &str) -> Result<(), SnapshotError> {
    let mut args = TEMP_INDEX_CONFIG.to_vec();
    args.extend(["add", "-A", "--ignore-errors"]);
    let opts = GitOpts::new().with_index_file(index).with_ok_codes(&[0, 1]);
    let output = backend.git(&args, &opts)?;
    if output.code == 0 {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim_end();
    let left_out = left_out_paths(stderr);
    match (left_out.unreadable.is_empty(), left_out.nested.is_empty()) {
        (true, false) => {
            log::info!(
                "a round snapshot left out nested repositories without a commit: {}",
                left_out.nested.join(", ")
            );
            Ok(())
        }
        (true, true) => Err(SnapshotError::UnexpectedOutput {
            command: "add",
            detail: stderr.to_string(),
        }),
        (false, _) => Err(SnapshotError::Unreadable {
            paths: left_out.unreadable,
            detail: stderr.to_string(),
        }),
    }
}

/// What `git add --ignore-errors` reported it left out.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LeftOut {
    /// Nested repositories without a commit (`'<dir>/' does not have a commit checked out`).
    pub nested: Vec<String>,
    /// Every other path it couldn't index (`unable to index file '<path>'`).
    pub unreadable: Vec<String>,
}

/// Parses `git add --ignore-errors`'s stderr (git runs with `LC_ALL=C`, so its messages are
/// git's own English).
pub(crate) fn left_out_paths(stderr: &str) -> LeftOut {
    let mut nested = Vec::new();
    let mut failed = Vec::new();
    for line in stderr.lines() {
        let Some(message) = line.strip_prefix("error: ") else {
            continue;
        };
        if let Some(path) = message
            .strip_prefix('\'')
            .and_then(|rest| rest.strip_suffix("' does not have a commit checked out"))
        {
            nested.push(path.to_string());
        } else if let Some(path) = message
            .strip_prefix("unable to index file '")
            .and_then(|rest| rest.strip_suffix('\''))
        {
            failed.push(path.to_string());
        }
    }
    let skipped: HashSet<&str> = nested.iter().map(String::as_str).collect();
    let unreadable = failed
        .into_iter()
        .filter(|path| !skipped.contains(path.as_str()))
        .collect();
    LeftOut { nested, unreadable }
}

/// `git write-tree` of the temp index; [`SnapshotError::Unmerged`] when it has conflicts.
fn write_tree(backend: &dyn Backend, index: &str) -> Result<Oid, SnapshotError> {
    let mut args = TEMP_INDEX_CONFIG.to_vec();
    args.push("write-tree");
    let output = match backend.git(&args, &GitOpts::new().with_index_file(index)) {
        Ok(output) => output,
        // git lists each conflicted entry as `<path>: unmerged (<oid>)` before giving up.
        Err(BackendError::Git { stderr, .. }) if stderr.contains(": unmerged (") => {
            return Err(SnapshotError::Unmerged {
                detail: stderr.trim_end().to_string(),
            });
        }
        Err(err) => return Err(err.into()),
    };
    parse_oid("write-tree", &output)
}

/// `git commit-tree` of `tree` with the fixed identity, never signed (a signing prompt would
/// hang a headless call, and an older git signs under `commit.gpgSign`).
fn commit_tree(
    repo: &GitRepo,
    tree: &Oid,
    parent: Option<&Oid>,
    message: &str,
) -> Result<Oid, SnapshotError> {
    let mut args = IDENTITY.to_vec();
    args.extend(["commit-tree", "--no-gpg-sign", "-m", message]);
    if let Some(parent) = parent {
        args.extend(["-p", parent.as_str()]);
    }
    args.push(tree.as_str());
    let output = repo.backend().git(&args, &GitOpts::new())?;
    parse_oid("commit-tree", &output)
}

/// Creates `name` at `commit` unless it exists: an existing ref at the same commit is fine,
/// one at another commit is [`SnapshotError::RefExists`].
fn create_ref(repo: &GitRepo, name: &str, commit: &Oid) -> Result<(), SnapshotError> {
    let zero = "0".repeat(repo.object_format().hex_len());
    let created = repo.backend().git(
        &["update-ref", name, commit.as_str(), &zero],
        &GitOpts::new(),
    );
    let err = match created {
        Ok(_) => return Ok(()),
        Err(err) => err,
    };
    // `--verify --quiet` exits 1 without output when the ref doesn't exist.
    let existing = repo.backend().git(
        &["rev-parse", "--verify", "--quiet", "--end-of-options", name],
        &GitOpts::new().with_ok_codes(&[0, 1]),
    )?;
    if existing.code == 1 {
        return Err(err.into());
    }
    let existing = parse_oid("rev-parse", &existing)?;
    if existing == *commit {
        Ok(())
    } else {
        Err(SnapshotError::RefExists {
            name: name.to_string(),
            existing,
        })
    }
}

/// Moves `name` from `existing` to `commit`, only while it still points at `existing` (a
/// compare-and-swap: another writer's move in between makes it [`SnapshotError::RefExists`]).
fn replace_ref(
    repo: &GitRepo,
    name: &str,
    commit: &Oid,
    existing: &Oid,
) -> Result<(), SnapshotError> {
    let moved = repo.backend().git(
        &["update-ref", name, commit.as_str(), existing.as_str()],
        &GitOpts::new(),
    );
    let err = match moved {
        Ok(_) => return Ok(()),
        Err(err) => err,
    };
    let now = repo.backend().git(
        &["rev-parse", "--verify", "--quiet", "--end-of-options", name],
        &GitOpts::new().with_ok_codes(&[0, 1]),
    )?;
    if now.code == 1 {
        return Err(err.into());
    }
    Err(SnapshotError::RefExists {
        name: name.to_string(),
        existing: parse_oid("rev-parse", &now)?,
    })
}

/// The single object id `command` printed.
fn parse_oid(command: &'static str, output: &GitOutput) -> Result<Oid, SnapshotError> {
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim_end();
    Oid::parse(text).ok_or_else(|| SnapshotError::UnexpectedOutput {
        command,
        detail: format!("expected an object id, got {text:?}"),
    })
}
