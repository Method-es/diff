//! The diff pipeline's git half (docs/PLAN.md §6 steps 1–3, D4): the file list with every file's
//! totals, per-file diffs parsed into aligned lines, intraline word ranges, the collapse of
//! unchanged runs into expandable gaps, and the pure helpers the stream's rows are built from
//! ([`split_rows`], [`hunk_range`]).
//!
//! git computes every diff (PLAN D4), so `core.autocrlf`, `.gitattributes` (eol, `-diff`,
//! `binary`) and rename detection behave exactly as they do in git. Per-file diffs run with
//! context larger than the file, so their output holds both sides' complete text and dv
//! collapses unchanged runs itself, from lines already loaded ([`collapse`]).

mod collapse;
mod file;
mod intraline;
mod list;
mod patch;
mod remap;
mod rows;
mod strays;

use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::backend::BackendError;
use crate::git::{GitError, GitRepo, Oid};
use crate::target::{DiffSpec, Side};

pub use collapse::{
    EXPAND_STEP, Expand, MIN_GAP, Segment, expand_gap, reveal_in_gap, segments, segments_showing,
};
pub use file::file_diff;
pub(crate) use intraline::Tokens;
pub use intraline::{diff_pair as intraline_pair, mark_intraline};
pub use list::file_list;
pub use remap::remap_lines;
pub use rows::{HunkRange, SplitRow, hunk_range, split_rows};

/// Per side, the byte size beyond which a file isn't diffed until the reviewer asks ("Load
/// anyway", PLAN §6 step 2).
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Per side, the line count beyond which a file isn't shown until the reviewer asks (PLAN §6
/// step 2).
pub const MAX_FILE_LINES: usize = 20_000;

/// The flags every `git diff` dv runs carries, the file list's and each file's alike. Each pins
/// something the user's configuration would otherwise change in what dv parses (PLAN D4, §5.1):
///
/// - `--no-ext-diff`, `--no-textconv`: git's own diff of the real content, never an external
///   tool's or a textconv filter's, because comment anchors and GitHub's line numbers refer to
///   the real file's lines.
/// - `--diff-algorithm=myers`: what GitHub shows, so the list's totals, the hunks and GitHub
///   agree. The flag, not only the policy's `-c diff.algorithm=myers`: a diff driver's
///   `diff.<driver>.algorithm` (git 2.41+) beats the config key but not the flag.
/// - `--indent-heuristic`: git's default and what GitHub shows, whatever
///   `diff.indentHeuristic` says. It decides where a block that could slide (an inserted
///   function next to a similar one) is marked, so the changed lines, and the comment anchors on
///   them, are the ones GitHub marks.
/// - `--submodule=short`: a gitlink's patch is git's `Subproject commit <oid>[-dirty]` lines,
///   which [`file_diff`] reads, whatever `diff.submodule` (`log`, `diff`) says.
/// - `--ignore-submodules=untracked`: git's own default for `git diff` (a submodule checkout is
///   dirty with modified content, not with only untracked files), whatever `diff.ignoreSubmodules`
///   or a `.gitmodules` `ignore` setting says; `all` would hide a submodule bump that GitHub
///   shows. Only the flag beats `.gitmodules`.
const DIFF_PINS: &[&str] = &[
    "--no-ext-diff",
    "--no-textconv",
    "--diff-algorithm=myers",
    "--indent-heuristic",
    "--submodule=short",
    "--ignore-submodules=untracked",
];

/// How an untracked file's path is passed to `git diff --no-index`, which is also the name its
/// patch prints. A path of exactly `-` means stdin there, even after `--`, and git would diff
/// dv's empty stdin instead of the file; `./-` names the file on disk, which git then converts
/// (autocrlf, attributes) like any other.
fn no_index_path(path: &str) -> &str {
    if path == "-" { "./-" } else { path }
}

/// One changed file in a diff (PLAN §6 step 1), with its totals known up front (PLAN §15: no
/// totals that appear only once a file is opened).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// The file's path on the new side (the old side for a deleted file), relative to the
    /// worktree root, `/`-separated.
    pub path: String,
    /// The old side's path when it differs: the source of a rename or copy.
    pub old_path: Option<String>,
    pub status: FileStatus,
    /// Lines added, as `git diff --numstat` counts them (`0` for a binary file); for an untracked
    /// file, its line count.
    pub additions: u32,
    /// Lines removed (`0` for a binary file).
    pub deletions: u32,
    /// git treats the file as binary: by `.gitattributes` (`-diff`, `binary`) or by content (a
    /// NUL byte near the start).
    pub binary: bool,
    /// The old side's mode; `None` when the file doesn't exist there.
    pub old_mode: Option<FileMode>,
    /// The new side's mode; `None` when the file doesn't exist there, or when git hasn't
    /// recorded one (an untracked file, except that an untracked symbolic link is
    /// [`FileMode::Symlink`] and a nested repository [`FileMode::Submodule`]).
    pub new_mode: Option<FileMode>,
    /// The old side's object id (a blob, or a commit for a submodule); `None` when the file
    /// doesn't exist there.
    pub old_oid: Option<Oid>,
    /// The new side's object id; `None` when the file doesn't exist there or git hasn't hashed
    /// it (a working tree file it didn't read, an untracked file).
    pub new_oid: Option<Oid>,
    /// The new side is a working tree file the index doesn't track, though the old side has
    /// the file: in an interdiff, a file the round's snapshot holds that is still untracked
    /// (PLAN §6 step 1). git compares such a file with the old side only through a temporary
    /// index that names it, which [`file_diff`] builds. An [`FileStatus::Untracked`] entry is
    /// new instead, and diffs against nothing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new_untracked: bool,
}

impl FileEntry {
    /// Whether either side is a submodule (a gitlink) or, untracked, a nested repository.
    pub fn is_submodule(&self) -> bool {
        self.old_mode == Some(FileMode::Submodule) || self.new_mode == Some(FileMode::Submodule)
    }

    /// Whether every side the file exists on is a gitlink (or, untracked, a nested
    /// repository): a submodule change, whose diff is its commit ids. A type change between a
    /// gitlink and a file or symlink is not; it diffs as text, git's `Subproject commit` line
    /// against the file's lines.
    pub fn is_gitlink_only(&self) -> bool {
        let gitlink_or_absent =
            |mode: Option<FileMode>| matches!(mode, None | Some(FileMode::Submodule));
        self.is_submodule() && gitlink_or_absent(self.old_mode) && gitlink_or_absent(self.new_mode)
    }
}

/// How a file changed, from git's `--raw` status letters, plus untracked files.
///
/// Serialised externally tagged in snake_case: `"added"`, `{"renamed": {"similarity": 90}}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    /// Moved from [`FileEntry::old_path`]; `similarity` is git's score in percent (100 for an
    /// unchanged move).
    Renamed {
        similarity: u8,
    },
    /// Copied from [`FileEntry::old_path`]. The file list runs with rename detection only (no
    /// `-C`, like GitHub), so git reports a copy as added; the status exists for diffs whose
    /// output does carry copies.
    Copied {
        similarity: u8,
    },
    /// The kind of file changed (a regular file became a symlink or a submodule, or back).
    TypeChanged,
    /// Not in the index and not ignored: new on the working tree side.
    Untracked,
    /// Has unresolved merge conflicts (stages 1–3 in the index); the index holds no single
    /// version to show.
    Unmerged,
}

/// A file's mode as git records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileMode {
    /// `100644`.
    Regular,
    /// `100755`.
    Executable,
    /// `120000`: the blob is the link target.
    Symlink,
    /// `160000`: a gitlink, whose object id is a commit in the submodule.
    Submodule,
}

impl FileMode {
    /// Parses an octal mode from git's `--raw` output; `Ok(None)` for `000000` (no file).
    pub fn parse_octal(text: &str) -> Option<Option<Self>> {
        let mode = u32::from_str_radix(text, 8).ok()?;
        Some(match mode {
            0 => None,
            0o120000 => Some(FileMode::Symlink),
            0o160000 => Some(FileMode::Submodule),
            mode if mode & 0o170000 == 0o100000 => Some(if mode & 0o111 != 0 {
                FileMode::Executable
            } else {
                FileMode::Regular
            }),
            _ => return None,
        })
    }

    /// The mode in git's octal notation (`100644`).
    pub fn octal(self) -> &'static str {
        match self {
            FileMode::Regular => "100644",
            FileMode::Executable => "100755",
            FileMode::Symlink => "120000",
            FileMode::Submodule => "160000",
        }
    }
}

/// Per-side caps for [`file_diff`] (PLAN §6 step 2). [`DiffLimits::default`] holds the
/// constants; "Load anyway" diffs with [`DiffLimits::UNLIMITED`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLimits {
    pub max_bytes: u64,
    pub max_lines: usize,
}

impl DiffLimits {
    pub const UNLIMITED: Self = Self {
        max_bytes: u64::MAX,
        max_lines: usize::MAX,
    };
}

impl Default for DiffLimits {
    fn default() -> Self {
        Self {
            max_bytes: MAX_FILE_BYTES,
            max_lines: MAX_FILE_LINES,
        }
    }
}

/// One file's diff (PLAN §6 step 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileDiff {
    /// Both sides' text, aligned.
    Text(TextDiff),
    /// git treats the file as binary; sizes in bytes, `None` for a side that doesn't exist (or,
    /// for a working tree file that vanished, couldn't be measured).
    Binary {
        old_len: Option<u64>,
        new_len: Option<u64>,
    },
    /// A side exceeds [`DiffLimits`]; sizes in bytes. Diffing again with
    /// [`DiffLimits::UNLIMITED`] loads it.
    TooLarge {
        old_len: Option<u64>,
        new_len: Option<u64>,
    },
    /// A submodule's commit changed (or, untracked, a nested repository appeared). `dirty`: the
    /// working tree's submodule checkout has changes of its own (git's `-dirty`).
    Submodule {
        old_oid: Option<Oid>,
        new_oid: Option<Oid>,
        dirty: bool,
    },
    /// The file has unresolved merge conflicts and the new side is the index, which holds no
    /// single version of it.
    Unmerged,
}

impl FileDiff {
    /// The text diff, when this is one.
    pub fn as_text(&self) -> Option<&TextDiff> {
        match self {
            FileDiff::Text(text) => Some(text),
            _ => None,
        }
    }

    /// The rows to show with `context` unchanged lines around each change ([`segments`]); empty
    /// for anything but text.
    pub fn segments(&self, context: usize) -> Vec<Segment> {
        self.as_text()
            .map(|text| text.segments(context))
            .unwrap_or_default()
    }
}

/// A text diff: every line of both sides in order, unchanged lines included, because the
/// per-file diff runs with context larger than the file (PLAN D4). A file whose content didn't
/// change (a mode change, an unchanged rename) is all context lines ([`TextDiff::has_changes`]
/// is false); an empty file has no lines.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TextDiff {
    pub lines: Vec<DiffLine>,
    /// The old side ends with a newline (vacuously true when it has no lines or didn't change).
    pub old_eof_newline: bool,
    /// The new side ends with a newline.
    pub new_eof_newline: bool,
    /// Lines in git's output ended `\r\n`; the `\r` is stripped from [`DiffLine::text`]. This is
    /// the content as git compares it: under `core.autocrlf` or a `text eol=crlf` attribute git
    /// compares the normalised LF form, so a CRLF checkout of such a file reports `false`.
    pub crlf: bool,
}

impl TextDiff {
    /// A diff with no lines whose sides both end with a newline.
    pub fn empty() -> Self {
        Self {
            lines: Vec::new(),
            old_eof_newline: true,
            new_eof_newline: true,
            crlf: false,
        }
    }

    /// The rows to show with `context` unchanged lines around each change ([`segments`]).
    pub fn segments(&self, context: usize) -> Vec<Segment> {
        segments(&self.lines, context)
    }

    /// Whether any line was added or removed. A file renamed or with its mode changed, its
    /// content as it was, has its text as context lines alone ("No content change").
    pub fn has_changes(&self) -> bool {
        self.lines.iter().any(|line| line.kind != LineKind::Context)
    }

    /// The number of lines on the old side.
    pub fn old_line_count(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| line.old_no.is_some())
            .count()
    }

    /// The number of lines on the new side.
    pub fn new_line_count(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| line.new_no.is_some())
            .count()
    }
}

/// One line of a [`TextDiff`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: LineKind,
    /// 1-based line number on the old side; `None` for an added line.
    pub old_no: Option<u32>,
    /// 1-based line number on the new side; `None` for a removed line.
    pub new_no: Option<u32>,
    /// The line without its line ending.
    pub text: String,
    /// Intraline word ranges: byte ranges of `text` that differ from the paired line on the other
    /// side (PLAN §6 step 3), on char boundaries. Empty for unpaired lines and for pairs too
    /// different or too long to mark ([`intraline_pair`]).
    pub words: Vec<Range<usize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

impl LineKind {
    /// Whether the line is a change (added or removed), not context.
    pub fn is_change(self) -> bool {
        self != LineKind::Context
    }
}

/// Why the file list or a file's diff failed.
#[derive(Debug, thiserror::Error)]
pub enum DiffError {
    #[error(transparent)]
    Git(#[from] GitError),
    /// The spec's sides are a pair git can't diff in that direction (the working tree on the old
    /// side, say). Targets never resolve to one ([`crate::target::Target::resolve`]).
    #[error("can't diff {old} against {new}")]
    UnsupportedSpec { old: String, new: String },
    #[error("unexpected output from git {command}: {detail}")]
    UnexpectedOutput {
        command: &'static str,
        detail: String,
    },
}

impl From<BackendError> for DiffError {
    fn from(err: BackendError) -> Self {
        DiffError::Git(GitError::Backend(err))
    }
}

/// The `git diff` arguments that compare `spec`'s two sides: `<old> <new>` for two trees,
/// `--cached <old>` for a tree against the index, `<old>` for a tree against the working tree,
/// nothing for the index against the working tree. Object ids are full hex, so none can be read
/// as an option.
fn side_args(repo: &GitRepo, spec: &DiffSpec) -> Result<Vec<String>, DiffError> {
    let tree = |side: &Side| match side {
        Side::Commit(oid) => Some(oid.to_string()),
        Side::EmptyTree => Some(repo.empty_tree().to_string()),
        Side::Index | Side::Worktree => None,
    };
    match (tree(&spec.old), tree(&spec.new), &spec.new) {
        (Some(old), Some(new), _) => Ok(vec![old, new]),
        (Some(old), None, Side::Index) => Ok(vec!["--cached".to_string(), old]),
        (Some(old), None, Side::Worktree) => Ok(vec![old]),
        (None, None, Side::Worktree) if spec.old == Side::Index => Ok(Vec::new()),
        _ => Err(DiffError::UnsupportedSpec {
            old: spec.old.to_string(),
            new: spec.new.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests;
