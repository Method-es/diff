//! The session's loads: the file list (with the spec and the labels), file diffs on demand
//! (the stream's viewport, and every file a thread shows in, wherever it is), their
//! highlighting, the review's own diff of a file whose threads the stream can't place against
//! what it shows (for placement, PLAN §8), and the fold.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use dv_core::diff::{DiffLimits, FileDiff, FileEntry, file_diff, file_list};
use dv_core::review::{LoadedReview, Round};
use dv_core::{DiffSpec, GitRepo, Side, Target, TargetError};
use gpui::{AppContext as _, Context, Task};

use super::placement::FileIndex;
use super::{
    Comparison, FileState, MAX_LOADS_IN_FLIGHT, Previous, ReviewSession, SessionEvent, Shown,
    default_comparison,
};
use crate::failure::Failure;
use crate::syntax::{FileHighlights, highlight_diff, language_for_path};
use dv_core::label::LabelContext;

/// What a refresh re-reads: the worktree paths a watch batch reported, whether the git state
/// moved, and `rescan` for everything (the first load, an explicit rescan, a watch that may
/// have lost events). Every refresh re-resolves the target and re-lists the files; the batch
/// only decides which loaded diffs can be kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Batch {
    pub paths: BTreeSet<String>,
    pub git: bool,
    pub rescan: bool,
}

impl Batch {
    /// Reload everything.
    pub fn everything() -> Self {
        Self {
            rescan: true,
            ..Self::default()
        }
    }

    /// A batch for these worktree paths.
    pub fn paths(paths: impl IntoIterator<Item = String>) -> Self {
        Self {
            paths: paths.into_iter().collect(),
            ..Self::default()
        }
    }

    pub fn merge(&mut self, other: Batch) {
        self.paths.extend(other.paths);
        self.git |= other.git;
        self.rescan |= other.rescan;
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && !self.git && !self.rescan
    }

    /// Whether the batch names `entry`'s file, by either of its paths.
    fn touches(&self, entry: &FileEntry) -> bool {
        self.paths.contains(&entry.path)
            || entry
                .old_path
                .as_ref()
                .is_some_and(|old| self.paths.contains(old))
    }
}

/// What a refresh read.
pub(super) struct Listed {
    /// What the files were listed with: the target resolved, or the interdiff since a round.
    spec: DiffSpec,
    /// The target resolved.
    resolved: DiffSpec,
    files: Vec<FileEntry>,
    /// In an interdiff, the target's own files (all changes).
    target_files: Option<Vec<FileEntry>>,
    /// The branch labels, when they were read again (`None` keeps the ones the session has).
    labels: Option<LabelContext>,
    /// What the files are.
    comparison: Comparison,
    /// Why the interdiff asked for fell back to all changes (its round's snapshot is missing).
    fallback: Option<Failure>,
}

/// Resolves `target` and lists its files (blocking): all its changes, or with `since` the
/// interdiff since that round (PLAN §2.3) and the target's own files beside it (the review's
/// files, which threads belong to), unless the round's snapshot is missing from the
/// repository, when it lists all changes and says why. The branch labels name HEAD and the
/// default branch, which only the git state moves: they are read again when `read_labels` (a
/// git change, a rescan, the first list) or when the target resolved elsewhere than
/// `previous`, never for a batch of worktree paths alone.
fn list_blocking(
    repo: &GitRepo,
    target: &Target,
    since: Option<&Round>,
    previous: Option<&DiffSpec>,
    read_labels: bool,
) -> Result<Listed, Failure> {
    let resolved = target
        .resolve(repo)
        .map_err(|error| Failure::from(&error))?;
    let (spec, comparison, fallback) = match since {
        None => (resolved.clone(), Comparison::All, None),
        Some(round) => match resolved.since_checked(repo, round) {
            Ok(spec) => (spec, Comparison::SinceRound(round.n), None),
            Err(error @ TargetError::MissingSnapshot { .. }) => (
                resolved.clone(),
                Comparison::All,
                Some(Failure::new(format!(
                    "Showing all changes: {}",
                    Failure::from(&error).message
                ))),
            ),
            Err(error) => return Err(Failure::from(&error)),
        },
    };
    let files = file_list(repo, &spec).map_err(|error| Failure::from(&error))?;
    let target_files = match comparison {
        Comparison::SinceRound(_) => {
            Some(file_list(repo, &resolved).map_err(|error| Failure::from(&error))?)
        }
        Comparison::All => None,
    };
    let labels = (read_labels || previous != Some(&resolved)).then(|| LabelContext::read(repo));
    Ok(Listed {
        spec,
        resolved,
        files,
        target_files,
        labels,
        comparison,
        fallback,
    })
}

/// The review's own diff of a file (the target resolved, without caps: what the CLI reads),
/// read in the background for the threads anchored in it that the stream can't place against
/// what it shows: a file shown too large ([`ReviewSession::needs_full_text`]), or, in an
/// interdiff, a file it doesn't list and old-side threads, which are placed against it for
/// where they stand in all changes (`placement`). It is kept while the review's entry for the
/// file stays as it was read and no watch batch touches the file.
pub(super) struct ReviewText {
    /// Which read this is: a result lands only on the read that asked for it.
    token: u64,
    /// The review's entry for the file, as it was read.
    entry: FileEntry,
    state: FullState,
}

enum FullState {
    /// Being read; dropping the task drops the read's result.
    Loading {
        _task: Task<()>,
    },
    Loaded(Arc<FileDiff>),
    /// It couldn't be read: the threads there have no text to be placed against.
    Failed,
}

/// Where the read of a file's review text stands ([`ReviewSession::review_text`]).
pub(super) enum FullRead<'a> {
    Loading,
    Loaded(&'a Arc<FileDiff>),
    /// It couldn't be read: a thread the stream draws is placed against the diff shown, which
    /// is all there is; one it doesn't draw stays unplaced (`placement`).
    Failed,
}

impl ReviewText {
    pub(super) fn is_loading(&self) -> bool {
        matches!(self.state, FullState::Loading { .. })
    }
}

/// Whether a diff loaded for `old` still shows `new`. The same entry, or one whose working tree
/// side only gained or lost its object id: git hashes a worktree file only while the index's
/// stat data says it matches, so `git add` alone flips the id between unknown and known without
/// the content changing (an edit to the file itself arrives in the watch's batch).
fn unchanged(old: &FileEntry, new: &FileEntry, worktree: bool) -> bool {
    if old == new {
        return true;
    }
    let flipped = old.new_oid.is_none() != new.new_oid.is_none();
    worktree
        && flipped
        && FileEntry {
            new_oid: None,
            ..old.clone()
        } == FileEntry {
            new_oid: None,
            ..new.clone()
        }
}

impl ReviewSession {
    /// Re-resolves the target and re-lists the files in the background, keeping the loaded
    /// diffs `batch` doesn't invalidate. While a refresh runs, another waits for it (merged
    /// with any others asked for meanwhile) rather than racing it.
    pub fn refresh(&mut self, batch: Batch, cx: &mut Context<Self>) {
        if self.refreshing.is_some() {
            self.queued.get_or_insert_default().merge(batch);
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        self.watch.note_list();
        let (repo, target) = (self.repo.clone(), self.review.target.clone());
        let since = self
            .asked
            .round()
            .and_then(|n| self.review.rounds.get((n as usize).checked_sub(1)?))
            .cloned();
        let previous = self.resolved.clone();
        let read_labels = batch.git || batch.rescan || !self.listed;
        let job = cx.background_spawn(async move {
            list_blocking(
                &repo,
                &target,
                since.as_ref(),
                previous.as_ref(),
                read_labels,
            )
        });
        self.refreshing = Some(cx.spawn(async move |this, cx| {
            let listed = job.await;
            this.update(cx, |this, cx| {
                this.apply_listed(generation, batch, listed, cx)
            })
            .ok();
        }));
        cx.notify();
    }

    /// Refreshes everything: the spec, the list and every diff.
    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        self.refresh(Batch::everything(), cx);
    }

    /// Applies a refresh's result, unless a newer refresh started since (`generation`).
    pub(super) fn apply_listed(
        &mut self,
        generation: u64,
        batch: Batch,
        listed: Result<Listed, Failure>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        self.refreshing = None;
        match listed {
            Ok(Listed {
                spec,
                resolved,
                files,
                target_files,
                labels,
                comparison,
                fallback,
            }) => {
                // A review text stays while the review's entry for its file is as it was read.
                let review_files = target_files.as_ref().unwrap_or(&files);
                let resolved_worktree = resolved.new == Side::Worktree;
                self.review_texts.retain(|path, text| {
                    !batch.rescan
                        && review_files.iter().any(|entry| {
                            entry.path == *path
                                && unchanged(&text.entry, entry, resolved_worktree)
                                && !batch.touches(entry)
                        })
                });
                let old: HashMap<&str, &FileEntry> = self
                    .list
                    .files
                    .iter()
                    .map(|entry| (entry.path.as_str(), entry))
                    .collect();
                let worktree = spec.new == Side::Worktree;
                // Another comparison lists other diffs, even of the same entries.
                let rescan = batch.rescan || comparison != self.comparison;
                let mut states = HashMap::with_capacity(files.len());
                for entry in &files {
                    let keep = !rescan
                        && old
                            .get(entry.path.as_str())
                            .is_some_and(|old| unchanged(old, entry, worktree))
                        && !batch.touches(entry);
                    let state = match (keep, self.states.remove(&entry.path)) {
                        (true, Some(state)) => state,
                        // It loads again; the diff it showed stays until the new one lands.
                        (false, Some(FileState::Loaded { diff, highlights })) => {
                            let shown = Shown { diff, highlights };
                            self.previous.insert(
                                entry.path.clone(),
                                Previous {
                                    shown,
                                    comparison: self.comparison,
                                },
                            );
                            FileState::Unloaded
                        }
                        _ => FileState::Unloaded,
                    };
                    states.insert(entry.path.clone(), state);
                }
                self.states = states;
                self.previous.retain(|path, _| {
                    matches!(
                        self.states.get(path),
                        Some(FileState::Unloaded | FileState::Loading { .. })
                    )
                });
                self.highlighting.retain(|path, (diff, _)| {
                    matches!(
                        self.states.get(path),
                        Some(FileState::Loaded { diff: loaded, highlights: None })
                            if Arc::ptr_eq(loaded, diff)
                    )
                });
                self.unlimited.retain(|path| self.states.contains_key(path));
                self.list = FileIndex::new(files);
                self.target_list = target_files.map(FileIndex::new);
                self.spec = Some(spec);
                self.resolved = Some(resolved);
                self.comparison = comparison;
                if let Some(fallback) = fallback {
                    self.asked = comparison;
                    self.comparison_error = Some(fallback);
                }
                // Placements stay: each is worked out again once what it read changes.
                self.forget_fuzzy_tables();
                if let Some(labels) = labels {
                    self.labels = labels;
                }
                self.listed = true;
                self.error = None;
                cx.emit(SessionEvent::FilesChanged);
                self.requeue_wanted();
                self.pump(cx);
                self.ensure_review_texts(cx);
                self.refresh_histories(&batch, cx);
                self.ensure_histories(cx);
                self.ensure_watch(cx);
            }
            Err(error) => match self.lost_connection(&error) {
                // Back already: list again.
                Some(true) => self.queued.get_or_insert_default().merge(batch),
                // The list stays as it was; the reconnect's rescan lists again.
                Some(false) => {}
                None => {
                    log::warn!("review {}: refresh failed: {error}", self.review.id);
                    self.error = Some(error);
                    cx.emit(SessionEvent::Error);
                    self.ensure_watch(cx);
                }
            },
        }
        if let Some(queued) = self.queued.take() {
            self.refresh(queued, cx);
        }
        cx.notify();
    }

    /// Asks for these files' diffs (the stream's viewport and a small prefetch). They load in
    /// order, at most [`MAX_LOADS_IN_FLIGHT`] at once, then the files threads show in
    /// ([`ReviewSession::thread_files`]), which always load, wherever the stream is; files asked
    /// for before and no longer wanted stop waiting.
    pub fn ensure_loaded(
        &mut self,
        paths: impl IntoIterator<Item = String>,
        cx: &mut Context<Self>,
    ) {
        self.wanted = paths.into_iter().collect();
        let threaded = self.thread_files();
        let wanted = &self.wanted;
        self.queue
            .retain(|path| wanted.contains(path) || threaded.contains(path));
        self.requeue_wanted();
        self.pump(cx);
        cx.notify();
    }

    /// Loads `path` again without the size caps ("Load anyway", PLAN §6 step 2); it stays
    /// uncapped across refreshes.
    pub fn load_anyway(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.states.contains_key(path) {
            self.unlimited.insert(path.to_string());
            self.reload_file(path, cx);
        }
    }

    /// Loads `path` again, first in line (Retry after a failed load), with the caps it had.
    pub fn reload_file(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.states.contains_key(path) {
            return;
        }
        // The diff it shows stays on screen until the new one lands.
        if let Some(FileState::Loaded { diff, highlights }) =
            self.states.insert(path.to_string(), FileState::Unloaded)
        {
            let previous = Previous {
                shown: Shown { diff, highlights },
                comparison: self.comparison,
            };
            self.previous.insert(path.to_string(), previous);
        }
        self.queue.retain(|queued| queued != path);
        self.queue.push_front(path.to_string());
        if !self.wanted.iter().any(|wanted| wanted == path) {
            self.wanted.push(path.to_string());
        }
        cx.emit(SessionEvent::FileLoaded(path.to_string()));
        self.pump(cx);
        cx.notify();
    }

    /// Queues the wanted files that need loading, then the files threads show in: a thread's
    /// place is worked out against its file's diff, so the comments list, "Copy as prompt" and
    /// the sheet name where it is now (PLAN §8), and a file the agent edited since it last
    /// loaded doesn't keep placing threads against the diff it had.
    pub(super) fn requeue_wanted(&mut self) {
        let threaded = self.thread_files();
        for path in self.wanted.iter().chain(&threaded) {
            if matches!(self.states.get(path), Some(FileState::Unloaded))
                && !self.queue.contains(path)
            {
                self.queue.push_back(path.clone());
            }
        }
    }

    /// Holds the files queued to load until `held` is false again, then starts them (tests:
    /// what the stream shows before a file's load lands).
    #[cfg(test)]
    pub(crate) fn hold_loads(&mut self, held: bool, cx: &mut Context<Self>) {
        self.loads_held = held;
        self.pump(cx);
    }

    /// Starts queued loads while there is room.
    pub(super) fn pump(&mut self, cx: &mut Context<Self>) {
        // While the connection is down every load would fail at once (`connection`).
        if self.spec.is_none() || self.link.down {
            return;
        }
        #[cfg(test)]
        if self.loads_held {
            return;
        }
        while self.loads_in_flight < MAX_LOADS_IN_FLIGHT {
            let Some(path) = self.queue.pop_front() else {
                break;
            };
            if matches!(self.states.get(&path), Some(FileState::Unloaded)) {
                self.start_load(path, cx);
            }
        }
    }

    fn start_load(&mut self, path: String, cx: &mut Context<Self>) {
        let (Some(entry), Some(spec)) = (self.file(&path).cloned(), self.spec.clone()) else {
            return;
        };
        self.next_token += 1;
        let token = self.next_token;
        self.states
            .insert(path.clone(), FileState::Loading { token });
        self.loads_in_flight += 1;
        self.watch.note_load();
        let limits = if self.unlimited.contains(&path) {
            DiffLimits::UNLIMITED
        } else {
            DiffLimits::default()
        };
        let repo = self.repo.clone();
        let job = cx.background_spawn(async move { file_diff(&repo, &spec, &entry, &limits) });
        // Detached, so the count of loads in flight always comes back down, even for a load
        // whose result is stale by then.
        cx.spawn(async move |this, cx| {
            let loaded = job
                .await
                .map(Arc::new)
                .map_err(|error| Failure::from(&error));
            this.update(cx, |this, cx| {
                this.loads_in_flight -= 1;
                this.finish_file_load(&path, token, loaded, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Applies a file load's result, unless its file was invalidated since it started (its
    /// state no longer carries `token`).
    pub(super) fn finish_file_load(
        &mut self,
        path: &str,
        token: u64,
        loaded: Result<Arc<FileDiff>, Failure>,
        cx: &mut Context<Self>,
    ) {
        let current =
            matches!(self.states.get(path), Some(FileState::Loading { token: t }) if *t == token);
        let lost = match &loaded {
            Err(failure) if current => self.lost_connection(failure),
            _ => None,
        };
        if lost.is_some() {
            // It loads again once the connection is back (at once when it is already), still
            // showing the diff it had meanwhile.
            self.states.insert(path.to_string(), FileState::Unloaded);
            if !self.queue.iter().any(|queued| queued == path) {
                self.queue.push_front(path.to_string());
            }
        } else if current {
            self.previous.remove(path);
            match loaded {
                Ok(diff) => {
                    self.states.insert(
                        path.to_string(),
                        FileState::Loaded {
                            diff: diff.clone(),
                            highlights: None,
                        },
                    );
                    self.start_highlight(path, diff, cx);
                    self.ensure_review_texts(cx);
                    self.ensure_histories(cx);
                }
                Err(error) => {
                    self.states
                        .insert(path.to_string(), FileState::Failed(error));
                }
            }
            cx.emit(SessionEvent::FileLoaded(path.to_string()));
        }
        self.pump(cx);
        cx.notify();
    }

    /// Highlights a loaded text diff in the background, when either side has a grammar.
    fn start_highlight(&mut self, path: &str, diff: Arc<FileDiff>, cx: &mut Context<Self>) {
        let old_path = self.file(path).and_then(|entry| entry.old_path.clone());
        let has_grammar = language_for_path(path).is_some()
            || old_path.as_deref().and_then(language_for_path).is_some();
        // A file whose content didn't change shows a notice, not its lines.
        if diff.as_text().is_none_or(|text| !text.has_changes()) || !has_grammar {
            return;
        }
        let job = cx.background_spawn({
            let (path, diff) = (path.to_string(), diff.clone());
            async move {
                let text = diff
                    .as_text()
                    .map(|text| highlight_diff(&path, old_path.as_deref(), text));
                text.unwrap_or_default()
            }
        });
        let task = cx.spawn({
            let (path, diff) = (path.to_string(), diff.clone());
            async move |this, cx| {
                let highlights = job.await;
                this.update(cx, |this, cx| {
                    this.finish_highlight(&path, &diff, highlights, cx)
                })
                .ok();
            }
        });
        self.highlighting.insert(path.to_string(), (diff, task));
    }

    fn finish_highlight(
        &mut self,
        path: &str,
        diff: &Arc<FileDiff>,
        highlights: FileHighlights,
        cx: &mut Context<Self>,
    ) {
        if self
            .highlighting
            .get(path)
            .is_some_and(|(running, _)| Arc::ptr_eq(running, diff))
        {
            self.highlighting.remove(path);
        }
        if let Some(FileState::Loaded {
            diff: loaded,
            highlights: slot,
        }) = self.states.get_mut(path)
            && Arc::ptr_eq(loaded, diff)
        {
            *slot = Some(Arc::new(highlights));
            cx.emit(SessionEvent::FileLoaded(path.to_string()));
            cx.notify();
        }
    }

    /// Whether `shown`, the diff the stream shows for a file, holds no text placement can read
    /// a thread's side from although the review's own diff of the file may (module docs of
    /// `placement`): a file too large to show.
    pub(super) fn needs_full_text(&self, shown: &FileDiff) -> bool {
        matches!(shown, FileDiff::TooLarge { .. })
    }

    /// Where the read of the review's own diff of `path` stands; `None` when none was asked for.
    pub(super) fn review_text(&self, path: &str) -> Option<FullRead<'_>> {
        Some(match &self.review_texts.get(path)?.state {
            FullState::Loading { .. } => FullRead::Loading,
            FullState::Loaded(diff) => FullRead::Loaded(diff),
            FullState::Failed => FullRead::Failed,
        })
    }

    /// Starts reading the review's own diff (its target resolved, without caps, as the CLI reads
    /// it) of every file a live thread is anchored in that the stream can't place it against
    /// (a file shown too large; in an interdiff, one not listed, or an old-side thread) and that
    /// has none read. Blocking work, so in the background; a read whose file changed meanwhile
    /// is dropped with it (`apply_listed`). A comment on the whole file reads no text, so it
    /// asks for none.
    pub(super) fn ensure_review_texts(&mut self, cx: &mut Context<Self>) {
        let Some(resolved) = self.resolved.clone() else {
            return;
        };
        let mut wanted: Vec<FileEntry> = Vec::new();
        for thread in self.review.live_threads() {
            if thread.anchor.is_file_level() {
                continue;
            }
            let Some(entry) = self.thread_entry(thread) else {
                continue;
            };
            let needed = if self.draws(thread, entry) {
                matches!(self.states.get(&entry.path), Some(FileState::Loaded { diff, .. })
                    if self.needs_full_text(diff))
            } else {
                true
            };
            let asked = self.review_texts.contains_key(&entry.path)
                || wanted.iter().any(|queued| queued.path == entry.path);
            if needed && !asked {
                wanted.push(entry.clone());
            }
        }
        for entry in wanted {
            self.next_token += 1;
            let token = self.next_token;
            let path = entry.path.clone();
            let (repo, spec, read) = (self.repo.clone(), resolved.clone(), entry.clone());
            let job = cx.background_spawn(async move {
                file_diff(&repo, &spec, &read, &DiffLimits::UNLIMITED)
            });
            let task = cx.spawn({
                let path = path.clone();
                async move |this, cx| {
                    let read = job.await;
                    this.update(cx, |this, cx| {
                        this.finish_review_text(&path, token, read, cx)
                    })
                    .ok();
                }
            });
            self.review_texts.insert(
                path,
                ReviewText {
                    token,
                    entry,
                    state: FullState::Loading { _task: task },
                },
            );
        }
    }

    /// Ends the read of `path`'s review text under way as a read git couldn't make (tests).
    #[cfg(test)]
    pub(super) fn fail_review_text(&mut self, path: &str, cx: &mut Context<Self>) {
        let token = self
            .review_texts
            .get(path)
            .filter(|text| text.is_loading())
            .map(|text| text.token);
        if let Some(token) = token {
            let failed = Err(dv_core::diff::DiffError::UnexpectedOutput {
                command: "diff",
                detail: "a read that failed (test)".to_string(),
            });
            self.finish_review_text(path, token, failed, cx);
        }
    }

    fn finish_review_text(
        &mut self,
        path: &str,
        token: u64,
        read: Result<FileDiff, dv_core::diff::DiffError>,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self
            .review_texts
            .get_mut(path)
            .filter(|text| text.token == token)
        else {
            return;
        };
        text.state = match read {
            Ok(diff) => FullState::Loaded(Arc::new(diff)),
            Err(error) => {
                log::warn!(
                    "review {}: couldn't read {path} in full: {error}",
                    self.review.id
                );
                FullState::Failed
            }
        };
        // A thread placed against this read may need its round's history now.
        self.ensure_histories(cx);
        cx.emit(SessionEvent::FileLoaded(path.to_string()));
        cx.notify();
    }

    /// Reads the review's log again (an append by the reviewer or an agent). A reload asked for
    /// while one runs follows it, so the last append is always read.
    pub fn reload_fold(&mut self, cx: &mut Context<Self>) {
        if self.fold_task.is_some() {
            self.fold_again = true;
            return;
        }
        self.folds_started += 1;
        let number = self.folds_started;
        let (store, id) = (self.store.clone(), self.review.id.clone());
        let job = cx.background_spawn(async move { store.load(&id) });
        self.fold_task = Some(cx.spawn(async move |this, cx| {
            let loaded = job.await.map_err(|error| Failure::from(&error));
            this.update(cx, |this, cx| this.apply_fold(number, loaded, cx))
                .ok();
        }));
    }

    /// Applies fold reload `number`'s result and forgets the viewed states it read back
    /// (`writes`). A round that closed moves the comparison to the interdiff since it, and a
    /// review that finished to all changes (PLAN §2.3, `comparison`).
    fn apply_fold(
        &mut self,
        number: u64,
        loaded: Result<LoadedReview, Failure>,
        cx: &mut Context<Self>,
    ) {
        self.fold_task = None;
        match loaded {
            Ok(loaded) => {
                self.fold_error = None;
                self.viewed_intents
                    .retain(|_, intent| !intent.read_back_by(number));
                if loaded.review != self.review || loaded.warnings != self.warnings {
                    let rounds = self.review.rounds.len();
                    let finished = self.review.status.is_final();
                    self.review = loaded.review;
                    self.warnings = loaded.warnings;
                    cx.emit(SessionEvent::ReviewChanged);
                    // A thread added in a file the stream hasn't loaded, or can't place it
                    // against, has that file loaded or read.
                    self.requeue_wanted();
                    self.pump(cx);
                    self.ensure_review_texts(cx);
                    self.ensure_histories(cx);
                    if self.review.rounds.len() > rounds
                        || self.review.status.is_final() != finished
                    {
                        self.set_comparison(default_comparison(&self.review), cx);
                    }
                }
            }
            Err(error) => match self.lost_connection(&error) {
                Some(true) => self.fold_again = true,
                // The reconnect's rescan reads it again.
                Some(false) => {}
                None => {
                    log::warn!("review {}: reload failed: {error}", self.review.id);
                    self.fold_error = Some(error);
                    cx.emit(SessionEvent::Error);
                }
            },
        }
        if std::mem::take(&mut self.fold_again) {
            self.reload_fold(cx);
        }
        cx.notify();
    }
}
