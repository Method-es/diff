//! The file list (docs/PLAN.md §6 step 1): every changed file with its status and totals.
//!
//! One `git diff --raw --numstat -z -M` call yields both the statuses (with modes and object
//! ids, which the per-file diff needs for its size check and submodule detection) and the line
//! totals, from one look at the working tree, so the two can't disagree about a file an agent is
//! editing. It carries the same pinned flags as each file's diff ([`DIFF_PINS`]: Myers, no
//! textconv, submodules shown), so the totals match the hunks. Rename detection is on (`-M`,
//! which also overrides a configured `diff.renames=copies`) and copy detection off, matching
//! GitHub's file list. A working tree new side adds the untracked files
//! (`git ls-files --others --exclude-standard`), counted by reading them unless git converts
//! them first (a clean filter, a `working-tree-encoding`), in which case git counts them; an
//! untracked file the old side holds too (a round snapshot's, in an interdiff) is compared with
//! it through a temporary index instead ([`super::strays`]).

use std::collections::{HashMap, HashSet};
use std::iter::Peekable;

use super::{
    DIFF_PINS, DiffError, FileEntry, FileMode, FileStatus, no_index_path, side_args, strays,
};
use crate::backend::{GitOpts, GitOutput, TextStats};
use crate::git::{GitRepo, Oid};
use crate::target::{DiffSpec, Side};

/// Every file that differs between `spec`'s sides, sorted by path, each with its totals.
///
/// A working tree new side is the working tree as `git add -A` would stage it, untracked files
/// included. A file the old side holds that the working tree has untracked (an interdiff's
/// snapshot holds the untracked files of its round) is compared with the file on disk through
/// a temporary index ([`super::strays`]) and listed once, as modified when it changed, marked
/// [`FileEntry::new_untracked`].
pub fn file_list(repo: &GitRepo, spec: &DiffSpec) -> Result<Vec<FileEntry>, DiffError> {
    // The tracked and the untracked files are listed at once: two git processes that don't
    // wait on each other (PLAN §17's budgets).
    let (listed, others) = std::thread::scope(|scope| {
        let others = (spec.new == Side::Worktree).then(|| scope.spawn(|| untracked(repo)));
        let listed = tracked(repo, spec, None, None);
        let others = others.map(|job| {
            job.join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        });
        (listed, others)
    });
    let mut entries = listed?;
    if let Some(others) = others {
        let mut untracked = others?;
        let found = {
            let names: HashSet<&str> = untracked.iter().map(|entry| entry.path.as_str()).collect();
            strays::find(&entries, &names)
        };
        if !found.is_empty() {
            let index = strays::index(repo, &found)?;
            entries = tracked(repo, spec, Some(index.rel()), None)?;
            let paths: HashSet<&str> = found.iter().map(|stray| stray.path.as_str()).collect();
            for entry in &mut entries {
                entry.new_untracked =
                    entry.status != FileStatus::Deleted && paths.contains(entry.path.as_str());
            }
            untracked.retain(|entry| !paths.contains(entry.path.as_str()));
        }
        entries.extend(untracked);
    }
    // git prints tracked entries by path (unless `diff.orderFile` says otherwise); untracked
    // files join them in place.
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// What `git diff --raw --numstat` lists between `spec`'s sides, through the temp index
/// `index` (a store path) when given, of the one file at `path` when given.
pub(super) fn tracked(
    repo: &GitRepo,
    spec: &DiffSpec,
    index: Option<&str>,
    path: Option<&str>,
) -> Result<Vec<FileEntry>, DiffError> {
    let mut args: Vec<String> = ["diff", "--raw", "--numstat", "-z", "-M", "--no-abbrev"]
        .iter()
        .chain(DIFF_PINS)
        .map(|arg| arg.to_string())
        .collect();
    args.extend(side_args(repo, spec)?);
    if let Some(path) = path {
        // `:(literal)`: the path is a name, never a glob.
        args.extend(["--".to_string(), format!(":(literal){path}")]);
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut opts = GitOpts::new();
    opts.index_file = index.map(str::to_string);
    let output = repo.backend().git(&args, &opts)?;
    let (raw, numstat) =
        parse_raw_numstat(&output.stdout).map_err(|detail| DiffError::UnexpectedOutput {
            command: "diff --raw --numstat",
            detail,
        })?;
    join(raw, numstat).map_err(|detail| DiffError::UnexpectedOutput {
        command: "diff --raw --numstat",
        detail,
    })
}

/// One `--raw` record: `:<old mode> <new mode> <old oid> <new oid> <status>` and its path(s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawRecord {
    pub old_mode: Option<FileMode>,
    pub new_mode: Option<FileMode>,
    /// `None` for git's all-zero id: no file, or a working tree file git didn't hash.
    pub old_oid: Option<Oid>,
    pub new_oid: Option<Oid>,
    pub status: FileStatus,
    /// The source of a rename or copy.
    pub old_path: Option<String>,
    pub path: String,
}

/// One `--numstat` record; `None` counts mean binary (`-`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NumstatRecord {
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    pub old_path: Option<String>,
    pub path: String,
}

/// Splits `git diff --raw --numstat -z` output into its two record lists. git prints every raw
/// record first, then every numstat record, each list in the same order.
pub(crate) fn parse_raw_numstat(
    output: &[u8],
) -> Result<(Vec<RawRecord>, Vec<NumstatRecord>), String> {
    let mut tokens = output
        .split(|&b| b == 0)
        .map(|token| String::from_utf8_lossy(token).into_owned())
        .peekable();
    let mut raw = Vec::new();
    let mut numstat = Vec::new();
    while let Some(token) = tokens.next() {
        if token.is_empty() && tokens.peek().is_none() {
            break; // the terminator after the last record
        }
        if let Some(header) = token.strip_prefix(':') {
            raw.push(raw_record(header, &mut tokens)?);
        } else {
            numstat.push(numstat_record(&token, &mut tokens)?);
        }
    }
    Ok((raw, numstat))
}

fn raw_record(
    header: &str,
    tokens: &mut Peekable<impl Iterator<Item = String>>,
) -> Result<RawRecord, String> {
    let malformed = || format!("malformed raw record {header:?}");
    let fields: Vec<&str> = header.split(' ').collect();
    let [old_mode, new_mode, old_oid, new_oid, status] = fields[..] else {
        return Err(malformed());
    };
    let mode = |text: &str| FileMode::parse_octal(text).ok_or_else(malformed);
    let oid = |text: &str| {
        if text.bytes().all(|b| b == b'0') {
            Ok(None)
        } else {
            Oid::parse(text).map(Some).ok_or_else(malformed)
        }
    };
    let score = || -> Result<u8, String> {
        status[1..]
            .parse::<u8>()
            .ok()
            .filter(|&score| score <= 100)
            .ok_or_else(malformed)
    };
    let (status, paths) = match status.as_bytes().first() {
        Some(b'A') => (FileStatus::Added, 1),
        Some(b'M') => (FileStatus::Modified, 1),
        Some(b'D') => (FileStatus::Deleted, 1),
        Some(b'T') => (FileStatus::TypeChanged, 1),
        Some(b'U') => (FileStatus::Unmerged, 1),
        Some(b'R') => (
            FileStatus::Renamed {
                similarity: score()?,
            },
            2,
        ),
        Some(b'C') => (
            FileStatus::Copied {
                similarity: score()?,
            },
            2,
        ),
        _ => return Err(format!("unknown status in raw record {header:?}")),
    };
    let mut next_path = || {
        tokens
            .next()
            .filter(|path| !path.is_empty())
            .ok_or_else(|| format!("raw record {header:?} lacks its path"))
    };
    let old_path = if paths == 2 { Some(next_path()?) } else { None };
    Ok(RawRecord {
        old_mode: mode(old_mode)?,
        new_mode: mode(new_mode)?,
        old_oid: oid(old_oid)?,
        new_oid: oid(new_oid)?,
        status,
        old_path,
        path: next_path()?,
    })
}

fn numstat_record(
    token: &str,
    tokens: &mut Peekable<impl Iterator<Item = String>>,
) -> Result<NumstatRecord, String> {
    let malformed = || format!("malformed numstat record {token:?}");
    let mut fields = token.splitn(3, '\t');
    let (Some(additions), Some(deletions), Some(path)) =
        (fields.next(), fields.next(), fields.next())
    else {
        return Err(malformed());
    };
    let count = |text: &str| match text {
        "-" => Ok(None),
        text => text.parse::<u32>().map(Some).map_err(|_| malformed()),
    };
    // A rename or copy prints an empty path, then its source and destination as tokens.
    let (old_path, path) = if path.is_empty() {
        let mut next = || {
            tokens
                .next()
                .filter(|path| !path.is_empty())
                .ok_or_else(malformed)
        };
        let old = next()?;
        (Some(old), next()?)
    } else {
        (None, path.to_string())
    };
    Ok(NumstatRecord {
        additions: count(additions)?,
        deletions: count(deletions)?,
        old_path,
        path,
    })
}

/// Pairs each raw record with its numstat record. The numstat list is the raw list minus the
/// entries git found unchanged: a working tree file that is only stat-dirty shows in `--raw` as
/// modified with an unhashed new side but has no numstat, because the git policy keeps git from
/// refreshing the index (`diff.autoRefreshIndex=false`, PLAN §5.1); such an entry is dropped
/// (PLAN §6 step 1).
pub(crate) fn join(
    raw: Vec<RawRecord>,
    numstat: Vec<NumstatRecord>,
) -> Result<Vec<FileEntry>, String> {
    let mut stats = numstat.into_iter().peekable();
    let mut entries = Vec::with_capacity(raw.len());
    for record in raw {
        let stat =
            stats.next_if(|stat| stat.path == record.path && stat.old_path == record.old_path);
        let stat = match stat {
            Some(stat) => stat,
            None if record.status == FileStatus::Modified && record.new_oid.is_none() => continue,
            // Any other entry without numstat is still a change; it shows with zero totals.
            None => NumstatRecord {
                additions: Some(0),
                deletions: Some(0),
                old_path: None,
                path: String::new(),
            },
        };
        let binary = stat.additions.is_none() || stat.deletions.is_none();
        entries.push(FileEntry {
            path: record.path,
            old_path: record.old_path,
            status: record.status,
            additions: stat.additions.unwrap_or(0),
            deletions: stat.deletions.unwrap_or(0),
            binary,
            old_mode: record.old_mode,
            new_mode: record.new_mode,
            old_oid: record.old_oid,
            new_oid: record.new_oid,
            new_untracked: false,
        });
    }
    match stats.next() {
        Some(extra) => Err(format!(
            "--numstat lists {:?}, which --raw doesn't",
            extra.path
        )),
        None => Ok(entries),
    }
}

/// The untracked, not ignored files, each counted: lines read from the file, binary by the `diff`
/// attribute when it decides (`-diff`, `binary`, `diff`, a driver's `diff.<driver>.binary`) and
/// otherwise by git's own heuristic (a NUL in the first 8000 bytes). A file git converts before
/// it counts or looks for a NUL is counted by git instead: one whose `filter` driver has a clean
/// command (a git-lfs asset is a three-line text pointer to git, whatever its bytes), or one with
/// a `working-tree-encoding` (UTF-16, say). The same file staged is counted from its converted
/// content by `--numstat`, so this keeps the two states in agreement. A nested repository
/// (`ls-files` prints it as `dir/`) is listed as a submodule-like entry with one addition, as
/// `--numstat` counts the gitlink it becomes once staged (or snapshotted, PLAN §2.3).
fn untracked(repo: &GitRepo) -> Result<Vec<FileEntry>, DiffError> {
    untracked_at(repo, None)
}

/// [`untracked`]'s entries, of the one file at `path` when given (none when it isn't an
/// untracked, not ignored file).
pub(super) fn untracked_at(
    repo: &GitRepo,
    path: Option<&str>,
) -> Result<Vec<FileEntry>, DiffError> {
    others_at(repo, path, false)
}

/// The entry of the file at `path` when git ignores it, counted as [`untracked`] counts its
/// files: a definition a language server found under `node_modules` (PLAN §18.1).
pub(super) fn ignored_at(repo: &GitRepo, path: &str) -> Result<Vec<FileEntry>, DiffError> {
    others_at(repo, Some(path), true)
}

/// The files git doesn't track (`ls-files --others`): those it doesn't ignore, or with
/// `ignored` those it does; of the one file at `path` when given.
fn others_at(
    repo: &GitRepo,
    path: Option<&str>,
    ignored: bool,
) -> Result<Vec<FileEntry>, DiffError> {
    let mut args = ["ls-files", "--others", "--exclude-standard", "-z"]
        .map(String::from)
        .to_vec();
    if ignored {
        args.push("--ignored".to_string());
    }
    if let Some(path) = path {
        args.extend(["--".to_string(), format!(":(literal){path}")]);
    }
    let output = git(repo, &args, None)?;
    let paths: Vec<String> = output
        .stdout
        .split(|&b| b == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect();
    let (nested, files): (Vec<String>, Vec<String>) =
        paths.into_iter().partition(|path| path.ends_with('/'));
    let attributes = untracked_attributes(repo, &files)?;

    let mut entries = Vec::with_capacity(files.len() + nested.len());
    for (path, attributes) in files.into_iter().zip(attributes) {
        let stats = match repo.backend().worktree_text_stats(&path) {
            Ok(Some(stats)) => stats,
            // Removed since `ls-files` ran.
            Ok(None) => continue,
            // One unreadable file (a name the backend refuses, a permission error) shows with
            // zero totals instead of failing the whole list.
            Err(err) => {
                log::warn!("counting the lines of untracked {path:?}: {err}");
                TextStats::default()
            }
        };
        // A file git converts before counting (a clean filter, `working-tree-encoding`) is
        // counted by git; a symbolic link is its target path, which git never converts.
        let converted = attributes.converted && !stats.symlink;
        let git_counts = if converted && attributes.diff != DiffAttribute::Unset {
            converted_counts(repo, &path)
        } else {
            None
        };
        let binary = match (attributes.diff, git_counts) {
            (DiffAttribute::Unset, _) => true,
            (_, Some(counts)) => counts.is_none(),
            (DiffAttribute::Set, None) => false,
            (DiffAttribute::Auto, None) => stats.binary,
        };
        let lines = if binary {
            0
        } else if let Some(Some(lines)) = git_counts {
            u64::from(lines)
        } else if stats.binary {
            // Forced to text by the attribute despite a NUL byte, so the streaming count
            // stopped early; count the whole file.
            match repo.backend().worktree_read(&path) {
                Ok(Some(bytes)) => count_lines(&bytes),
                _ => 0,
            }
        } else {
            stats.lines
        };
        entries.push(FileEntry {
            path,
            old_path: None,
            status: FileStatus::Untracked,
            additions: u32::try_from(lines).unwrap_or(u32::MAX),
            deletions: 0,
            binary,
            old_mode: None,
            // git records no mode for an untracked file; a symbolic link is still told apart,
            // because its content is its target path.
            new_mode: stats.symlink.then_some(FileMode::Symlink),
            old_oid: None,
            new_oid: None,
            new_untracked: false,
        });
    }
    for path in nested {
        entries.push(FileEntry {
            path: path.trim_end_matches('/').to_string(),
            old_path: None,
            status: FileStatus::Untracked,
            additions: 1,
            deletions: 0,
            binary: false,
            old_mode: None,
            new_mode: Some(FileMode::Submodule),
            old_oid: None,
            new_oid: None,
            new_untracked: false,
        });
    }
    Ok(entries)
}

/// What a path's `diff` attribute says about diffing it as text, as git decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffAttribute {
    /// `-diff` (also through the `binary` macro), or a driver whose `diff.<driver>.binary` is
    /// true: always binary.
    Unset,
    /// `diff`, or a driver whose `diff.<driver>.binary` is false: always text.
    Set,
    /// Unspecified, or a driver that doesn't set `binary` (or isn't configured): git looks at
    /// the content.
    Auto,
}

/// The attributes that decide how an untracked file is counted (PLAN §6 step 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UntrackedAttributes {
    diff: DiffAttribute,
    /// git converts the file before it counts it or looks for a NUL: its `filter` driver has a
    /// clean command, or a `working-tree-encoding` is set.
    converted: bool,
}

/// The attributes of each path that decide how it is counted, in order: `diff`,
/// `working-tree-encoding` and `filter`, from one `git check-attr --stdin` call, plus one
/// `git config` call for the diff drivers' `binary` settings when a path names a diff driver
/// and one for the filter drivers' commands when a path names a filter driver.
fn untracked_attributes(
    repo: &GitRepo,
    paths: &[String],
) -> Result<Vec<UntrackedAttributes>, DiffError> {
    const NAMES: [&str; 3] = ["diff", "working-tree-encoding", "filter"];
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut stdin = Vec::new();
    for path in paths {
        stdin.extend_from_slice(path.as_bytes());
        stdin.push(0);
    }
    let mut args = ["check-attr", "-z", "--stdin"].map(String::from).to_vec();
    args.extend(NAMES.map(String::from));
    let output = git(repo, &args, Some(stdin))?;
    // `-z` output is `<path> NUL <attribute> NUL <value> NUL` per path and attribute: paths in
    // input order, each path's attributes in the order asked.
    let fields: Vec<&[u8]> = output.stdout.split(|&b| b == 0).collect();
    let unexpected = |detail: String| DiffError::UnexpectedOutput {
        command: "check-attr",
        detail,
    };
    let mut values = Vec::with_capacity(paths.len());
    for (i, path) in paths.iter().enumerate() {
        let mut path_values = [&b""[..]; NAMES.len()];
        for (j, (attribute, slot)) in NAMES.iter().zip(&mut path_values).enumerate() {
            let at = (i * NAMES.len() + j) * 3;
            let (Some(name), Some(reported), Some(value)) =
                (fields.get(at), fields.get(at + 1), fields.get(at + 2))
            else {
                return Err(unexpected(format!("no {attribute} attribute for {path:?}")));
            };
            if *name != path.as_bytes() || *reported != attribute.as_bytes() {
                return Err(unexpected(format!(
                    "expected {path:?} {attribute}, got {:?} {:?}",
                    String::from_utf8_lossy(name),
                    String::from_utf8_lossy(reported)
                )));
            }
            *slot = value;
        }
        values.push(path_values);
    }
    // A set, unset or unspecified `diff` or `filter` names no driver; git looks any other value
    // up as a driver name.
    let names_driver = |value: &[u8]| !matches!(value, b"set" | b"unset" | b"unspecified");
    let drivers = if values.iter().any(|[diff, ..]| names_driver(diff)) {
        driver_binary_settings(repo)?
    } else {
        HashMap::new()
    };
    let filters = if values.iter().any(|[.., filter]| names_driver(filter)) {
        filter_drivers(repo)?
    } else {
        HashSet::new()
    };
    Ok(values
        .into_iter()
        .map(|[diff, encoding, filter]| UntrackedAttributes {
            diff: match diff {
                b"unset" => DiffAttribute::Unset,
                b"set" => DiffAttribute::Set,
                b"unspecified" => DiffAttribute::Auto,
                driver => match drivers.get(String::from_utf8_lossy(driver).as_ref()) {
                    Some(true) => DiffAttribute::Unset,
                    Some(false) => DiffAttribute::Set,
                    None => DiffAttribute::Auto,
                },
            },
            converted: encoding != b"unspecified"
                || (names_driver(filter)
                    && filters.contains(String::from_utf8_lossy(filter).as_ref())),
        })
        .collect())
}

/// git's own count of an untracked file, from `git diff --no-index --numstat` with the pinned
/// flags: `Some(Some(lines))` for text, `Some(None)` for binary. Used for a file git converts
/// before diffing (through a clean filter or from a `working-tree-encoding`), whose bytes on
/// disk say nothing about either. `None` when git fails (a clean command that fails under
/// `filter.<driver>.required`, an encoding it can't convert, or a `working-tree-encoding` set
/// without a value, which git refuses); the file is then counted from its bytes.
fn converted_counts(repo: &GitRepo, path: &str) -> Option<Option<u32>> {
    let mut args: Vec<&str> = vec!["diff", "--no-index", "--numstat", "-z"];
    args.extend(DIFF_PINS);
    args.extend(["--", "/dev/null", no_index_path(path)]);
    // Exit code 1: the sides differ, which they do unless the file is empty.
    let output = repo
        .backend()
        .git(&args, &GitOpts::new().with_ok_codes(&[0, 1]));
    let output = match output {
        Ok(output) => output,
        Err(err) => {
            log::warn!("counting the lines of untracked {path:?} through git: {err}");
            return None;
        }
    };
    // `<added> TAB <removed> TAB <path> NUL`; no record at all for an empty file.
    let record = output.stdout.split(|&b| b == 0).next().unwrap_or_default();
    let added = record.split(|&b| b == b'\t').next().unwrap_or_default();
    match added {
        b"" => Some(Some(0)),
        b"-" => Some(None),
        digits => match std::str::from_utf8(digits).map(str::parse::<u32>) {
            Ok(Ok(lines)) => Some(Some(lines)),
            _ => {
                log::warn!(
                    "unexpected numstat for untracked {path:?}: {:?}",
                    String::from_utf8_lossy(record)
                );
                None
            }
        },
    }
}

/// Every diff driver's `diff.<driver>.binary` setting, by driver name, from one
/// `git config --get-regexp` call. git consults it before looking at the content: true makes
/// every file of that driver binary, false makes it text.
fn driver_binary_settings(repo: &GitRepo) -> Result<HashMap<String, bool>, DiffError> {
    // Exit 1: no driver sets `binary`. `--type=bool` prints each value as `true` or `false`.
    let output = repo.backend().git(
        &[
            "config",
            "--type=bool",
            "-z",
            "--get-regexp",
            r"^diff\..*\.binary$",
        ],
        &GitOpts::new().with_ok_codes(&[0, 1]),
    )?;
    let mut settings = HashMap::new();
    // `-z` prints `<key>\n<value>\0` per setting in the order git reads them, so a later one
    // (a repo's over the user's) replaces an earlier one, as in git. The driver name is the
    // key's middle, case and dots kept.
    for record in output.stdout.split(|&b| b == 0) {
        let record = String::from_utf8_lossy(record);
        if let Some((key, value)) = record.split_once('\n')
            && let Some(driver) = key
                .strip_prefix("diff.")
                .and_then(|rest| rest.strip_suffix(".binary"))
        {
            settings.insert(driver.to_string(), value == "true");
        }
    }
    Ok(settings)
}

/// The filter drivers git applies when it reads a working tree file: those whose
/// `filter.<driver>.clean` or `filter.<driver>.process` command is set and not empty, from one
/// `git config --get-regexp` call. git leaves a file whose `filter` attribute names any other
/// driver (git-lfs's `filter=lfs` where git-lfs isn't set up) as it is, or fails on it when the
/// driver is `required`; either way dv counts the bytes on disk without asking git, which saves
/// a git process per such file.
pub(super) fn filter_drivers(repo: &GitRepo) -> Result<HashSet<String>, DiffError> {
    // Exit 1: no driver has a command.
    let output = repo.backend().git(
        &[
            "config",
            "-z",
            "--get-regexp",
            r"^filter\..*\.(clean|process)$",
        ],
        &GitOpts::new().with_ok_codes(&[0, 1]),
    )?;
    // `-z` prints `<key>\n<value>\0` per setting (`<key>\0` for one without a value) in the
    // order git reads them, so a later one (a repo's empty `clean` over the user's) replaces an
    // earlier one, as in git. The driver name is the key's middle, case and dots kept.
    let mut commands: HashMap<(String, String), bool> = HashMap::new();
    for record in output.stdout.split(|&b| b == 0) {
        let record = String::from_utf8_lossy(record);
        let (key, value) = record.split_once('\n').unwrap_or((record.as_ref(), ""));
        if let Some((driver, kind)) = key
            .strip_prefix("filter.")
            .and_then(|rest| rest.rsplit_once('.'))
        {
            commands.insert((driver.to_string(), kind.to_string()), !value.is_empty());
        }
    }
    Ok(commands
        .into_iter()
        .filter(|&(_, set)| set)
        .map(|((driver, _), _)| driver)
        .collect())
}

/// Lines the way git counts them: one per `\n`, plus a final line without one.
fn count_lines(bytes: &[u8]) -> u64 {
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
    newlines + u64::from(bytes.last().is_some_and(|&b| b != b'\n'))
}

/// Runs `git <args>` through the repo's backend.
pub(super) fn git(
    repo: &GitRepo,
    args: &[String],
    stdin: Option<Vec<u8>>,
) -> Result<GitOutput, DiffError> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut opts = GitOpts::new();
    opts.stdin = stdin;
    Ok(repo.backend().git(&args, &opts)?)
}
