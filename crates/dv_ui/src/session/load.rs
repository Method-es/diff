//! The session's loads: the file list (with the spec and the labels), file diffs on demand,
//! their highlighting, and the fold.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use dv_core::diff::{DiffLimits, FileDiff, FileEntry, file_diff, file_list};
use dv_core::review::LoadedReview;
use dv_core::{DiffSpec, GitRepo, Side, Target};
use gpui::{AppContext as _, Context};

use super::{FileState, MAX_LOADS_IN_FLIGHT, ReviewSession, SessionEvent, Shown};
use crate::failure::Failure;
use crate::services::reviews::LabelContext;
use crate::syntax::{FileHighlights, highlight_diff, language_for_path};

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
    spec: DiffSpec,
    files: Vec<FileEntry>,
    /// The branch labels, when they were read again (`None` keeps the ones the session has).
    labels: Option<LabelContext>,
}

/// Resolves `target` and lists its files (blocking). The branch labels name HEAD and the
/// default branch, which only the git state moves: they are read again when `read_labels`
/// (a git change, a rescan, the first list) or when the spec moved since `previous`, never for
/// a batch of worktree paths alone.
fn list_blocking(
    repo: &GitRepo,
    target: &Target,
    previous: Option<&DiffSpec>,
    read_labels: bool,
) -> Result<Listed, Failure> {
    let spec = target
        .resolve(repo)
        .map_err(|error| Failure::from(&error))?;
    let files = file_list(repo, &spec).map_err(|error| Failure::from(&error))?;
    let labels = (read_labels || previous != Some(&spec)).then(|| LabelContext::read(repo));
    Ok(Listed {
        spec,
        files,
        labels,
    })
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
        let previous = self.spec.clone();
        let read_labels = batch.git || batch.rescan || !self.listed;
        let job = cx.background_spawn(async move {
            list_blocking(&repo, &target, previous.as_ref(), read_labels)
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
                files,
                labels,
            }) => {
                let old: HashMap<&str, &FileEntry> = self
                    .files
                    .iter()
                    .map(|entry| (entry.path.as_str(), entry))
                    .collect();
                let worktree = spec.new == Side::Worktree;
                let mut states = HashMap::with_capacity(files.len());
                for entry in &files {
                    let keep = !batch.rescan
                        && old
                            .get(entry.path.as_str())
                            .is_some_and(|old| unchanged(old, entry, worktree))
                        && !batch.touches(entry);
                    let state = match (keep, self.states.remove(&entry.path)) {
                        (true, Some(state)) => state,
                        // It loads again; the diff it showed stays until the new one lands.
                        (false, Some(FileState::Loaded { diff, highlights })) => {
                            self.previous
                                .insert(entry.path.clone(), Shown { diff, highlights });
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
                self.files = files;
                self.spec = Some(spec);
                if let Some(labels) = labels {
                    self.labels = labels;
                }
                self.listed = true;
                self.error = None;
                cx.emit(SessionEvent::FilesChanged);
                self.requeue_wanted();
                self.pump(cx);
                self.ensure_watch(cx);
            }
            Err(error) => {
                log::warn!("review {}: refresh failed: {error}", self.review.id);
                self.error = Some(error);
                cx.emit(SessionEvent::Error);
                self.ensure_watch(cx);
            }
        }
        if let Some(queued) = self.queued.take() {
            self.refresh(queued, cx);
        }
        cx.notify();
    }

    /// Asks for these files' diffs (the stream's viewport and a small prefetch). They load in
    /// order, at most [`MAX_LOADS_IN_FLIGHT`] at once; files asked for before and no longer
    /// wanted stop waiting.
    pub fn ensure_loaded(
        &mut self,
        paths: impl IntoIterator<Item = String>,
        cx: &mut Context<Self>,
    ) {
        self.wanted = paths.into_iter().collect();
        let wanted = &self.wanted;
        self.queue.retain(|path| wanted.contains(path));
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
            self.previous
                .insert(path.to_string(), Shown { diff, highlights });
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

    fn requeue_wanted(&mut self) {
        for path in &self.wanted {
            if matches!(self.states.get(path), Some(FileState::Unloaded))
                && !self.queue.contains(path)
            {
                self.queue.push_back(path.clone());
            }
        }
    }

    /// Starts queued loads while there is room.
    fn pump(&mut self, cx: &mut Context<Self>) {
        if self.spec.is_none() {
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
        if current {
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
        if diff.as_text().is_none() || !has_grammar {
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
    /// (`writes`).
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
                    self.review = loaded.review;
                    self.warnings = loaded.warnings;
                    cx.emit(SessionEvent::ReviewChanged);
                }
            }
            Err(error) => {
                log::warn!("review {}: reload failed: {error}", self.review.id);
                self.fold_error = Some(error);
                cx.emit(SessionEvent::Error);
            }
        }
        if std::mem::take(&mut self.fold_again) {
            self.reload_fold(cx);
        }
        cx.notify();
    }
}
