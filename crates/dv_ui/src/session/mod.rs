//! One open review (PLAN §11 `ReviewSession`): its repository and store, the folded review,
//! the target resolved to a [`DiffSpec`], the file list, and each file's diff as it loads.
//!
//! **Loading.** The file list comes first (one background job resolves the target and runs
//! `dv_core::diff::file_list`), then file diffs on demand: the stream asks for the files around
//! its viewport ([`ReviewSession::ensure_loaded`]), at most [`MAX_LOADS_IN_FLIGHT`] load at
//! once, and "Load anyway" reloads one file without the size caps
//! ([`ReviewSession::load_anyway`]). A loaded text diff is then highlighted in the background
//! (`crate::syntax`, token kinds only); rows paint unhighlighted until it lands. Nothing here
//! blocks the UI thread: git, file reads and the store all run in `background_spawn` jobs.
//!
//! **Staleness.** The session's load generation is the stale-completion guard (PLAN §11): a
//! file-list result is applied only if no newer refresh started, and each file load carries a
//! token that a refresh or a "Load anyway" replaces, so a load that finishes after its file was
//! invalidated is dropped. The target never changes, so nothing else can go stale. A file that
//! loads again keeps showing the diff it had until the new one lands
//! ([`ReviewSession::shown`]), so the rows under the reader — a thread's card, the composer
//! being typed in — stay where they are.
//!
//! **Live updates.** A live target (`working`, `staged`, a branch) watches the repository
//! (PLAN §5.3, `watch`): worktree paths and git-state changes are coalesced for
//! [`REFRESH_DEBOUNCE`] and refresh the spec and the file list, keeping every loaded diff whose
//! entry didn't change (same oids, totals and modes, path not in the batch) and reloading the
//! rest; an append to this review's log reloads the fold. The watch starts with the session; a
//! change made before it was registered is never reported, so whatever was read before then is
//! read again once it is. A watch that can't cover the worktree leaves a notice
//! ([`ReviewSession::watch_notice`]) and the session refreshes when the window is activated
//! instead ([`ReviewSession::window_activated`]).
//!
//! **Writes** go through `dv_core::review::ops` on a background thread, never as hand-built
//! events, and the fold reloads afterwards (`writes`); the watch reports the same append, and
//! both paths converge on the log. A viewed toggle that fails says why
//! ([`ReviewSession::viewed_error`]), since its checkbox alone can't.

mod load;
mod watch;
mod writes;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use dv_core::diff::{FileDiff, FileEntry};
use dv_core::review::{
    Anchor, AnchorError, AnchorPlace, AnchorSide, Author, LogWarning, Review, ReviewId,
    ReviewStore, Thread, anchor_selection, side_lines,
};
use dv_core::{Backend, DiffSpec, GitRepo};
use gpui::{Context, EventEmitter, Task};

use crate::failure::Failure;
use crate::services::reviews::{LabelContext, OpenedReview, side_labels, target_label};
use crate::syntax::FileHighlights;
pub use load::Batch;
use watch::WatchState;
use writes::ViewedIntent;

/// How many file diffs load at once.
pub const MAX_LOADS_IN_FLIGHT: usize = 4;

/// How long worktree and git changes gather before the file list refreshes, on top of the
/// watch's own coalescing (PLAN §5.3).
pub const REFRESH_DEBOUNCE: Duration = Duration::from_millis(150);

/// Where one file's diff stands.
#[derive(Clone, Debug)]
pub enum FileState {
    /// Not asked for yet, or invalidated by a refresh.
    Unloaded,
    /// A load is running; `token` tells its result from a superseded one's.
    Loading {
        token: u64,
    },
    Loaded {
        diff: Arc<FileDiff>,
        /// `None` until highlighting lands (or for a file with no grammar, or not text).
        highlights: Option<Arc<FileHighlights>>,
    },
    Failed(Failure),
}

impl FileState {
    /// `"unloaded"`, `"loading"`, `"loaded"` or `"failed"`, as automation state spells it.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Unloaded => "unloaded",
            Self::Loading { .. } => "loading",
            Self::Loaded { .. } => "loaded",
            Self::Failed(_) => "failed",
        }
    }

    /// The loaded diff, if any.
    pub fn diff(&self) -> Option<&Arc<FileDiff>> {
        match self {
            Self::Loaded { diff, .. } => Some(diff),
            _ => None,
        }
    }

    /// The loaded diff's highlights, once they landed.
    pub fn highlights(&self) -> Option<&Arc<FileHighlights>> {
        match self {
            Self::Loaded { highlights, .. } => highlights.as_ref(),
            _ => None,
        }
    }

    fn is_pending(&self) -> bool {
        matches!(self, Self::Unloaded | Self::Loading { .. })
    }
}

/// A loaded diff as the stream shows it: the diff and, once they landed, its highlights.
#[derive(Clone, Debug)]
pub struct Shown {
    pub diff: Arc<FileDiff>,
    pub highlights: Option<Arc<FileHighlights>>,
}

/// `"text"`, `"binary"`, `"too_large"`, `"submodule"` or `"unmerged"`: what a loaded diff is,
/// as automation state spells it.
pub fn diff_kind(diff: &FileDiff) -> &'static str {
    match diff {
        FileDiff::Text(_) => "text",
        FileDiff::Binary { .. } => "binary",
        FileDiff::TooLarge { .. } => "too_large",
        FileDiff::Submodule { .. } => "submodule",
        FileDiff::Unmerged => "unmerged",
    }
}

/// Where a thread shows (decision of PLAN §8 until relocation exists).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Its anchored text stands at its recorded lines: inline, after the range.
    Inline,
    /// It doesn't (or its file left the diff): listed as outdated with its original snippet
    /// (DESIGN §4.1).
    Outdated,
    /// Its file's diff isn't loaded yet, so it can't be told.
    Unknown,
}

/// What a session tells its subscribers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// The spec, the file list or the files' states were replaced.
    FilesChanged,
    /// A file's diff (or its highlights) landed, or it was sent to load again ("Load anyway",
    /// Retry).
    FileLoaded(String),
    /// The folded review changed.
    ReviewChanged,
    /// Refreshing failed; [`ReviewSession::error`] says why.
    Error,
}

/// One open review (see the module docs).
pub struct ReviewSession {
    backend: Arc<dyn Backend>,
    repo: GitRepo,
    store: ReviewStore,
    review: Review,
    warnings: Vec<LogWarning>,
    /// Who writes the reviewer's events.
    reviewer: Author,
    labels: LabelContext,
    spec: Option<DiffSpec>,
    files: Vec<FileEntry>,
    states: HashMap<String, FileState>,
    /// The diff each file showed before a refresh (or Retry, "Load anyway") sent it to load
    /// again; it stays on screen until the new one lands.
    previous: HashMap<String, Shown>,
    /// Files the reviewer loaded anyway: they keep loading without caps after a refresh.
    unlimited: HashSet<String>,
    /// The files the stream asked for, which `settled` waits on.
    wanted: Vec<String>,
    queue: VecDeque<String>,
    loads_in_flight: usize,
    /// Highlighting jobs by path, with the diff each one highlights.
    highlighting: HashMap<String, (Arc<FileDiff>, Task<()>)>,
    next_token: u64,
    /// The load generation: bumped by every refresh, checked by its result.
    generation: u64,
    refreshing: Option<Task<()>>,
    /// A refresh asked for while one runs; it starts when that one lands.
    queued: Option<Batch>,
    /// Whether a file list has landed.
    listed: bool,
    /// Why the last refresh failed.
    error: Option<Failure>,
    fold_task: Option<Task<()>>,
    /// Another fold reload was asked for while one ran.
    fold_again: bool,
    /// How many fold reloads have started; each knows its number, so a write can tell the
    /// reloads that read it from those that started before it landed.
    folds_started: u64,
    /// Why the last fold reload failed.
    fold_error: Option<Failure>,
    /// Writes running.
    writes: usize,
    /// Viewed toggles whose outcome hasn't been recorded yet.
    viewed_pending: usize,
    /// The viewed state the reviewer last asked for, by path, until the fold reads it back
    /// (`writes`): a toggle made meanwhile flips that, not the fold's stale state.
    viewed_intents: HashMap<String, ViewedIntent>,
    /// Why the last viewed toggle failed.
    viewed_error: Option<Failure>,
    watch: WatchState,
}

impl EventEmitter<SessionEvent> for ReviewSession {}

impl ReviewSession {
    /// A session over an opened review ([`crate::services::Reviews::open_review`]); it starts
    /// loading the file list at once.
    pub fn new(opened: OpenedReview, cx: &mut Context<Self>) -> Self {
        let OpenedReview {
            backend,
            loaded,
            reviewer,
            labels,
            ..
        } = opened;
        let mut session = Self {
            repo: GitRepo::new(backend.clone()),
            store: ReviewStore::new(backend.clone()),
            backend,
            review: loaded.review,
            warnings: loaded.warnings,
            reviewer,
            // As the review opened, so the target is named in full before the first listing
            // reads them again.
            labels,
            spec: None,
            files: Vec::new(),
            states: HashMap::new(),
            previous: HashMap::new(),
            unlimited: HashSet::new(),
            wanted: Vec::new(),
            queue: VecDeque::new(),
            loads_in_flight: 0,
            highlighting: HashMap::new(),
            next_token: 0,
            generation: 0,
            refreshing: None,
            queued: None,
            listed: false,
            error: None,
            fold_task: None,
            fold_again: false,
            folds_started: 0,
            fold_error: None,
            writes: 0,
            viewed_pending: 0,
            viewed_intents: HashMap::new(),
            viewed_error: None,
            watch: WatchState::default(),
        };
        // The watch first, so it is registered as early as possible; what the first list reads
        // before then is read again once it is (`watch`).
        session.ensure_watch(cx);
        session.refresh(Batch::everything(), cx);
        session
    }

    pub fn backend(&self) -> &Arc<dyn Backend> {
        &self.backend
    }

    pub fn store(&self) -> &ReviewStore {
        &self.store
    }

    pub fn review(&self) -> &Review {
        &self.review
    }

    pub fn review_id(&self) -> &ReviewId {
        &self.review.id
    }

    /// Problems found in the review's log (PLAN §7).
    pub fn warnings(&self) -> &[LogWarning] {
        &self.warnings
    }

    pub fn reviewer(&self) -> &Author {
        &self.reviewer
    }

    /// The target as the title bar names it ("working tree vs main").
    pub fn target_label(&self) -> String {
        target_label(&self.review.target, &self.labels)
    }

    /// What the stream's side labels call the old and the new side (`main` / `working tree`).
    pub fn side_labels(&self) -> (String, String) {
        side_labels(&self.review.target, &self.labels)
    }

    /// The target resolved at the last refresh; `None` until one landed.
    pub fn spec(&self) -> Option<&DiffSpec> {
        self.spec.as_ref()
    }

    /// The files of the last refresh, sorted by path.
    pub fn files(&self) -> &[FileEntry] {
        &self.files
    }

    pub fn file(&self, path: &str) -> Option<&FileEntry> {
        self.files.iter().find(|entry| entry.path == path)
    }

    pub fn file_state(&self, path: &str) -> Option<&FileState> {
        self.states.get(path)
    }

    /// The diff the stream shows for `path`: its loaded diff, or, while it loads again, the one
    /// it had before. `None` before its first load lands and after a failed one.
    pub fn shown(&self, path: &str) -> Option<Shown> {
        match self.states.get(path)? {
            FileState::Loaded { diff, highlights } => Some(Shown {
                diff: diff.clone(),
                highlights: highlights.clone(),
            }),
            FileState::Unloaded | FileState::Loading { .. } => self.previous.get(path).cloned(),
            FileState::Failed(_) => None,
        }
    }

    /// [`ReviewSession::shown`]'s diff.
    pub fn shown_diff(&self, path: &str) -> Option<&Arc<FileDiff>> {
        match self.states.get(path)? {
            FileState::Loaded { diff, .. } => Some(diff),
            FileState::Unloaded | FileState::Loading { .. } => {
                self.previous.get(path).map(|shown| &shown.diff)
            }
            FileState::Failed(_) => None,
        }
    }

    /// The load generation (PLAN §11).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether the file list is (re)loading.
    pub fn is_loading(&self) -> bool {
        self.refreshing.is_some() || self.is_first_load()
    }

    /// Whether the first file list is still loading: the views show skeletons then, never
    /// over a list that is only refreshing (DESIGN §4.4).
    pub fn is_first_load(&self) -> bool {
        !self.listed && self.error.is_none()
    }

    /// Why the last refresh (or, failing that, the last fold reload) failed.
    pub fn error(&self) -> Option<&Failure> {
        self.error.as_ref().or(self.fold_error.as_ref())
    }

    /// Why the last viewed toggle failed, until the next one lands or it is dismissed.
    pub fn viewed_error(&self) -> Option<&Failure> {
        self.viewed_error.as_ref()
    }

    /// Forgets [`ReviewSession::viewed_error`] (its banner's Dismiss).
    pub fn dismiss_viewed_error(&mut self, cx: &mut Context<Self>) {
        if self.viewed_error.take().is_some() {
            cx.notify();
        }
    }

    /// An approved, closed or submitted review is read-only (DESIGN §4.4).
    pub fn is_read_only(&self) -> bool {
        self.review.status.is_final()
    }

    /// Whether `path` is viewed in the round in progress, or in the round that finished a
    /// finished review ([`dv_core::review::Review::shown_round`]).
    pub fn is_viewed(&self, path: &str) -> bool {
        self.review.is_viewed(self.review.shown_round(), path)
    }

    /// Whether `path` will be viewed once the reviewer's viewed toggles have landed: the state
    /// the last one asked for while the fold hasn't read it back yet, else
    /// [`ReviewSession::is_viewed`]. What the next toggle flips.
    pub fn intends_viewed(&self, path: &str) -> bool {
        match self.viewed_intents.get(path) {
            Some(intent) if intent.round == self.review.current_round() => intent.on,
            _ => self.is_viewed(path),
        }
    }

    /// How many files of the list are viewed in the round [`ReviewSession::is_viewed`] reads.
    pub fn viewed_count(&self) -> usize {
        self.files
            .iter()
            .filter(|entry| self.is_viewed(&entry.path))
            .count()
    }

    /// The live threads anchored in `path` (on either side), in the order they were added.
    pub fn threads_in<'a>(&'a self, path: &'a str) -> impl Iterator<Item = &'a Thread> + 'a {
        let entry = self.file(path);
        self.review
            .live_threads()
            .filter(move |thread| entry.is_some_and(|entry| anchor_is_in(&thread.anchor, entry)))
    }

    /// The listed file `thread` is anchored in, if its file is still in the diff.
    pub fn file_of(&self, thread: &Thread) -> Option<&FileEntry> {
        self.files
            .iter()
            .find(|entry| anchor_is_in(&thread.anchor, entry))
    }

    /// Open threads anchored in `path` (on either side).
    pub fn open_threads_in(&self, path: &str) -> usize {
        self.review
            .live_threads()
            .filter(|thread| {
                thread.state == dv_core::review::ThreadState::Open
                    && self
                        .file(path)
                        .is_some_and(|entry| anchor_is_in(&thread.anchor, entry))
            })
            .count()
    }

    /// Why the watch can't cover the worktree (the session then refreshes on window
    /// activation); `None` while it does.
    pub fn watch_notice(&self) -> Option<&str> {
        self.watch.notice()
    }

    /// Nothing is loading, waiting or writing, and every file the stream asked for has landed:
    /// automation's `settled`.
    pub fn is_settled(&self) -> bool {
        let wanted_landed = self.spec.is_none()
            || self.wanted.iter().all(|path| {
                self.states
                    .get(path)
                    .is_none_or(|state| !state.is_pending())
            });
        self.refreshing.is_none()
            && self.queued.is_none()
            && self.watch.is_settled()
            && self.writes == 0
            && self.viewed_pending == 0
            && self.fold_task.is_none()
            && self.loads_in_flight == 0
            && self.highlighting.is_empty()
            && wanted_landed
    }

    /// Where `thread` shows: inline when its anchored text still stands at its recorded lines
    /// of the diff shown ([`ReviewSession::shown`]), outdated when it doesn't or its file left
    /// the diff.
    pub fn placement(&self, thread: &Thread) -> Placement {
        let Some(entry) = self
            .files
            .iter()
            .find(|entry| anchor_is_in(&thread.anchor, entry))
        else {
            return Placement::Outdated;
        };
        match self.shown_diff(&entry.path) {
            Some(diff) => placement_in(thread, diff),
            None => Placement::Unknown,
        }
    }

    /// The anchor for lines `from` to `to` (either order) of `path`'s `side` in its loaded
    /// text diff, in the round in progress (`dv_core::review::anchor_selection`).
    pub fn anchor_for(
        &self,
        path: &str,
        side: AnchorSide,
        from: u32,
        to: u32,
    ) -> Result<Anchor, Failure> {
        let entry = self
            .file(path)
            .ok_or_else(|| Failure::new(format!("{path} isn't in this review's diff")))?;
        // The lines the reviewer sees: while the file loads again, the ones it showed before.
        let text = self
            .shown_diff(path)
            .and_then(|diff| diff.as_text())
            .ok_or_else(|| Failure::new(format!("{path}'s lines aren't loaded")))?;
        let side_path = match side {
            AnchorSide::New => entry.path.clone(),
            AnchorSide::Old => entry.old_path.clone().unwrap_or_else(|| entry.path.clone()),
        };
        anchor_selection(
            AnchorPlace {
                round: self.review.current_round(),
                path: side_path,
                old_path: entry.old_path.clone(),
                side,
            },
            &side_lines(text, side),
            from,
            to,
        )
        .map_err(|error: AnchorError| Failure::new(error.to_string()))
    }
}

/// Where `thread` shows against `diff`, a diff of its file: inline when its anchored text stands
/// at its recorded lines there, else outdated.
pub fn placement_in(thread: &Thread, diff: &FileDiff) -> Placement {
    match diff.as_text() {
        Some(text) if thread.anchor.matches(&side_lines(text, thread.anchor.side)) => {
            Placement::Inline
        }
        _ => Placement::Outdated,
    }
}

/// Whether `anchor` is anchored in `entry`'s file: its path on the anchored side (a renamed
/// file's old side by its old path), or the new path.
fn anchor_is_in(anchor: &Anchor, entry: &FileEntry) -> bool {
    match anchor.side {
        AnchorSide::New => anchor.path == entry.path,
        AnchorSide::Old => {
            anchor.path == entry.path || entry.old_path.as_deref() == Some(anchor.path.as_str())
        }
    }
}
