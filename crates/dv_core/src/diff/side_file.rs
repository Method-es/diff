//! One side's files whole (PLAN §18.1 "The file view", §8): the paths a side holds, and one
//! file's text at that side, for the file view and for the threads on files a review doesn't
//! list.
//!
//! **Decoding.** A file's text at a side is read through the diff pipeline itself: the side
//! diffed against the empty tree, one file, with the pinned flags ([`file_diff`] on the entry
//! `git diff --raw --numstat` gives it), so git converts the working tree file (autocrlf,
//! `eol`, a clean filter, a `working-tree-encoding`) and classifies it (binary, too large under
//! the same [`DiffLimits`]) exactly as it does the new side of any review's diff. A line of the
//! file view therefore holds the same text as the same line in the stream, which anchors
//! compare (PLAN §8). Every line comes back as a context line numbered alike on both sides, as
//! a file whose content didn't change does, so the same helpers read either.
//!
//! **Which files.** A tree side's files are its blobs and links; the index's, every path it
//! tracks; the working tree's, every tracked path still on disk plus the untracked files git
//! doesn't ignore, as the review's file list sees them. One file of the working tree is read
//! whatever git says of it, ignored ones too: a language server's definition can be in an
//! ignored file inside the worktree (a library's `.d.ts` under `node_modules`, PLAN §18.1), and
//! the file view shows it as git would show it untracked.

use std::collections::BTreeSet;

use super::list::{git, ignored_at, tracked, untracked_at};
use super::{DiffError, DiffLimits, FileDiff, LineKind, TextDiff, file_diff};
use crate::git::GitRepo;
use crate::target::{DiffSpec, Side};

/// Every file path `side` holds, sorted, `/`-separated and relative to the worktree root
/// (module docs, "Which files"). Blocking: one or two git calls through the repo's backend.
pub fn side_paths(repo: &GitRepo, side: &Side) -> Result<Vec<String>, DiffError> {
    let names = |args: &[&str]| -> Result<Vec<String>, DiffError> {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        Ok(split_z(&git(repo, &args, None)?.stdout))
    };
    let mut paths: BTreeSet<String> = match side {
        Side::EmptyTree => BTreeSet::new(),
        Side::Commit(oid) => names(&[
            "ls-tree",
            "-r",
            "-z",
            "--name-only",
            "--full-tree",
            oid.as_str(),
        ])?
        .into_iter()
        .collect(),
        Side::Index => names(&["ls-files", "-z", "--cached"])?
            .into_iter()
            .collect(),
        Side::Worktree => {
            let deleted: BTreeSet<String> = names(&["ls-files", "-z", "--deleted"])?
                .into_iter()
                .collect();
            names(&[
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ])?
            .into_iter()
            // A nested repository lists as `dir/`.
            .map(|path| path.trim_end_matches('/').to_string())
            .filter(|path| !deleted.contains(path))
            .collect()
        }
    };
    paths.remove("");
    Ok(paths.into_iter().collect())
}

/// `path`'s content at `side` as the diff pipeline reads that side (module docs, "Decoding"):
/// its lines as context lines, or binary, too large under `limits`, a submodule or unmerged, as
/// a review's diff of the file would say; in the working tree an ignored file reads as an
/// untracked one would. `None` when `side` holds no file at `path`. Blocking: two to four git
/// calls.
pub fn side_file(
    repo: &GitRepo,
    side: &Side,
    path: &str,
    limits: &DiffLimits,
) -> Result<Option<FileDiff>, DiffError> {
    if *side == Side::EmptyTree {
        return Ok(None);
    }
    let spec = DiffSpec {
        old: Side::EmptyTree,
        new: side.clone(),
    };
    let mut entries = tracked(repo, &spec, None, Some(path))?;
    if entries.is_empty() && *side == Side::Worktree {
        entries = untracked_at(repo, Some(path))?;
    }
    if entries.is_empty() && *side == Side::Worktree {
        entries = ignored_at(repo, path)?;
    }
    let Some(entry) = entries.into_iter().find(|entry| entry.path == path) else {
        return Ok(None);
    };
    Ok(Some(match file_diff(repo, &spec, &entry, limits)? {
        FileDiff::Text(text) => FileDiff::Text(as_context(text)),
        other => other,
    }))
}

/// A diff of a file against nothing (every line added) as the file's text: every line context,
/// numbered alike on both sides.
fn as_context(text: TextDiff) -> TextDiff {
    let lines = text
        .lines
        .into_iter()
        .filter(|line| line.new_no.is_some())
        .map(|mut line| {
            line.kind = LineKind::Context;
            line.old_no = line.new_no;
            line.words.clear();
            line
        })
        .collect();
    TextDiff {
        lines,
        old_eof_newline: text.new_eof_newline,
        new_eof_newline: text.new_eof_newline,
        crlf: text.crlf,
    }
}

/// NUL-separated names, the empty ones left out.
fn split_z(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|&b| b == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect()
}
