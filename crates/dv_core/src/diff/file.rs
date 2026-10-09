//! One file's diff, on demand (docs/PLAN.md §6 step 2, D4).
//!
//! Cheap checks come first, so nothing large is read for a file that won't be shown: unmerged
//! entries and submodule changes (every side a gitlink) need no content; then both sides' sizes
//! (blob sizes from one `git cat-file --batch-check`, a working tree file's from the backend)
//! decide `Binary` (the file list already knows) and `TooLarge`. Only then does git print the
//! diff, with more context lines than the file has (the count comes from the size check: a file
//! of `n` bytes has at most `n + 1` lines), so the output is both sides' complete text. A listed
//! file whose content didn't change (renamed or its mode changed, nothing else) gets no hunk from
//! git; its text is read from the old side's blob and given as context lines, so it still holds
//! both sides' complete text, which threads are placed against (PLAN §8).

use std::collections::HashMap;

use super::list::git;
use super::patch::{self, PatchFile};
use super::{
    DIFF_PINS, DiffError, DiffLimits, DiffLine, FileDiff, FileEntry, FileMode, FileStatus,
    LineKind, TextDiff, mark_intraline, no_index_path, side_args, strays,
};
use crate::backend::GitOpts;
use crate::git::{GitRepo, Oid};
use crate::target::{DiffSpec, Side};

/// More than the header lines a per-file diff prints before its hunk (`diff --git`, modes,
/// similarity, renames, `index`, `---`, `+++`, `@@`).
const HEADER_LINES: usize = 16;

/// The largest `-U` dv passes. Beyond about a billion lines a file's tail would land in a second
/// hunk, which only a file far past any sane "Load anyway" reaches.
const MAX_CONTEXT: u64 = 1 << 30;

/// The flags a per-file diff adds to [`DIFF_PINS`] (PLAN D4): no colour, and fixed `a/` `b/`
/// prefixes whatever `diff.noprefix` or `diff.mnemonicPrefix` say, because the parser reads
/// names with them.
const PATCH_FLAGS: &[&str] = &["--no-color", "--src-prefix=a/", "--dst-prefix=b/"];

/// `entry`'s diff between `spec`'s sides (PLAN §6 step 2). `entry` comes from
/// [`super::file_list`] for the same spec; a file that has since stopped differing comes back as
/// an empty [`TextDiff`].
pub fn file_diff(
    repo: &GitRepo,
    spec: &DiffSpec,
    entry: &FileEntry,
    limits: &DiffLimits,
) -> Result<FileDiff, DiffError> {
    if entry.status == FileStatus::Unmerged {
        return Ok(FileDiff::Unmerged);
    }
    if entry.is_gitlink_only() {
        return submodule_diff(repo, spec, entry);
    }
    let (old_len, new_len) = sizes(repo, spec, entry)?;
    if entry.binary {
        return Ok(FileDiff::Binary { old_len, new_len });
    }
    let over = |len: Option<u64>| len.is_some_and(|len| len > limits.max_bytes);
    // A side has at least as many lines as the list says were added to or removed from it.
    let too_many = |count: u32| usize::try_from(count).unwrap_or(usize::MAX) > limits.max_lines;
    if over(old_len) || over(new_len) || too_many(entry.additions) || too_many(entry.deletions) {
        return Ok(FileDiff::TooLarge { old_len, new_len });
    }
    if entry.status == FileStatus::Untracked {
        if new_len.is_none() {
            // Removed since the file list was made.
            return Ok(FileDiff::Text(TextDiff::empty()));
        }
        if entry.new_mode == Some(FileMode::Symlink) {
            // An untracked symbolic link is its target path, one line without a newline, as
            // git shows it; `git diff --no-index` would follow a link to a directory instead.
            return Ok(FileDiff::Text(
                match repo.backend().worktree_read(&entry.path)? {
                    Some(target) => link_diff(&target),
                    None => TextDiff::empty(),
                },
            ));
        }
    }
    let context = old_len
        .max(new_len)
        .unwrap_or(0)
        .saturating_add(1)
        .clamp(3, MAX_CONTEXT);

    let output = run_diff(repo, spec, entry, context)?;
    // The two sides' line counts add up to at least the output's line count, so output far
    // beyond twice the cap has a side over it; that is found here without building its lines.
    let output_lines = output.iter().filter(|&&b| b == b'\n').count();
    if output_lines.saturating_sub(HEADER_LINES) > limits.max_lines.saturating_mul(2) {
        return Ok(FileDiff::TooLarge { old_len, new_len });
    }
    let name = if entry.status == FileStatus::Untracked {
        no_index_path(&entry.path)
    } else {
        &entry.path
    };
    let Some(file) = section(&output, name)? else {
        return Ok(FileDiff::Text(TextDiff::empty()));
    };
    if file.combined {
        return Ok(FileDiff::Unmerged);
    }
    if file.binary {
        return Ok(FileDiff::Binary { old_len, new_len });
    }
    if file.lines.is_empty() {
        // Git printed the file's header (a rename, a mode change) but no hunk.
        let text = unchanged_text(repo, entry)?;
        if text.old_line_count() > limits.max_lines {
            return Ok(FileDiff::TooLarge { old_len, new_len });
        }
        return Ok(FileDiff::Text(text));
    }
    let mut text = TextDiff {
        lines: file.lines,
        old_eof_newline: file.old_eof_newline,
        new_eof_newline: file.new_eof_newline,
        crlf: file.crlf,
    };
    if text.old_line_count() > limits.max_lines || text.new_line_count() > limits.max_lines {
        return Ok(FileDiff::TooLarge { old_len, new_len });
    }
    mark_intraline(&mut text.lines);
    Ok(FileDiff::Text(text))
}

/// The text of a listed file git printed no hunk for: both sides are the old side's blob (the
/// content as git compares it, which a working tree side matches once converted, or git would
/// have printed a hunk), as context lines. A file without an old blob (an empty file added) or
/// without a new side (an empty file deleted) has no lines.
fn unchanged_text(repo: &GitRepo, entry: &FileEntry) -> Result<TextDiff, DiffError> {
    let blob = match (entry.old_mode, &entry.old_oid) {
        (Some(FileMode::Regular | FileMode::Executable | FileMode::Symlink), Some(oid))
            if entry.status != FileStatus::Deleted =>
        {
            oid
        }
        _ => return Ok(TextDiff::empty()),
    };
    let args = ["cat-file", "blob", blob.as_str()].map(String::from);
    Ok(context_text(&git(repo, &args, None)?.stdout))
}

/// `content` as a diff that changes nothing: every line context, numbered alike on both sides,
/// line endings as git's patch output gives them (`\r\n` stripped and reported).
fn context_text(content: &[u8]) -> TextDiff {
    let mut text = TextDiff::empty();
    if content.is_empty() {
        return text;
    }
    let ends_in_newline = content.ends_with(b"\n");
    let body = content.strip_suffix(b"\n").unwrap_or(content);
    for (index, line) in body.split(|&b| b == b'\n').enumerate() {
        let line = match line.strip_suffix(b"\r") {
            Some(stripped) => {
                text.crlf = true;
                stripped
            }
            None => line,
        };
        let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        text.lines.push(DiffLine {
            kind: LineKind::Context,
            old_no: Some(number),
            new_no: Some(number),
            text: String::from_utf8_lossy(line).into_owned(),
            words: Vec::new(),
        });
    }
    text.old_eof_newline = ends_in_newline;
    text.new_eof_newline = ends_in_newline;
    text
}

/// The diff of a new symbolic link: its target path, added, without a newline.
fn link_diff(target: &[u8]) -> TextDiff {
    TextDiff {
        lines: vec![DiffLine {
            kind: LineKind::Added,
            old_no: None,
            new_no: Some(1),
            text: String::from_utf8_lossy(target).into_owned(),
            words: Vec::new(),
        }],
        old_eof_newline: true,
        new_eof_newline: false,
        crlf: false,
    }
}

/// Runs git's diff of `entry` with `context` lines of context and returns what it printed.
fn run_diff(
    repo: &GitRepo,
    spec: &DiffSpec,
    entry: &FileEntry,
    context: u64,
) -> Result<Vec<u8>, DiffError> {
    let mut args = vec!["diff".to_string()];
    let untracked = entry.status == FileStatus::Untracked;
    if untracked {
        // An untracked file isn't in any tree or the index; `--no-index` diffs it against
        // nothing, honouring the repo's attributes and autocrlf as `git diff` does, and exits 1
        // when the two differ.
        args.push("--no-index".to_string());
    }
    args.extend(
        DIFF_PINS
            .iter()
            .chain(PATCH_FLAGS)
            .map(|flag| flag.to_string()),
    );
    args.push(format!("-U{context}"));
    if untracked {
        args.extend(["--", "/dev/null", no_index_path(&entry.path)].map(String::from));
    } else {
        // Rename detection pairs the old and new paths again, which needs both in the pathspec.
        args.push("-M".to_string());
        args.extend(side_args(repo, spec)?);
        args.push("--".to_string());
        // `:(literal)`: the path is a name, never a glob.
        if let Some(old_path) = &entry.old_path {
            args.push(format!(":(literal){old_path}"));
        }
        args.push(format!(":(literal){}", entry.path));
    }
    let ok_codes: &[i32] = if untracked { &[0, 1] } else { &[0] };
    let str_args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut opts = GitOpts::new().with_ok_codes(ok_codes);
    // A file the old side holds that the working tree has untracked is compared with the file
    // on disk through a temp index that names it (`super::strays`).
    let index = match entry
        .new_untracked
        .then(|| strays::of_entry(entry))
        .flatten()
    {
        Some(stray) => Some(strays::index(repo, &[stray])?),
        None => None,
    };
    if let Some(index) = &index {
        opts = opts.with_index_file(index.rel());
    }
    Ok(repo.backend().git(&str_args, &opts)?.stdout)
}

/// `path`'s section of a per-file diff's output; `None` when git reported no difference.
fn section(output: &[u8], path: &str) -> Result<Option<PatchFile>, DiffError> {
    let files = patch::parse(output).map_err(|detail| DiffError::UnexpectedOutput {
        command: "diff",
        detail,
    })?;
    Ok(select(files, path))
}

/// The section of a per-file diff that belongs to `path`. A type change (a file becoming a
/// symlink) prints as a deletion and an addition of the same path, which are joined into one
/// diff: the old side's lines removed, then the new side's added.
fn select(files: Vec<PatchFile>, path: &str) -> Option<PatchFile> {
    // A section whose names couldn't be parsed is taken only when it is the one section.
    let only = files.len() == 1;
    let mut matching = files.into_iter().filter(|file| match file.new_name() {
        Some(name) => name == path,
        None => only,
    });
    let mut selected = matching.next()?;
    for next in matching {
        selected.binary |= next.binary;
        selected.combined |= next.combined;
        selected.crlf |= next.crlf;
        let removes = next.lines.iter().any(|line| line.kind == LineKind::Removed);
        let adds = next.lines.iter().any(|line| line.kind == LineKind::Added);
        if removes {
            selected.old_eof_newline = next.old_eof_newline;
        }
        if adds {
            selected.new_eof_newline = next.new_eof_newline;
        }
        selected.lines.extend(next.lines);
    }
    Some(selected)
}

/// A submodule's change: the gitlinks' commit ids, which `--raw` already gave, except on a
/// working tree side, where git reads the submodule checkout's `HEAD` (and whether it has changes
/// of its own) only when it prints the patch: `Subproject commit <oid>[-dirty]`.
fn submodule_diff(
    repo: &GitRepo,
    spec: &DiffSpec,
    entry: &FileEntry,
) -> Result<FileDiff, DiffError> {
    let gitlink = |mode: Option<FileMode>, oid: &Option<Oid>| {
        (mode == Some(FileMode::Submodule))
            .then(|| oid.clone())
            .flatten()
    };
    let old_oid = gitlink(entry.old_mode, &entry.old_oid);
    let mut new_oid = gitlink(entry.new_mode, &entry.new_oid);
    let mut dirty = false;
    if entry.status != FileStatus::Untracked
        && spec.new == Side::Worktree
        && entry.new_mode == Some(FileMode::Submodule)
        && let Some(file) = section(&run_diff(repo, spec, entry, 1)?, &entry.path)?
    {
        let line = file
            .lines
            .iter()
            .rev()
            .find(|line| line.kind == LineKind::Added)
            .or_else(|| {
                file.lines
                    .iter()
                    .rev()
                    .find(|line| line.kind == LineKind::Context)
            });
        if let Some(commit) = line.and_then(|line| line.text.strip_prefix("Subproject commit ")) {
            let (commit, is_dirty) = match commit.strip_suffix("-dirty") {
                Some(commit) => (commit, true),
                None => (commit, false),
            };
            new_oid = Oid::parse(commit).or(new_oid);
            dirty = is_dirty;
        }
    }
    Ok(FileDiff::Submodule {
        old_oid,
        new_oid,
        dirty,
    })
}

/// Both sides' sizes in bytes; `None` for a side the file doesn't exist on, or that is a
/// gitlink (a commit in another repository, one `Subproject commit` line in the diff).
fn sizes(
    repo: &GitRepo,
    spec: &DiffSpec,
    entry: &FileEntry,
) -> Result<(Option<u64>, Option<u64>), DiffError> {
    let blob = |mode: Option<FileMode>, oid: &Option<Oid>| {
        mode.filter(|&mode| mode != FileMode::Submodule)
            .and(oid.clone())
    };
    let old_oid = blob(entry.old_mode, &entry.old_oid);
    let new_oid = blob(entry.new_mode, &entry.new_oid);
    let new_is_file = match entry.new_mode {
        Some(mode) => mode != FileMode::Submodule,
        None => entry.status == FileStatus::Untracked,
    };
    let blobs = blob_sizes(repo, old_oid.iter().chain(&new_oid))?;
    let old_len = old_oid.and_then(|oid| blobs.get(&oid).copied());
    let new_len = match new_oid.and_then(|oid| blobs.get(&oid).copied()) {
        Some(len) => Some(len),
        // A working tree file git hasn't hashed (an all-zero id), or hashed only in memory: rename
        // detection hashes every added and deleted candidate without writing the object, so
        // `cat-file` doesn't know the id `--raw` printed. The file on disk is that side.
        None if new_is_file && spec.new == Side::Worktree => {
            repo.backend().worktree_len(&entry.path)?
        }
        None => None,
    };
    Ok((old_len, new_len))
}

/// The sizes of the given blobs, from one `git cat-file --batch-check` call; a missing object is
/// left out.
fn blob_sizes<'a>(
    repo: &GitRepo,
    oids: impl Iterator<Item = &'a Oid>,
) -> Result<HashMap<Oid, u64>, DiffError> {
    let mut stdin = Vec::new();
    for oid in oids {
        stdin.extend_from_slice(oid.as_str().as_bytes());
        stdin.push(b'\n');
    }
    if stdin.is_empty() {
        return Ok(HashMap::new());
    }
    let args = ["cat-file", "--batch-check=%(objectname) %(objectsize)"].map(String::from);
    let output = git(repo, &args, Some(stdin))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut sizes = HashMap::new();
    for line in text.lines() {
        // `<oid> <size>`, or `<oid> missing`.
        if let Some((oid, size)) = line.split_once(' ')
            && let (Some(oid), Ok(size)) = (Oid::parse(oid), size.parse::<u64>())
        {
            sizes.insert(oid, size);
        }
    }
    Ok(sizes)
}
