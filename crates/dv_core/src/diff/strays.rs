//! Files the old side holds that the working tree has untracked (PLAN §6 step 1).
//!
//! A round's snapshot holds the untracked files of its time (`git add -A`, PLAN §2.3), so an
//! interdiff's old side names files the index doesn't track. git's diff against the working
//! tree sees a file only through the index, so it lists each of them as deleted (or pairs it,
//! as a rename's source, with a similar file added since), while `ls-files --others` lists the
//! same file as new. A temporary index that registers each one at its old-side blob
//! (`update-index --index-info`, which writes no object) has git compare them with the files on
//! disk instead: a changed one lists as modified, an unchanged one not at all — what a snapshot
//! taken now would show against the round's.

use std::collections::HashSet;

use super::{DiffError, FileEntry, FileMode, FileStatus};
use crate::backend::GitOpts;
use crate::git::temp_index::{TEMP_INDEX_CONFIG, TempIndex};
use crate::git::{GitRepo, Oid};

/// A file the old side holds as `mode` and `oid` that the working tree has untracked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Stray {
    pub path: String,
    pub mode: FileMode,
    pub oid: Oid,
}

/// The strays among git's `entries` for a working tree new side, given the paths
/// `ls-files --others` lists: a deletion of an untracked path, or a rename from one.
pub(super) fn find(entries: &[FileEntry], untracked: &HashSet<&str>) -> Vec<Stray> {
    entries
        .iter()
        .filter_map(|entry| {
            let path = match entry.status {
                FileStatus::Deleted => &entry.path,
                FileStatus::Renamed { .. } => entry.old_path.as_ref()?,
                _ => return None,
            };
            if !untracked.contains(path.as_str()) {
                return None;
            }
            Some(Stray {
                path: path.clone(),
                mode: entry.old_mode?,
                oid: entry.old_oid.clone()?,
            })
        })
        .collect()
}

/// The stray an entry whose new side is untracked ([`FileEntry::new_untracked`]) stands for.
pub(super) fn of_entry(entry: &FileEntry) -> Option<Stray> {
    Some(Stray {
        path: entry.path.clone(),
        mode: entry.old_mode?,
        oid: entry.old_oid.clone()?,
    })
}

/// A copy of the worktree's index with each of `strays` registered at its old-side mode and
/// blob, which `git diff` against the working tree then compares with the file on disk. Its
/// entries carry no stat data, so git reads those files again, as it does a file it finds
/// stat-dirty.
pub(super) fn index<'a>(repo: &'a GitRepo, strays: &[Stray]) -> Result<TempIndex<'a>, DiffError> {
    let backend = repo.backend().as_ref();
    let index = TempIndex::copy(backend)?;
    let mut stdin = Vec::new();
    for stray in strays {
        // `-z`: `<mode> SP <oid> TAB <path> NUL`, the path taken as it is.
        stdin.extend_from_slice(format!("{} {}\t", stray.mode.octal(), stray.oid).as_bytes());
        stdin.extend_from_slice(stray.path.as_bytes());
        stdin.push(0);
    }
    let mut args = TEMP_INDEX_CONFIG.to_vec();
    args.extend(["update-index", "-z", "--index-info"]);
    backend.git(
        &args,
        &GitOpts::new()
            .with_index_file(index.rel())
            .with_stdin(stdin),
    )?;
    Ok(index)
}
