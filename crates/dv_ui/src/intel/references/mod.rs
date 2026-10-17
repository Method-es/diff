//! The References tab (PLAN §18.1 "Find references", DESIGN §4.1 "References"): a symbol's
//! references, or a go to definition's several places, grouped by file, each row its line's
//! text highlighted with the symbol marked.
//!
//! **The rows.** The places come from the server and show at once, grouped by file: the files
//! the review changes first, then the rest, each group in path order, the count known from the
//! start. Their lines are read in the background as the server read them ([`read_lines`]): a
//! worktree file from the working tree and a file outside the worktree through
//! `Backend::probe_path` (up to its read cap), [`READERS`] files at a time, each file's text
//! landing as it is read; the file asked from, when the server read it from the text dv opened
//! for another side (an old side, the index), from the text the request sent, so its rows are
//! that side's lines. Once
//! every file is read, the rows' lines are highlighted, only those lines, each on its own so
//! nothing one opens (a block comment) colours another (`crate::syntax::LineHighlighter`, one
//! grammar's queries compiled once), and their tokens land.
//!
//! **Changed rows.** A row on a line the review changed carries the changed marker: its line is
//! an added line of the review's new side with the same text (a removed line of the old side,
//! for rows read on the old side), as the shown comparison has it (in an interdiff, changed
//! since the round). So a row read against the working tree on a review whose new side isn't
//! it is marked only where the two agree; **In this review** keeps only marked rows. Which rows
//! are marked is worked out when a file's text lands and when the session loads its diff
//! ([`References::refresh_changed`]), never per frame. The review's diffs of the files the rows
//! are in load for it (`ReviewSession::load_for_references`), whatever the stream's viewport.
//!
//! **The view** ([`ReferencesPanel`], `render`) is a virtualised list of the files' headers and
//! rows; a click on a row goes where go to definition goes ([`super::IntelEvent`]).
//!
//! Automation targets: `references` (the tab's body), `references/close`,
//! `references/in_review`, `references/row/<path>/<line>` (the first row on that line, while
//! it is drawn).

mod render;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use dv_core::Backend;
use dv_core::backend::PathProbe;
use dv_core::diff::LineKind;
use dv_core::review::AnchorSide;
use dv_lsp::{SymbolLocation, UriTarget};
use futures::channel::mpsc::UnboundedSender;
use gpui::Task;

pub use render::{ReferencesPanel, runs};

use super::SymbolAt;
use crate::diff_pane::rows::side_number;
use crate::session::ReviewSession;
use crate::syntax::{LineHighlighter, Token};

/// How many files' lines are read at once (each read is a call to the backend, a WSL one's a
/// round trip to the distro).
pub const READERS: usize = 8;

/// What the list holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferencesKind {
    References,
    /// A go to definition that found several places.
    Definitions,
}

impl ReferencesKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::References => "References",
            Self::Definitions => "Definitions",
        }
    }

    /// `"references"` or `"definitions"`, as automation state spells it.
    pub fn key(self) -> &'static str {
        match self {
            Self::References => "references",
            Self::Definitions => "definitions",
        }
    }
}

/// One symbol's references (module docs).
pub struct References {
    pub kind: ReferencesKind,
    pub from: SymbolAt,
    /// The pane it was asked from.
    pub pane: super::Pane,
    /// The server read the asking file's text in place of the disk's (PLAN §18.2).
    pub against_worktree: bool,
    /// **In this review**: only the rows on lines the review changed.
    pub in_review: bool,
    /// The places, by file, once the server answered.
    pub files: Option<Vec<RefFile>>,
    /// Why there are none to show.
    pub error: Option<String>,
    /// Tells this list from the one before (the panel lays its rows out again).
    pub(super) id: u64,
    /// Bumped whenever which rows show may change (a file's changed rows worked out again).
    pub(super) shape: u64,
    /// The rows' lines being read and highlighted.
    pub(super) reading: Option<Task<()>>,
}

impl References {
    pub(super) fn loading(
        kind: ReferencesKind,
        from: SymbolAt,
        pane: super::Pane,
        against_worktree: bool,
        in_review: bool,
        id: u64,
    ) -> Self {
        Self {
            kind,
            from,
            pane,
            against_worktree,
            in_review,
            files: None,
            error: None,
            id,
            shape: 0,
            reading: None,
        }
    }

    /// The server hasn't answered.
    pub fn is_loading(&self) -> bool {
        self.files.is_none() && self.error.is_none()
    }

    /// The rows' lines are still being read or highlighted.
    pub fn is_reading(&self) -> bool {
        self.reading.is_some()
    }

    /// Every place found.
    pub fn count(&self) -> usize {
        self.files
            .iter()
            .flatten()
            .map(|file| file.rows.len())
            .sum()
    }

    /// The worktree files the places are in.
    pub(super) fn worktree_paths(&self) -> Vec<String> {
        self.files
            .iter()
            .flatten()
            .filter_map(|file| file.worktree_path().map(str::to_string))
            .collect()
    }

    /// The rows shown, by file, with whether the review changed each (module docs): every row,
    /// or with **In this review** only the changed ones (and only files that keep a row).
    pub fn shown(&self) -> Vec<(&RefFile, Vec<(&RefRow, bool)>)> {
        self.files
            .iter()
            .flatten()
            .filter_map(|file| {
                let rows: Vec<(&RefRow, bool)> = file
                    .rows
                    .iter()
                    .map(|row| (row, file.changed.contains(&row.line)))
                    .filter(|(_, changed)| !self.in_review || *changed)
                    .collect();
                (!rows.is_empty()).then_some((file, rows))
            })
            .collect()
    }

    /// Works out again which rows of `path`'s file (every file's, for `None`) are on lines the
    /// review changed, from the diff the session shows now: its diff landed, or the rows'
    /// text did.
    pub(super) fn refresh_changed(&mut self, path: Option<&str>, session: &ReviewSession) {
        let mut moved = false;
        for file in self.files.iter_mut().flatten() {
            if path.is_some_and(|path| file.worktree_path() != Some(path)) {
                continue;
            }
            let changed = changed_lines(file, session);
            if changed != file.changed {
                file.changed = changed;
                moved = true;
            }
        }
        if moved {
            self.shape += 1;
        }
    }

    /// Whether `path` is one of the list's worktree files.
    pub(super) fn has_file(&self, path: &str) -> bool {
        self.files
            .iter()
            .flatten()
            .any(|file| file.worktree_path() == Some(path))
    }

    /// File `index`'s lines landed ([`read_lines`]): `texts` has one per row.
    pub(super) fn text_landed(
        &mut self,
        index: usize,
        texts: Vec<Option<String>>,
        session: &ReviewSession,
    ) {
        let Some(file) = self.files.as_mut().and_then(|files| files.get_mut(index)) else {
            return;
        };
        for (row, text) in file.rows.iter_mut().zip(texts) {
            row.text = text;
        }
        file.read = true;
        let changed = changed_lines(file, session);
        if changed != file.changed {
            file.changed = changed;
            self.shape += 1;
        }
    }

    /// File `index`'s rows' tokens landed: one list per row.
    pub(super) fn tokens_landed(&mut self, index: usize, tokens: Vec<Vec<Token>>) {
        if let Some(file) = self.files.as_mut().and_then(|files| files.get_mut(index)) {
            for (row, tokens) in file.rows.iter_mut().zip(tokens) {
                row.tokens = tokens;
            }
        }
    }
}

/// One file's places.
pub struct RefFile {
    pub target: UriTarget,
    /// The side its rows were read on: the asking file's, when the server read the text dv
    /// opened for that side (module docs); `None` for a file read as the working tree holds it
    /// (or outside the worktree).
    pub side: Option<AnchorSide>,
    /// The review changes it (all changes): it lists first.
    pub in_review: bool,
    pub rows: Vec<RefRow>,
    /// Its rows' lines are there (read, or read and found missing).
    pub read: bool,
    /// The lines of its rows the review changed (module docs).
    pub changed: HashSet<u32>,
}

impl RefFile {
    /// Its path as the list shows it: worktree-relative, else the path on the backend's
    /// machine (or the URI, for one that isn't a file).
    pub fn label(&self) -> &str {
        match &self.target {
            UriTarget::Worktree(path) => path,
            UriTarget::Outside { uri, path } => path.as_deref().unwrap_or(uri),
        }
    }

    /// The worktree path, for a file inside the worktree.
    pub fn worktree_path(&self) -> Option<&str> {
        match &self.target {
            UriTarget::Worktree(path) => Some(path),
            UriTarget::Outside { .. } => None,
        }
    }
}

/// One place.
pub struct RefRow {
    pub location: SymbolLocation,
    /// Its line, 1-based.
    pub line: u32,
    /// The line's text, once read (`None` meanwhile, and when it couldn't be read).
    pub text: Option<String>,
    /// Its tokens, once highlighted, when the file has a grammar.
    pub tokens: Vec<Token>,
}

impl RefRow {
    /// The symbol's bytes in the line's text, on character boundaries (a server's columns can
    /// fall inside a character when its text isn't quite the one read here); a span over
    /// several lines ends at the line's end.
    pub fn symbol(&self) -> Range<usize> {
        let text = self.text.as_deref().unwrap_or("");
        let range = &self.location.range;
        let start = floor_boundary(text, range.start.column as usize);
        let end = if range.end.line == range.start.line {
            floor_boundary(text, (range.end.column as usize).max(start))
        } else {
            text.len()
        };
        start..end.max(start)
    }
}

/// `at` within `text` (its length past the end), moved back to a character boundary.
pub fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// `locations` grouped by file (module docs, "The rows"): the files the review changes first,
/// then the rest, each group in path order; each file's places in order, without repeats, their
/// lines not read yet, but for the asking file read from `own` (the side and the text the
/// server read in place of the disk's) when it was.
pub(super) fn group(
    locations: Vec<SymbolLocation>,
    from: &SymbolAt,
    own: Option<(AnchorSide, Arc<str>)>,
    session: &ReviewSession,
) -> Vec<RefFile> {
    let mut by_file: BTreeMap<UriTarget, Vec<SymbolLocation>> = BTreeMap::new();
    for location in locations {
        by_file
            .entry(location.target.clone())
            .or_default()
            .push(location);
    }
    let mut files: Vec<RefFile> = by_file
        .into_iter()
        .map(|(target, mut locations)| {
            locations.sort_by_key(|location| location.range);
            locations.dedup();
            let rows: Vec<RefRow> = locations
                .into_iter()
                .map(|location| RefRow {
                    line: location.range.start.line + 1,
                    location,
                    text: None,
                    tokens: Vec::new(),
                })
                .collect();
            let in_review = match &target {
                UriTarget::Worktree(path) => session.in_review(path),
                UriTarget::Outside { .. } => false,
            };
            let mut file = RefFile {
                target,
                side: None,
                in_review,
                rows,
                read: false,
                changed: HashSet::new(),
            };
            if let Some((side, text)) = &own
                && file.worktree_path() == Some(from.path.as_str())
            {
                let lines = split_lines(text);
                for row in &mut file.rows {
                    row.text = lines
                        .get(row.location.range.start.line as usize)
                        .map(|line| line.to_string());
                }
                file.side = Some(*side);
                file.read = true;
            }
            file.changed = changed_lines(&file, session);
            file
        })
        .collect();
    // Stable: path order within each group.
    files.sort_by_key(|file| !file.in_review);
    files
}

/// `text`'s lines without their line ends.
fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

/// The lines of `file`'s rows the review changed, with the text the row shows (module docs,
/// "Changed rows").
fn changed_lines(file: &RefFile, session: &ReviewSession) -> HashSet<u32> {
    let Some(path) = file.worktree_path() else {
        return HashSet::new();
    };
    let Some(text) = session
        .file(path)
        .and(session.shown_diff(path))
        .and_then(|diff| diff.as_text())
    else {
        return HashSet::new();
    };
    let side = file.side.unwrap_or(AnchorSide::New);
    let kind = match side {
        AnchorSide::New => LineKind::Added,
        AnchorSide::Old => LineKind::Removed,
    };
    let rows: HashMap<u32, Option<&str>> = file
        .rows
        .iter()
        .map(|row| (row.line, row.text.as_deref()))
        .collect();
    text.lines
        .iter()
        .filter(|line| line.kind == kind)
        .filter_map(|line| {
            let number = side_number(line, side)?;
            (rows.get(&number).copied().flatten() == Some(line.text.as_str())).then_some(number)
        })
        .collect()
}

/// What [`read_lines`] reports as it goes.
pub(super) enum Landed {
    /// File `index`'s lines, one per row (`None` for one it couldn't read).
    Text {
        index: usize,
        texts: Vec<Option<String>>,
    },
    /// File `index`'s rows' tokens, one list per row.
    Tokens {
        index: usize,
        tokens: Vec<Vec<Token>>,
    },
}

/// What one file of the list needs read: its index, where it is, its rows' lines (0-based),
/// and its rows' text when that is known already (the asking file's other side).
pub(super) struct ReadJob {
    pub index: usize,
    pub target: UriTarget,
    pub lines: Vec<u32>,
    pub known: Option<Vec<Option<String>>>,
}

impl ReadJob {
    /// What reading `files` takes.
    pub fn for_files(files: &[RefFile]) -> Vec<ReadJob> {
        files
            .iter()
            .enumerate()
            .map(|(index, file)| ReadJob {
                index,
                target: file.target.clone(),
                lines: file
                    .rows
                    .iter()
                    .map(|row| row.location.range.start.line)
                    .collect(),
                known: file
                    .read
                    .then(|| file.rows.iter().map(|row| row.text.clone()).collect()),
            })
            .collect()
    }
}

/// Reads the rows' lines of `jobs`' files, [`READERS`] at a time, sending each file's as it
/// lands, then highlights every row's line, one highlighter per language, and sends the tokens
/// (module docs). Blocking: runs on a background thread; stops early once `send` has no
/// receiver (the list went).
pub(super) fn read_lines(backend: &dyn Backend, jobs: Vec<ReadJob>, send: UnboundedSender<Landed>) {
    let texts: Vec<parking_lot::Mutex<Option<Vec<Option<String>>>>> = jobs
        .iter()
        .map(|job| parking_lot::Mutex::new(job.known.clone()))
        .collect();
    let next = AtomicUsize::new(0);
    let workers = READERS.min(jobs.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let at = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(at) else {
                        return;
                    };
                    if job.known.is_some() {
                        continue;
                    }
                    if send.is_closed() {
                        return;
                    }
                    let read = read_file(backend, &job.target);
                    let lines = read.as_deref().map(split_lines).unwrap_or_default();
                    let rows: Vec<Option<String>> = job
                        .lines
                        .iter()
                        .map(|&line| lines.get(line as usize).map(|text| text.to_string()))
                        .collect();
                    *texts[at].lock() = Some(rows.clone());
                    let _ = send.unbounded_send(Landed::Text {
                        index: job.index,
                        texts: rows,
                    });
                }
            });
        }
    });
    if send.is_closed() {
        return;
    }
    // Each row's line highlighted on its own, so nothing one opens colours another.
    let texts: Vec<Vec<Option<String>>> = texts
        .into_iter()
        .map(|text| text.into_inner().unwrap_or_default())
        .collect();
    let mut highlighter = LineHighlighter::new();
    let tokens: Vec<Vec<Vec<Token>>> = jobs
        .iter()
        .zip(&texts)
        .map(|(job, rows)| {
            let name = target_name(&job.target);
            rows.iter()
                .map(|text| {
                    text.as_deref()
                        .map(|text| highlighter.highlight(&name, text))
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect();
    for (job, tokens) in jobs.iter().zip(tokens) {
        if tokens.iter().any(|row| !row.is_empty()) {
            let _ = send.unbounded_send(Landed::Tokens {
                index: job.index,
                tokens,
            });
        }
    }
}

/// The text of `target` as a server reads it: a worktree file from the working tree, one
/// outside it through `probe_path` (up to its read cap).
fn read_file(backend: &dyn Backend, target: &UriTarget) -> Option<String> {
    let bytes = match target {
        UriTarget::Worktree(path) => backend.worktree_read(path).ok().flatten(),
        UriTarget::Outside {
            path: Some(path), ..
        } => match backend.probe_path(path, dv_core::backend::PROBE_READ_MAX) {
            Ok(PathProbe::File {
                contents: Some(bytes),
                ..
            }) => Some(bytes),
            _ => None,
        },
        UriTarget::Outside { path: None, .. } => None,
    }?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The name a file's language is found from.
fn target_name(target: &UriTarget) -> String {
    match target {
        UriTarget::Worktree(path) => path.clone(),
        UriTarget::Outside { uri, path } => path.clone().unwrap_or_else(|| uri.clone()),
    }
}
