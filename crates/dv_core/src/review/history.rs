//! A thread's round history (docs/PLAN.md §8, "Round history"): what a closed round's snapshot
//! (`refs/dv/reviews/<id>/<n>`, the new side as it stood when round n closed, §2.3) says about a
//! thread anchored in that round, read when the current text alone can't place it.
//!
//! - **Following the file** ([`read_round_history`]): the interdiff since the round
//!   (`DiffSpec::since`, snapshot → now) says what each of the round's files is called now. An
//!   entry renamed from the anchor's path names it (git's rename detection). git can't pair a
//!   file the snapshot holds with an untracked one (dv lists untracked files apart, §6 step 1),
//!   and an agent's new files are untracked until committed, so a path the interdiff lists as
//!   deleted is paired with the most similar added or untracked file of that list
//!   ([`content_match`]: over [`PAIR_THRESHOLD`], git's default rename threshold, on the lines
//!   that say something, with at least [`PAIR_MIN_SHARED_LINES`] of them and
//!   [`PAIR_MIN_SHARED_CHARS`] characters shared, so two small unrelated files that share a
//!   `return 0;` and some braces never pair), the candidates ranked by same extension, then name
//!   likeness, then path, at most [`MAX_PAIR_READS`] of them read (each within the default
//!   `DiffLimits`); a tie goes to the candidate ranked first.
//! - **Where the code went** ([`trace`]): for a new-side anchor relocation calls outdated, the
//!   anchor is found in the snapshot's text, and those lines are followed through a line diff of
//!   the snapshot's text against the text now: [`Went::Replaced`] at the lines that replaced
//!   them, or [`Went::Removed`] where they stood.
//!
//! Reading is blocking, over [`GitRepo`] (so through any backend, the WSL one included, exactly
//! as `file_list` and `file_diff` run); [`super::place_thread`] stays pure and takes what was
//! read as a [`History`].

use std::collections::{BTreeMap, BTreeSet, HashMap};

use imara_diff::{Algorithm, Diff, InternedInput};

use super::anchor::side_lines;
use super::model::{Anchor, AnchorSide, Round};
use super::relocate::{REWRITE_SLACK, Relocation, line_similarity, relocate};
use crate::backend::BackendError;
use crate::diff::{DiffError, DiffLimits, FileEntry, FileStatus, Tokens, file_diff, file_list};
use crate::git::{GitError, GitRepo};
use crate::target::DiffSpec;

/// What a deleted file's pairing with an added or untracked one must score more than
/// ([`content_similarity`]): git's default rename threshold (50%).
pub const PAIR_THRESHOLD: f64 = 0.5;

/// The least number of substantive lines ([`is_substantive`]) a pairing's two texts share.
pub const PAIR_MIN_SHARED_LINES: usize = 3;

/// The least number of characters (trimmed) the substantive lines a pairing's two texts share
/// add up to.
pub const PAIR_MIN_SHARED_CHARS: usize = 60;

/// A line says something, for pairing, when it has at least this many alphanumeric characters
/// once trimmed: `}`, `);` and `*/` don't.
pub const SUBSTANTIVE_CHARS: usize = 3;

/// The most candidates one deleted file's pairing reads.
pub const MAX_PAIR_READS: usize = 30;

/// Scores within this of each other are equal.
const EPSILON: f64 = 1e-9;

/// Where a thread's code went since its round ([`trace`]), on the new side now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trail {
    /// The round whose snapshot the anchor was found in: "code changed since round N".
    pub round: u32,
    pub went: Went,
}

/// What became of a thread's anchored lines since its round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Went {
    /// Lines `start_line..=end_line` (1-based) replaced them.
    Replaced { start_line: u32, end_line: u32 },
    /// Nothing replaced them: they stood before line `before_line` (1 at the top, one past the
    /// last line at the end).
    Removed { before_line: u32 },
}

impl Went {
    /// `"replaced"` or `"removed"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Replaced { .. } => "replaced",
            Self::Removed { .. } => "removed",
        }
    }
}

impl Trail {
    /// The line a card drawn where the code went follows (DESIGN §4.1): the last replacing
    /// line, or the line before where removed lines stood; `None` when they stood at the top,
    /// where the card heads its file.
    pub fn after_line(&self) -> Option<u32> {
        match self.went {
            Went::Replaced { end_line, .. } => Some(end_line),
            Went::Removed { before_line } => before_line.checked_sub(1).filter(|&line| line > 0),
        }
    }

    /// The lines it names, `(start, end)`: the replacing lines, or for removed lines the line
    /// they now follow (line 1 when they stood at the top).
    pub fn lines(&self) -> (u32, u32) {
        match self.went {
            Went::Replaced {
                start_line,
                end_line,
            } => (start_line, end_line),
            Went::Removed { .. } => {
                let line = self.after_line().unwrap_or(1);
                (line, line)
            }
        }
    }
}

/// What a closed round's snapshot says about one of its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHistory {
    /// The path the file has now: its own (changed since or not), the one git or content
    /// similarity pairs it with, or `None` when it was deleted since.
    pub now: Option<String>,
    /// The file's text in the snapshot, line 1 first, when the trace has something to read:
    /// `None` for a file unchanged since the round (its text now is the same), one the snapshot
    /// doesn't hold, or one without text there (binary, a submodule).
    pub snapshot: Option<Vec<String>>,
}

/// What one closed round's snapshot says about the files some threads anchored in that round
/// need ([`read_round_history`]), by the path their anchors name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoundHistory {
    pub round: u32,
    files: BTreeMap<String, FileHistory>,
    /// Every path whose text was read: the files themselves, what they are called now, and the
    /// candidates a deleted file was compared with. A change to one of them can change what
    /// this history says.
    read: BTreeSet<String>,
    /// The snapshot's texts read, by path (`None`: no text there). A snapshot never changes,
    /// so a later read of the same round takes them from here ([`read_round_history`]).
    snapshots: BTreeMap<String, Option<Vec<String>>>,
}

impl RoundHistory {
    /// A history of `round` that holds nothing yet.
    pub fn new(round: u32) -> Self {
        Self {
            round,
            ..Self::default()
        }
    }

    /// What the round says about `path` (an anchor's path), when it was read for it.
    pub fn file(&self, path: &str) -> Option<&FileHistory> {
        self.files.get(path)
    }

    /// Records what the round says about `path`.
    pub fn insert(&mut self, path: impl Into<String>, file: FileHistory) {
        self.files.insert(path.into(), file);
    }

    /// The paths it was read for.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// Every path whose text was read ([`RoundHistory::read`]).
    pub fn read_paths(&self) -> &BTreeSet<String> {
        &self.read
    }
}

/// A thread's round history as [`super::place_thread`]'s caller has it: the history of the
/// round its anchor was taken in.
#[derive(Debug, Clone, Copy)]
pub enum History<'a> {
    /// No history to read: the anchor's round is still in progress, the anchor is on the old
    /// side (a snapshot is a new side), or reading it failed. Placement goes on without it.
    None,
    /// Not read yet: a thread that needs it waits (`Placement::Pending`).
    Pending,
    /// Read; a path it wasn't read for counts as not read yet.
    Read(&'a RoundHistory),
}

/// What a [`History`] says about one path.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PathHistory<'a> {
    None,
    Pending,
    Read(&'a FileHistory),
}

impl<'a> History<'a> {
    pub(crate) fn of(self, path: &str) -> PathHistory<'a> {
        match self {
            Self::None => PathHistory::None,
            Self::Pending => PathHistory::Pending,
            Self::Read(round) => round
                .file(path)
                .map_or(PathHistory::Pending, PathHistory::Read),
        }
    }
}

/// Reads what round `round` of a review whose target resolves to `resolved` says about `paths`
/// (anchors' paths), blocking (module docs): one file list of the interdiff since the round, the
/// snapshot's text of each path changed since, and for a path deleted since, the text of the
/// candidates it is paired with. The snapshot's texts `previous` (an earlier read of the same
/// round) holds aren't read again. A round whose snapshot is missing from the repository says
/// nothing: every path is as it stands now.
pub fn read_round_history(
    repo: &GitRepo,
    resolved: &DiffSpec,
    round: &Round,
    paths: &[String],
    previous: Option<&RoundHistory>,
) -> Result<RoundHistory, DiffError> {
    let mut history = RoundHistory::new(round.n);
    if let Some(previous) = previous.filter(|previous| previous.round == round.n) {
        history.snapshots = previous.snapshots.clone();
    }
    if !repo.has_commit(&round.new_oid)? {
        log::info!(
            "round {}'s snapshot ({}) is missing: its threads are placed without it",
            round.n,
            round.new_oid.short()
        );
        for path in paths {
            history.insert(path.clone(), unchanged(path));
        }
        return Ok(history);
    }
    let since = resolved.since(round);
    let entries = file_list(repo, &since)?;
    for path in paths {
        let file = follow(repo, &since, &entries, path, &mut history)?;
        history.insert(path.clone(), file);
    }
    Ok(history)
}

/// A file the round's history says nothing new about: it stands at its path as it did.
fn unchanged(path: &str) -> FileHistory {
    FileHistory {
        now: Some(path.to_string()),
        snapshot: None,
    }
}

/// What the interdiff since the round, `entries` (listed with `since`), says about `path`.
fn follow(
    repo: &GitRepo,
    since: &DiffSpec,
    entries: &[FileEntry],
    path: &str,
    history: &mut RoundHistory,
) -> Result<FileHistory, DiffError> {
    if let Some(renamed) = entries.iter().find(|entry| {
        entry.old_path.as_deref() == Some(path)
            && matches!(entry.status, FileStatus::Renamed { .. })
    }) {
        history.read.insert(path.to_string());
        history.read.insert(renamed.path.clone());
        return Ok(FileHistory {
            now: Some(renamed.path.clone()),
            snapshot: snapshot_text(repo, since, renamed, path, history)?,
        });
    }
    let Some(entry) = entries.iter().find(|entry| entry.path == path) else {
        // Not changed since the round (or in neither the snapshot nor now).
        return Ok(unchanged(path));
    };
    match entry.status {
        // The snapshot didn't hold it at that path.
        FileStatus::Added
        | FileStatus::Untracked
        | FileStatus::Renamed { .. }
        | FileStatus::Copied { .. } => Ok(unchanged(path)),
        FileStatus::Deleted => {
            history.read.insert(path.to_string());
            let snapshot = snapshot_text(repo, since, entry, path, history)?;
            let now = match &snapshot {
                Some(text) => pair(repo, since, entries, path, text, &mut history.read)?,
                None => None,
            };
            Ok(FileHistory { now, snapshot })
        }
        FileStatus::Modified | FileStatus::TypeChanged | FileStatus::Unmerged => {
            history.read.insert(path.to_string());
            Ok(FileHistory {
                now: Some(path.to_string()),
                snapshot: snapshot_text(repo, since, entry, path, history)?,
            })
        }
    }
}

/// The snapshot's text of `path`, the old side of `entry` in the interdiff since the round, read
/// in full unless `history` holds it already; `None` when it has no text there.
fn snapshot_text(
    repo: &GitRepo,
    since: &DiffSpec,
    entry: &FileEntry,
    path: &str,
    history: &mut RoundHistory,
) -> Result<Option<Vec<String>>, DiffError> {
    if let Some(text) = history.snapshots.get(path) {
        return Ok(text.clone());
    }
    let diff = file_diff(repo, since, entry, &DiffLimits::UNLIMITED)?;
    let text = diff
        .as_text()
        .map(|text| owned(side_lines(text, AnchorSide::Old)));
    history.snapshots.insert(path.to_string(), text.clone());
    Ok(text)
}

fn owned(lines: Vec<&str>) -> Vec<String> {
    lines.into_iter().map(str::to_string).collect()
}

/// The added or untracked file of `entries` that `path`, deleted since the round with `snapshot`
/// as its text then, became (module docs); `None` when none is similar enough.
fn pair(
    repo: &GitRepo,
    since: &DiffSpec,
    entries: &[FileEntry],
    path: &str,
    snapshot: &[String],
    read: &mut BTreeSet<String>,
) -> Result<Option<String>, DiffError> {
    let limits = DiffLimits::default();
    let fits = |count: u32| usize::try_from(count).is_ok_and(|count| count <= limits.max_lines);
    let mut candidates: Vec<(&FileEntry, bool, f64)> = entries
        .iter()
        .filter(|entry| {
            matches!(entry.status, FileStatus::Added | FileStatus::Untracked)
                && !entry.binary
                && !entry.is_submodule()
                && fits(entry.additions)
        })
        .map(|entry| {
            let same_extension = extension(&entry.path) == extension(path);
            let likeness = line_similarity(file_name(&entry.path), file_name(path));
            (entry, same_extension, likeness)
        })
        .collect();
    candidates.sort_by(|(a, a_ext, a_like), (b, b_ext, b_like)| {
        b_ext
            .cmp(a_ext)
            .then_with(|| b_like.total_cmp(a_like))
            .then_with(|| a.path.cmp(&b.path))
    });
    let snapshot = Bag::new(snapshot);
    let mut best: Option<(f64, &str)> = None;
    for (entry, ..) in candidates.into_iter().take(MAX_PAIR_READS) {
        let diff = match file_diff(repo, since, entry, &limits) {
            Ok(diff) => diff,
            // One candidate that can't be read (removed since it was listed) is passed over.
            Err(DiffError::Git(GitError::Backend(
                err @ (BackendError::Git { .. } | BackendError::UnexpectedOutput { .. }),
            ))) => {
                log::warn!("reading {:?} to pair it with {path:?}: {err}", entry.path);
                continue;
            }
            Err(err) => return Err(err),
        };
        read.insert(entry.path.clone());
        let Some(text) = diff.as_text() else {
            continue;
        };
        let Some(score) = snapshot.matches(&Bag::new(&side_lines(text, AnchorSide::New))) else {
            continue;
        };
        if best.is_none_or(|(kept, _)| score > kept + EPSILON) {
            best = Some((score, &entry.path));
        }
    }
    Ok(best.map(|(_, path)| path.to_string()))
}

/// A path's last component.
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// A path's extension (what follows the last `.` of its name, not a leading one), if any.
fn extension(path: &str) -> Option<&str> {
    let name = file_name(path);
    name.rfind('.')
        .filter(|&dot| dot > 0)
        .map(|dot| &name[dot + 1..])
}

/// Whether `line` (trimmed) says something, for pairing: at least [`SUBSTANTIVE_CHARS`]
/// alphanumeric characters. A brace, `);` or a lone keyword's punctuation doesn't.
pub fn is_substantive(line: &str) -> bool {
    line.trim()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .nth(SUBSTANTIVE_CHARS - 1)
        .is_some()
}

/// A text's substantive lines ([`is_substantive`]), trimmed, with how often each stands in it,
/// and their weight: each line weighs its length in characters.
struct Bag<'a> {
    counts: HashMap<&'a str, usize>,
    weight: usize,
}

/// What two texts share ([`Bag::shared`]).
struct Shared {
    lines: usize,
    weight: usize,
}

impl<'a> Bag<'a> {
    fn new<S: AsRef<str>>(lines: &'a [S]) -> Self {
        let mut counts = HashMap::new();
        let mut weight = 0;
        for line in lines {
            let line = line.as_ref().trim();
            if is_substantive(line) {
                *counts.entry(line).or_insert(0) += 1;
                weight += line.chars().count();
            }
        }
        Self { counts, weight }
    }

    fn shared(&self, other: &Bag<'_>) -> Shared {
        let mut shared = Shared {
            lines: 0,
            weight: 0,
        };
        for (line, &count) in &self.counts {
            let both = count.min(other.counts.get(line).copied().unwrap_or(0));
            shared.lines += both;
            shared.weight += both * line.chars().count();
        }
        shared
    }

    fn similarity(&self, other: &Bag<'_>) -> f64 {
        let total = self.weight + other.weight;
        if total == 0 {
            return 0.0;
        }
        2.0 * self.shared(other).weight as f64 / total as f64
    }

    /// [`content_match`] of the two texts.
    fn matches(&self, other: &Bag<'_>) -> Option<f64> {
        let shared = self.shared(other);
        let score = self.similarity(other);
        (shared.lines >= PAIR_MIN_SHARED_LINES
            && shared.weight >= PAIR_MIN_SHARED_CHARS
            && score > PAIR_THRESHOLD + EPSILON)
            .then_some(score)
    }
}

/// How alike two texts are, from 0 to 1: the Dice coefficient of their substantive lines
/// ([`is_substantive`], trimmed), counted as a multiset and each weighed by its length (twice
/// the weight of the lines they share, repeats included, over the weight of the lines both
/// have). Two texts without such lines are 0: nothing says they are one file.
pub fn content_similarity<S: AsRef<str>, T: AsRef<str>>(a: &[S], b: &[T]) -> f64 {
    Bag::new(a).similarity(&Bag::new(b))
}

/// Whether two texts are one file, for pairing (module docs): their [`content_similarity`] is
/// over [`PAIR_THRESHOLD`], and they share at least [`PAIR_MIN_SHARED_LINES`] substantive lines
/// adding up to at least [`PAIR_MIN_SHARED_CHARS`] characters. The score when they are.
pub fn content_match<S: AsRef<str>, T: AsRef<str>>(a: &[S], b: &[T]) -> Option<f64> {
    Bag::new(a).matches(&Bag::new(b))
}

/// Where a new-side `anchor`'s code went between `snapshot`, its file's text when its round
/// closed, and `now`, the file's text now (module docs): the anchor is found in `snapshot`
/// ([`relocate`]: at its lines, or moved), then those lines are followed through a line diff of
/// `snapshot` against `now`. The change blocks that overlap them (a pure insertion inside
/// them counts only when it falls between two such blocks) give the lines that replaced them:
/// the new side of those blocks and the unchanged lines between, cut at the anchored count plus
/// [`REWRITE_SLACK`] lines from its start, so a file rewritten whole doesn't point far down it.
/// When every such block replaces its lines with nothing, they were removed:
/// [`Went::Removed`], where the first of them stood (never their own lines left standing).
/// `None` for an old-side anchor, one not found in `snapshot` either, or lines no change block
/// overlaps.
pub fn trace<S: AsRef<str>, T: AsRef<str>>(
    anchor: &Anchor,
    snapshot: &[S],
    now: &[T],
) -> Option<Went> {
    if anchor.side != AnchorSide::New {
        return None;
    }
    let (start, end) = match relocate(anchor, snapshot) {
        Relocation::Unchanged => (anchor.start_line, anchor.end_line),
        Relocation::Moved {
            start_line,
            end_line,
            ..
        } => (start_line, end_line),
        Relocation::Outdated => return None,
    };
    // 0-based: the anchored lines are `first..past` of `snapshot`.
    let (first, past) = ((start as usize).checked_sub(1)?, end as usize);
    let before: Vec<&str> = snapshot.iter().map(AsRef::as_ref).collect();
    let after: Vec<&str> = now.iter().map(AsRef::as_ref).collect();
    let input = InternedInput::new(Tokens(&before), Tokens(&after));
    let diff = Diff::compute(Algorithm::Myers, &input);
    let hunks: Vec<_> = diff.hunks().collect();
    // The blocks that change anchored lines, and the insertions that stand between two of them.
    let changing: Vec<_> = hunks
        .iter()
        .filter(|hunk| {
            let (from, to) = (hunk.before.start as usize, hunk.before.end as usize);
            from < to && from < past && first < to
        })
        .collect();
    let (head, tail) = (changing.first()?, changing.last()?);
    let between = |hunk: &&imara_diff::Hunk| {
        hunk.before.is_empty()
            && head.before.end <= hunk.before.start
            && hunk.before.start <= tail.before.start
    };
    let replacing = changing.iter().any(|hunk| !hunk.after.is_empty())
        || hunks
            .iter()
            .filter(between)
            .any(|hunk| !hunk.after.is_empty());
    if !replacing {
        return Some(Went::Removed {
            before_line: line_number(head.after.start as usize),
        });
    }
    let (from, to) = (head.after.start as usize, tail.after.end as usize);
    let to = to.min(from + (past - first) + REWRITE_SLACK);
    Some(Went::Replaced {
        start_line: line_number(from),
        end_line: line_number(to - 1),
    })
}

/// A 0-based line index as a 1-based line number.
fn line_number(index: usize) -> u32 {
    u32::try_from(index + 1).unwrap_or(u32::MAX)
}
