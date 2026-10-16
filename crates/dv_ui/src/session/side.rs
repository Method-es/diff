//! Files at the review's sides, whole (PLAN §18.1 "The file view", §8): one file's text as the
//! review's new side (or, for a file it deletes, its old side) holds it
//! (`dv_core::diff::side_file`), read in the background for the file view and for the threads
//! on files the review doesn't list, and the paths the new side holds, for `Ctrl P`.
//!
//! **Reads.** A file is read when the file view shows it (highlighted then too, `crate::syntax`)
//! or when a live thread's file left the review's list ([`ReviewSession::ensure_side_reads`],
//! after a listing, a fold, a history read; at the path its round's history says the file has
//! now), and kept while the target resolves to the same sides and no refresh touches its path
//! (a watch batch naming it, a git change, a rescan). It is read whole, without the size caps,
//! as the CLI reads it, so a large file places its threads alike; the file view applies the caps
//! to what it shows ("Load anyway"). Read again, it keeps placing threads and showing in the
//! file view until the new read lands, so nothing flickers; a read that fails says why to the
//! file view and leaves its threads unplaced by it (outdated without a file, as before).
//!
//! **The working tree.** The file view also shows a file as the working tree holds it now
//! ([`ReviewSession::worktree_file`]), whatever the review's sides are: where go to definition
//! and references go in a review whose new side isn't the working tree, since a language server
//! reads the working tree (PLAN §18.2). Read the same way, kept apart from the sides' reads.
//!
//! **Paths.** The side's paths are read once per review open and round, when `Ctrl P` first
//! asks ([`ReviewSession::ensure_side_paths`]), and kept; the finder adds the review's own
//! files to them, so a file the agent created since shows too.

use std::collections::HashMap;
use std::sync::Arc;

use dv_core::Side;
use dv_core::diff::{DiffError, DiffLimits, FileDiff, side_file, side_paths};
use dv_core::review::{AnchorSide, SideText, ThreadFile, unlisted_path};
use gpui::{AppContext as _, Context, Task};

use super::load::Batch;
use super::placement::KeptSide;
use super::{ReviewSession, SessionEvent, Shown};
use crate::failure::Failure;
use crate::syntax::{FileHighlights, highlight_diff, language_for_path};

/// Where a read reads a file: one of the review's sides, or the working tree as it stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReadAt {
    Side(AnchorSide),
    Worktree,
}

/// One file read at one of the review's sides.
pub(super) struct SideRead {
    /// The side it is read at: a read at another one is dropped.
    side: Side,
    /// Bumped by every read that lands, so placement knows when to work a thread out again.
    pub version: u64,
    /// The last read that landed: `Some(None)` when the side holds no file at the path.
    read: Option<Result<Option<Arc<FileDiff>>, Failure>>,
    highlights: Option<Arc<FileHighlights>>,
    /// The file view shows it: it is highlighted.
    highlight: bool,
    /// A read under way, which a newer one replaces.
    loading: Option<(u64, Task<()>)>,
    highlighting: Option<Task<()>>,
}

/// Where a file at the new side stands, as the file view shows it.
#[derive(Clone, Debug)]
pub enum SideFileState {
    /// Not read yet (or read again, with nothing read before).
    Loading,
    /// The side holds no file at that path.
    Missing,
    Loaded(Shown),
    Failed(Failure),
}

/// The side's paths, read once (module docs, "Paths").
pub(super) struct SidePaths {
    key: (Side, u32),
    paths: Option<Arc<Vec<String>>>,
    _task: Option<Task<()>>,
}

impl ReviewSession {
    /// The review's new side, once the target resolved.
    pub fn new_side(&self) -> Option<&Side> {
        self.resolved.as_ref().map(|spec| &spec.new)
    }

    /// The review's `which` side (its target resolved: all changes, an interdiff's too), once
    /// it resolved.
    fn review_side(&self, which: AnchorSide) -> Option<&Side> {
        self.resolved.as_ref().map(|spec| match which {
            AnchorSide::Old => &spec.old,
            AnchorSide::New => &spec.new,
        })
    }

    /// The side `at` reads: the review's `which`, or the working tree.
    fn read_side(&self, at: ReadAt) -> Option<Side> {
        match at {
            ReadAt::Side(which) => self.review_side(which).cloned(),
            ReadAt::Worktree => Some(Side::Worktree),
        }
    }

    /// Where `path` stands at the review's `which` side, as [`ReviewSession::ensure_side_file`]
    /// reads it.
    pub fn side_file(&self, which: AnchorSide, path: &str) -> SideFileState {
        self.read_state(ReadAt::Side(which), path)
    }

    /// Where `path` stands in the working tree, as [`ReviewSession::ensure_worktree_file`]
    /// reads it (module docs, "The working tree").
    pub fn worktree_file(&self, path: &str) -> SideFileState {
        self.read_state(ReadAt::Worktree, path)
    }

    fn read_state(&self, at: ReadAt, path: &str) -> SideFileState {
        let Some(read) = self.side_reads.get(&(at, path.to_string())) else {
            return SideFileState::Loading;
        };
        match &read.read {
            None => SideFileState::Loading,
            Some(Ok(None)) => SideFileState::Missing,
            Some(Ok(Some(diff))) => SideFileState::Loaded(Shown {
                diff: diff.clone(),
                highlights: read.highlights.clone(),
            }),
            Some(Err(failure)) => SideFileState::Failed(failure.clone()),
        }
    }

    /// Reads `path` at the review's `which` side unless it is read, or being read; `highlight`
    /// (the file view shows it) highlights it too.
    pub fn ensure_side_file(
        &mut self,
        which: AnchorSide,
        path: &str,
        highlight: bool,
        cx: &mut Context<Self>,
    ) {
        self.ensure_read(ReadAt::Side(which), path, highlight, cx);
    }

    /// Reads `path` as the working tree holds it unless it is read, or being read; `highlight`
    /// highlights it too (module docs, "The working tree").
    pub fn ensure_worktree_file(&mut self, path: &str, highlight: bool, cx: &mut Context<Self>) {
        self.ensure_read(ReadAt::Worktree, path, highlight, cx);
    }

    fn ensure_read(&mut self, at: ReadAt, path: &str, highlight: bool, cx: &mut Context<Self>) {
        let Some(side) = self.read_side(at) else {
            return;
        };
        let key = (at, path.to_string());
        match self.side_reads.get_mut(&key) {
            Some(read) if read.side == side => {
                if highlight && !read.highlight {
                    read.highlight = true;
                    self.start_side_highlight(&key, cx);
                }
            }
            _ => self.start_side_read(key, side, highlight, cx),
        }
    }

    /// `path`'s text at the new side as placement reads it ([`dv_core::review::place_unlisted`]):
    /// its lines once read (the last read while another runs), pending before the first read
    /// lands, unavailable when the side holds no text there or the read failed.
    pub(super) fn side_text(&self, path: &str) -> SideText<KeptSide<'_>> {
        match self
            .side_reads
            .get(&(ReadAt::Side(AnchorSide::New), path.to_string()))
            .and_then(|read| read.read.as_ref())
        {
            None => SideText::Pending,
            Some(Ok(Some(diff))) => self.lines_of(path, AnchorSide::New, diff),
            Some(Ok(None) | Err(_)) => SideText::Unavailable,
        }
    }

    /// Whether the new side holds a file at `path`, as last read (`false` before a read lands,
    /// and when it failed).
    pub(super) fn side_present(&self, path: &str) -> bool {
        matches!(
            self.side_reads
                .get(&(ReadAt::Side(AnchorSide::New), path.to_string()))
                .and_then(|read| read.read.as_ref()),
            Some(Ok(Some(_)))
        )
    }

    /// Which read of `path` at the new side placement worked a thread out against.
    pub(super) fn side_version(&self, path: &str) -> Option<u64> {
        self.side_reads
            .get(&(ReadAt::Side(AnchorSide::New), path.to_string()))
            .map(|read| read.version)
    }

    /// Starts reading, at the new side, every file a live thread is anchored in whose file left
    /// the review's list, at the path its round's history says it has now (module docs,
    /// "Reads"). Blocking work, so in the background.
    pub(super) fn ensure_side_reads(&mut self, cx: &mut Context<Self>) {
        if !self.listed {
            return;
        }
        let wanted: Vec<String> = self
            .review
            .live_threads()
            .filter(|thread| {
                matches!(
                    self.thread_file_with(thread, self.history_for(thread)),
                    ThreadFile::Gone
                )
            })
            .map(|thread| unlisted_path(&thread.anchor, self.history_for(thread)))
            .collect();
        for path in wanted {
            self.ensure_side_file(AnchorSide::New, &path, false, cx);
        }
    }

    /// After a listing (module docs, "Reads"): reads again every file `batch` touches, or all
    /// of them when the git state moved or the new side is another.
    pub(super) fn refresh_side_reads(&mut self, batch: &Batch, cx: &mut Context<Self>) {
        let Some(side) = self.new_side().cloned() else {
            return;
        };
        let stale: Vec<((ReadAt, String), Side, bool)> = self
            .side_reads
            .iter()
            .filter_map(|((which, path), read)| {
                let now = self.read_side(*which)?;
                (batch.rescan || batch.git || read.side != now || batch.paths.contains(path))
                    .then(|| ((*which, path.clone()), now, read.highlight))
            })
            .collect();
        for (key, now, highlight) in stale {
            self.start_side_read(key, now, highlight, cx);
        }
        let round = self.review.current_round();
        if self
            .side_paths
            .as_ref()
            .is_some_and(|paths| paths.key != (side.clone(), round))
        {
            self.side_paths = None;
        }
    }

    fn start_side_read(
        &mut self,
        key: (ReadAt, String),
        side: Side,
        highlight: bool,
        cx: &mut Context<Self>,
    ) {
        self.next_token += 1;
        let token = self.next_token;
        let (repo, read_side, read_path) = (self.repo.clone(), side.clone(), key.1.clone());
        // Whole, as the CLI reads it: the file view applies the caps to what it shows.
        let job = cx.background_spawn(async move {
            side_file(&repo, &read_side, &read_path, &DiffLimits::UNLIMITED)
        });
        let task = cx.spawn({
            let key = key.clone();
            async move |this, cx| {
                let read = job.await;
                this.update(cx, |this, cx| this.finish_side_read(&key, token, read, cx))
                    .ok();
            }
        });
        let read = self.side_reads.entry(key).or_insert_with(|| SideRead {
            side: side.clone(),
            version: 0,
            read: None,
            highlights: None,
            highlight,
            loading: None,
            highlighting: None,
        });
        // A read at another side doesn't show meanwhile.
        if read.side != side {
            read.read = None;
            read.highlights = None;
        }
        read.side = side;
        read.highlight |= highlight;
        read.loading = Some((token, task));
    }

    fn finish_side_read(
        &mut self,
        key: &(ReadAt, String),
        token: u64,
        read: Result<Option<FileDiff>, DiffError>,
        cx: &mut Context<Self>,
    ) {
        let path = key.1.as_str();
        let Some(state) = self
            .side_reads
            .get_mut(key)
            .filter(|state| state.loading.as_ref().is_some_and(|(at, _)| *at == token))
        else {
            return;
        };
        state.loading = None;
        let read = match read {
            Ok(diff) => Ok(diff.map(Arc::new)),
            Err(error) => {
                let failure = Failure::from(&error);
                if self.lost_connection(&failure).is_some() {
                    // The reconnect's rescan reads it again; meanwhile it shows what it had.
                    cx.notify();
                    return;
                }
                log::warn!(
                    "review {}: couldn't read {path} at the review's side: {error}",
                    self.review.id
                );
                Err(failure)
            }
        };
        let Some(state) = self.side_reads.get_mut(key) else {
            return;
        };
        let same = match (&state.read, &read) {
            (Some(Ok(Some(old))), Ok(Some(new))) => old == new,
            _ => false,
        };
        if !same {
            state.read = Some(read);
            state.highlights = None;
            state.version += 1;
        }
        if state.highlight && state.highlights.is_none() {
            self.start_side_highlight(key, cx);
        }
        cx.emit(SessionEvent::FileLoaded(path.to_string()));
        cx.notify();
    }

    /// Highlights the read `key` names in the background, when its file has a grammar.
    fn start_side_highlight(&mut self, key: &(ReadAt, String), cx: &mut Context<Self>) {
        let path = key.1.as_str();
        let Some(diff) = self
            .side_reads
            .get(key)
            .and_then(|read| read.read.as_ref())
            .and_then(|read| read.as_ref().ok())
            .and_then(Option::as_ref)
            .cloned()
        else {
            return;
        };
        if language_for_path(path).is_none() || diff.as_text().is_none() {
            return;
        }
        let job = cx.background_spawn({
            let (path, diff) = (path.to_string(), diff.clone());
            async move {
                diff.as_text()
                    .map(|text| highlight_diff(&path, None, text))
                    .unwrap_or_default()
            }
        });
        let task = cx.spawn({
            let key = key.clone();
            async move |this, cx| {
                let highlights = job.await;
                this.update(cx, |this, cx| {
                    let path = key.1.clone();
                    let Some(read) = this.side_reads.get_mut(&key) else {
                        return;
                    };
                    read.highlighting = None;
                    let current =
                        matches!(&read.read, Some(Ok(Some(now))) if Arc::ptr_eq(now, &diff));
                    if current {
                        read.highlights = Some(Arc::new(highlights));
                        cx.emit(SessionEvent::FileLoaded(path.clone()));
                        cx.notify();
                    }
                })
                .ok();
            }
        });
        if let Some(read) = self.side_reads.get_mut(key) {
            read.highlighting = Some(task);
        }
    }

    /// Whether a read or a highlight of a file at the new side is under way (automation's
    /// `settled`).
    pub(super) fn side_reads_loading(&self) -> bool {
        self.side_reads
            .values()
            .any(|read| read.loading.is_some() || read.highlighting.is_some())
            || self
                .side_paths
                .as_ref()
                .is_some_and(|paths| paths.paths.is_none())
    }

    /// The paths the new side holds, once read ([`ReviewSession::ensure_side_paths`]).
    pub fn side_paths(&self) -> Option<&Arc<Vec<String>>> {
        self.side_paths.as_ref()?.paths.as_ref()
    }

    /// Reads the paths the new side holds unless they are read for this round, or being read
    /// (module docs, "Paths").
    pub fn ensure_side_paths(&mut self, cx: &mut Context<Self>) {
        let Some(side) = self.new_side().cloned() else {
            return;
        };
        let key = (side.clone(), self.review.current_round());
        if self
            .side_paths
            .as_ref()
            .is_some_and(|paths| paths.key == key)
        {
            return;
        }
        let repo = self.repo.clone();
        let job = cx.background_spawn(async move { side_paths(&repo, &side) });
        let task = cx.spawn({
            let key = key.clone();
            async move |this, cx| {
                let read = job.await;
                this.update(cx, |this, cx| {
                    let Some(paths) = this.side_paths.as_mut().filter(|paths| paths.key == key)
                    else {
                        return;
                    };
                    paths._task = None;
                    paths.paths = Some(Arc::new(read.unwrap_or_else(|error| {
                        log::warn!("listing the files of the review's new side: {error}");
                        Vec::new()
                    })));
                    cx.notify();
                })
                .ok();
            }
        });
        self.side_paths = Some(SidePaths {
            key,
            paths: None,
            _task: Some(task),
        });
    }
}

/// The reads kept per side and path.
pub(super) type SideReads = HashMap<(ReadAt, String), SideRead>;
