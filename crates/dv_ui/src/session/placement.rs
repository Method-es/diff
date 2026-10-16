//! Where the stream shows a thread now (PLAN §8, DESIGN §4.1): `dv_core::review::place_thread`,
//! the one placement the CLI uses too, over the review's files and the diffs the session shows,
//! plus the interdiff's own rule.
//!
//! **The file.** A thread belongs to the file of the review's changes its anchor names
//! (`dv_core::review::thread_file`: its path or a rename of it, else what its round's history
//! says the file is called now, `history`): in all changes that is the list shown; in an
//! interdiff it is the target's own list (all changes), which the session keeps beside the
//! interdiff's, so a thread is in the same file, or outdated, whichever comparison is shown, as
//! the CLI (which always works on all changes) says. Until a list has landed (the first listing
//! is under way, or it failed) no file has left the review: every thread is `Pending` without a
//! file, never in the stream's "Not in the diff" section; one waiting for its round's history
//! is `Pending` too, so it never shows there, or heading its file, before it jumps — unless it
//! was placed before: then it keeps that placement until the history lands, so its card, and a
//! reply being typed in it, don't go.
//!
//! **Files the review doesn't change.** A thread whose file left the review's list (it never
//! changed, or stopped changing) is placed against that file's text at the review's new side
//! (`dv_core::review::place_unlisted`, read in the background, `side`): found there, it is
//! `Placement::Unlisted`, shown in the "Not in the diff" section as on a file the review doesn't
//! change and in that file's file view at its lines; else outdated without a file. Until the
//! read lands it is outdated in the section, as before it.
//!
//! **Interdiff.** Its new side is the same text as in all changes, so a new-side thread whose
//! file is also in the interdiff places exactly as there. The old column is round n's
//! snapshot, which no anchor names: old-side threads, and threads whose file isn't in the
//! interdiff (one the agent reverted after the round is in the interdiff but not among the
//! review's changes: its threads are outdated, as in all changes), are
//! [`ThreadPlacement::NotInComparison`] — not drawn in the stream, and listed in the panel with
//! an "all changes" tag (DESIGN §4.1). They are still placed, against the review's own diff of
//! their file (read in the background, `load`), so the panel, "Copy as prompt" and the sheet
//! name where they stand in all changes, as the CLI does.
//!
//! **Text.** A thread the stream draws is placed against the diff the stream shows for its
//! file ([`ReviewSession::shown`]): while a file loads again it is the diff it had, except that
//! an old side listed under the other comparison (round n's snapshot rather than the target's
//! old side, or the other way round) can't be placed against, so such a thread waits
//! (`Pending`) until its file's new diff lands. Every file a thread shows in loads whatever
//! the stream's viewport (`load`), so no thread waits long, nor stays placed against a diff
//! its file no longer has. Where the diff shown holds no text to read a side from although
//! the review's own diff of the file may — a file too large to show — that diff (the target
//! resolved, without caps: what the CLI reads) is read in the background for the threads
//! anchored there (`load`), so they are placed as the CLI places them, and wait (`Pending`)
//! until that read lands.
//!
//! **Cost.** Relocation can run its fuzzy step over a whole side, so each thread's placement is
//! kept until something it was worked out from changes: its anchor, the comparison, the file
//! that holds it (path, rename and status), whether the stream draws it there, or the text it
//! was placed against (which diff, and whether a read is still under way), and its round's
//! history (which read, `history`). A refresh that keeps
//! a file's diff keeps its threads' placements. The fuzzy step's form of a side is built once
//! and shared by every thread on it (`dv_core::review::FuzzyLines`).

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use dv_core::diff::{FileDiff, FileEntry, FileStatus};
use dv_core::review::{
    Anchor, AnchorPlace, AnchorSide, FuzzyLines, History, Placement, SideLines, SideText, Thread,
    ThreadFile, ThreadId, ThreadState, Trail, place_thread, place_unlisted, relocated_anchor,
    shows_edited, shows_moved, side_lines, thread_file, unlisted_path,
};

use super::history::HistoryKey;
use super::load::FullRead;
use super::{FileState, ReviewSession};
use crate::session::Comparison;
use crate::thread::{file_location, location_label, path_lines};

/// Where a thread shows in the session's stream.
#[derive(Clone, Debug, PartialEq)]
pub enum ThreadPlacement {
    /// Placed among the files shown ([`dv_core::review::place_thread`]).
    Placed(Placement),
    /// The stream shows an interdiff that doesn't hold it: an old-side thread, or one whose
    /// file isn't in the interdiff (module docs). It isn't drawn; the placement is where it
    /// stands in all changes, which the panel names.
    NotInComparison(Placement),
    /// Its file left the review's list and the file at the new side isn't read yet (`side`):
    /// `Pending` inside, drawn in the "Not in the diff" section meanwhile, neutrally (neither
    /// outdated nor placed), so its card, and a reply being typed in it, stay there.
    Reading(Placement),
}

impl ThreadPlacement {
    /// The placement itself, drawn or not.
    pub fn placement(&self) -> &Placement {
        match self {
            Self::Placed(placement)
            | Self::NotInComparison(placement)
            | Self::Reading(placement) => placement,
        }
    }

    /// The listed file the stream draws it in (as an outdated card when it has no lines
    /// there); `None` when the stream doesn't draw it.
    pub fn file(&self) -> Option<&str> {
        match self {
            Self::Placed(placement) => placement.file(),
            Self::NotInComparison(_) | Self::Reading(_) => None,
        }
    }

    /// The lines on its side it stands at now, `(start, end)`: `anchor`'s own while
    /// unchanged, where it moved to, else `None`. For a thread the interdiff doesn't show,
    /// where it stands in all changes.
    pub fn lines(&self, anchor: &Anchor) -> Option<(u32, u32)> {
        self.placement().lines(anchor)
    }

    /// Relocation can't find it (PLAN §8 step 6), or its file left the diff.
    pub fn is_outdated(&self) -> bool {
        self.placement().is_outdated()
    }

    /// Its file left the review's list, whichever comparison is shown: outdated without a
    /// file, placed in that file at the review's new side (`Placement::Unlisted`, a comment on
    /// the whole file `Placement::UnlistedFile`), or waiting for that file's read. The stream
    /// draws it in its "Not in the diff" section (DESIGN §4.1).
    pub fn is_not_in_diff(&self) -> bool {
        matches!(
            self,
            Self::Placed(
                Placement::Outdated { file: None, .. }
                    | Placement::Unlisted { .. }
                    | Placement::UnlistedFile { .. }
            ) | Self::Reading(_)
        )
    }

    /// It stands in a file the review doesn't change (`Placement::Unlisted`, or a comment on
    /// its whole: `Placement::UnlistedFile`): placed, not outdated, in no listed file; the file
    /// view of that file draws it.
    pub fn is_unlisted(&self) -> bool {
        matches!(
            self,
            Self::Placed(Placement::Unlisted { .. } | Placement::UnlistedFile { .. })
        )
    }

    /// Where an outdated thread's code went since its round, when its snapshot tells (PLAN
    /// §8): the stream draws its card there.
    pub fn trail(&self) -> Option<&Trail> {
        self.placement().trail()
    }

    /// The lines it is named by: where it stands ([`ThreadPlacement::lines`]), else for an
    /// outdated thread with a trail the lines its code went to.
    pub fn named_lines(&self, anchor: &Anchor) -> Option<(u32, u32)> {
        self.lines(anchor)
            .or_else(|| self.trail().map(Trail::lines))
    }

    /// A comment on the whole file, standing in its file (`Placement::File`).
    pub fn is_file(&self) -> bool {
        self.placement().is_file()
    }

    /// Its file's text, or its round's history, isn't there to place it against yet.
    pub fn is_pending(&self) -> bool {
        matches!(self.placement(), Placement::Pending { .. })
    }

    /// The interdiff shown doesn't hold it (the "all changes" tag).
    pub fn is_not_in_comparison(&self) -> bool {
        matches!(self, Self::NotInComparison(_))
    }

    /// Whether `thread`, placed here, shows "· moved" (`dv_core::review::shows_moved`: found
    /// elsewhere now, or standing where a recorded move put it). Only a drawn thread shows.
    pub fn shows_moved(&self, thread: &Thread) -> bool {
        match self {
            Self::Placed(placement) => shows_moved(thread, placement),
            Self::NotInComparison(_) | Self::Reading(_) => false,
        }
    }

    /// `"unchanged"`, `"moved"`, `"edited"`, `"pending"`, `"outdated"` or
    /// `"not_in_comparison"`, as automation state spells it and as the CLI says (`moved` as
    /// [`ThreadPlacement::shows_moved`] says; `edited`, code edited where it stands, as
    /// `dv_core::review::shows_edited` says).
    pub fn key(&self, thread: &Thread) -> &'static str {
        match self {
            _ if self.shows_moved(thread) => "moved",
            Self::Placed(placement) if shows_edited(thread, placement) => "edited",
            Self::Placed(Placement::Moved { .. }) => "unchanged",
            Self::Placed(placement) | Self::Reading(placement) => placement.as_str(),
            Self::NotInComparison(_) => "not_in_comparison",
        }
    }
}

/// A file list with its lookups: by path, and by the path a file was renamed from.
#[derive(Default)]
pub(super) struct FileIndex {
    pub files: Vec<FileEntry>,
    by_path: HashMap<String, usize>,
    renamed_from: HashMap<String, usize>,
}

impl FileIndex {
    pub fn new(files: Vec<FileEntry>) -> Self {
        let by_path = files
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.path.clone(), index))
            .collect();
        let mut renamed_from = HashMap::new();
        // The first entry renamed from a path, as `anchored_entry` finds it.
        for (index, entry) in files.iter().enumerate() {
            if let Some(old) = &entry.old_path {
                renamed_from.entry(old.clone()).or_insert(index);
            }
        }
        Self {
            files,
            by_path,
            renamed_from,
        }
    }

    pub fn get(&self, path: &str) -> Option<&FileEntry> {
        self.by_path
            .get(path)
            .and_then(|&index| self.files.get(index))
    }

    fn renamed(&self, from: &str) -> Option<&FileEntry> {
        self.renamed_from
            .get(from)
            .and_then(|&index| self.files.get(index))
    }

    /// The entry `anchor` belongs to: `dv_core::review::anchored_entry`'s rule, through the
    /// indexes.
    pub fn anchored(&self, anchor: &Anchor) -> Option<&FileEntry> {
        self.lookup(&anchor.path, anchor.old_path.as_deref())
    }

    /// The entry for a file at `path`, renamed from `old_path` when it was written:
    /// `dv_core::review::entry_for`'s rule, through the indexes.
    pub fn lookup(&self, path: &str, old_path: Option<&str>) -> Option<&FileEntry> {
        self.get(path)
            .or_else(|| self.renamed(path))
            .or_else(|| old_path.and_then(|old| self.renamed(old)))
    }
}

/// What a placement was worked out from (module docs, "Cost").
#[derive(Clone, PartialEq)]
struct Key {
    anchor: Anchor,
    comparison: Comparison,
    /// The review's file that holds the thread: its path, rename and status.
    entry: Option<(String, Option<String>, FileStatus)>,
    /// Whether the stream draws it in that file.
    drawn: bool,
    /// The diff its side is read from ([`ReviewSession::basis`]), held so its address can't be
    /// reused; `None` while there is none yet.
    diff: Option<Arc<FileDiff>>,
    /// Whether a file list has landed: before one, no thread's file has left the review.
    listed: bool,
    /// Its round's history, as read (`history`).
    history: HistoryKey,
    /// The read of its file at the new side (`side`), for a thread whose file left the list.
    side: Option<u64>,
}

impl Key {
    fn same(&self, other: &Key) -> bool {
        let diff = match (&self.diff, &other.diff) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        diff && self.anchor == other.anchor
            && self.comparison == other.comparison
            && self.entry == other.entry
            && self.drawn == other.drawn
            && self.listed == other.listed
            && self.history == other.history
            && self.side == other.side
    }
}

/// A placement and what it was worked out from.
pub(super) struct Cached {
    key: Key,
    placement: ThreadPlacement,
    /// It is the placement worked out before, held while only the thread's round history is
    /// missing (module docs, "The file").
    held: bool,
}

/// A side's lines with the fuzzy table the session keeps for that side.
pub(super) struct KeptSide<'a> {
    lines: Vec<&'a str>,
    fuzzy: Rc<FuzzyLines>,
}

impl<'a> SideLines for KeptSide<'a> {
    type Line = &'a str;

    fn lines(&self) -> &[&'a str] {
        &self.lines
    }

    fn fuzzy(&self) -> Option<&FuzzyLines> {
        Some(&self.fuzzy)
    }
}

/// The fuzzy tables kept per file and side, with the diff each was read from.
pub(super) type FuzzyTables = HashMap<(String, AnchorSide), (Arc<FileDiff>, Rc<FuzzyLines>)>;

impl ReviewSession {
    /// Where `thread` shows now (module docs).
    pub fn placement(&self, thread: &Thread) -> ThreadPlacement {
        let key = self.placement_key(thread);
        let earlier = {
            let placements = self.placements.borrow();
            let cached = placements.get(&thread.id);
            if let Some(cached) = cached
                && cached.key.same(&key)
            {
                return cached.placement.clone();
            }
            cached
                .map(|cached| cached.placement.clone())
                .filter(|earlier| !earlier.is_pending())
        };
        let history = self.history_for(thread);
        let mut placement = self.place_with(thread, history);
        let mut held = false;
        // Waiting for its round's history alone (placed without one, it isn't waiting), a
        // thread placed before keeps that placement: its card, and a reply being typed in it,
        // stay until the history lands.
        if placement.is_pending()
            && !matches!(history, History::None)
            && let Some(earlier) = earlier
            && !self.place_with(thread, History::None).is_pending()
        {
            placement = earlier;
            held = true;
        }
        self.placements.borrow_mut().insert(
            thread.id.clone(),
            Cached {
                key,
                placement: placement.clone(),
                held,
            },
        );
        placement
    }

    /// Whether `thread`'s placement is one held while its round's history is missing
    /// ([`ReviewSession::placement`]).
    pub(super) fn placement_held(&self, thread: &Thread) -> bool {
        self.placements
            .borrow()
            .get(&thread.id)
            .is_some_and(|cached| cached.held)
    }

    fn placement_key(&self, thread: &Thread) -> Key {
        let entry = self.thread_entry(thread);
        let drawn = entry.is_some_and(|entry| self.draws(thread, entry));
        // A comment on the whole file is placed without its file's text.
        let diff = entry
            .filter(|_| !thread.anchor.is_file_level())
            .and_then(|entry| self.basis(&entry.path, thread.anchor.side, drawn))
            .cloned();
        Key {
            anchor: thread.anchor.clone(),
            comparison: self.comparison,
            entry: entry.map(|entry| (entry.path.clone(), entry.old_path.clone(), entry.status)),
            drawn,
            diff,
            listed: self.listed,
            history: self.history_key(thread),
            side: self.side_version(&unlisted_path(&thread.anchor, self.history_for(thread))),
        }
    }

    /// How many placements were worked out rather than kept (tests).
    #[cfg(test)]
    pub(super) fn placements_worked_out(&self) -> usize {
        self.worked_out.get()
    }

    fn place_with(&self, thread: &Thread, history: History<'_>) -> ThreadPlacement {
        #[cfg(test)]
        self.worked_out.set(self.worked_out.get() + 1);
        let entry = match self.thread_file_with(thread, history) {
            ThreadFile::Found(entry) => entry,
            // Its round's history would say what its file is called now: it waits for it.
            ThreadFile::Pending(entry) => {
                return ThreadPlacement::Placed(Placement::Pending {
                    file: entry.map(|entry| entry.path.clone()),
                });
            }
            // No file list has landed (the first is under way, or it failed): nothing says its
            // file left the review, so it waits as one whose text isn't there yet.
            ThreadFile::Gone if !self.listed => {
                return ThreadPlacement::Placed(Placement::Pending { file: None });
            }
            // Its file left the review: placed in that file at the new side, or outdated,
            // whichever comparison is shown; drawn in the stream's "Not in the diff" section,
            // where it waits (neutral, `Reading`) until that file's read lands.
            ThreadFile::Gone => {
                let path = unlisted_path(&thread.anchor, history);
                let present = self.side_present(&path);
                return match place_unlisted(thread, &path, present, self.side_text(&path)) {
                    pending @ Placement::Pending { .. } => ThreadPlacement::Reading(pending),
                    placement => ThreadPlacement::Placed(placement),
                };
            }
        };
        let drawn = self.draws(thread, entry);
        let basis = self.basis(&entry.path, thread.anchor.side, drawn);
        let placement = place_thread(
            thread,
            std::slice::from_ref(entry),
            history,
            |entry, side| match basis {
                Some(diff) => self.lines_of(&entry.path, side, diff),
                None => SideText::Pending,
            },
        );
        if drawn {
            ThreadPlacement::Placed(placement)
        } else {
            ThreadPlacement::NotInComparison(placement)
        }
    }

    /// Whether the stream draws `thread`, whose file of the review's changes is `entry`: its
    /// file is listed, and it isn't an old-side thread in an interdiff (module docs).
    pub(super) fn draws(&self, thread: &Thread, entry: &FileEntry) -> bool {
        let old_column = self.comparison.is_interdiff() && thread.anchor.side == AnchorSide::Old;
        !old_column && self.file(&entry.path).is_some()
    }

    /// The paths of the listed files the stream draws threads in: they load whatever the
    /// stream's viewport (`load`). A comment on the whole file needs no text, so its file
    /// loads as any other.
    pub(super) fn thread_files(&self) -> Vec<String> {
        let mut paths: Vec<String> = Vec::new();
        for thread in self.review.live_threads() {
            if !thread.anchor.is_file_level()
                && let Some(entry) = self.thread_entry(thread)
                && self.draws(thread, entry)
                && !paths.contains(&entry.path)
            {
                paths.push(entry.path.clone());
            }
        }
        paths
    }

    /// The file of the review's changes `thread` belongs to (module docs, "The file"): in the
    /// list shown, or in an interdiff the target's own list; `None` while its round's history,
    /// which would say, isn't read.
    pub fn thread_entry(&self, thread: &Thread) -> Option<&FileEntry> {
        match self.thread_file_with(thread, self.history_for(thread)) {
            ThreadFile::Found(entry) => Some(entry),
            ThreadFile::Pending(_) | ThreadFile::Gone => None,
        }
    }

    /// `dv_core::review::thread_file` over the review's files, with `history`.
    pub(super) fn thread_file_with(&self, thread: &Thread, history: History<'_>) -> ThreadFile<'_> {
        let index = self.target_index();
        thread_file(&thread.anchor, history, |path, old| index.lookup(path, old))
    }

    /// The listed file `anchor` belongs to among the files shown (`dv_core::review::
    /// anchored_entry`'s rule, through the list's indexes).
    pub fn anchored_entry(&self, anchor: &Anchor) -> Option<&FileEntry> {
        self.list.anchored(anchor)
    }

    /// Whether `path` is one of the review's changed files (all changes), whichever
    /// comparison is shown.
    pub fn in_review(&self, path: &str) -> bool {
        self.target_index().get(path).is_some()
    }

    /// The diff `side` of `path` (a file of the review's changes) is placed against (module
    /// docs, "Text"): for a thread the stream draws (`drawn`), [`ReviewSession::placement_diff`];
    /// for one it doesn't, the review's own diff of the file once it is read. `None` until
    /// there is one.
    pub(super) fn basis(
        &self,
        path: &str,
        side: AnchorSide,
        drawn: bool,
    ) -> Option<&Arc<FileDiff>> {
        if drawn {
            return self.placement_diff(path, side);
        }
        match self.review_text(path)? {
            FullRead::Loaded(full) => Some(full),
            FullRead::Loading | FullRead::Failed => None,
        }
    }

    /// The diff `side` of `path` is placed against in the stream (module docs, "Text"), which
    /// comments anchor on too: the diff shown, or the one it had while it loads again, or where
    /// that holds no text to read, the review's own diff of the file once that is read (its
    /// shown diff when that read failed: the diff shown is all there is). `None` until there is
    /// one.
    pub(super) fn placement_diff(&self, path: &str, side: AnchorSide) -> Option<&Arc<FileDiff>> {
        let shown = self.shown_for_placement(path, side)?;
        if !self.needs_full_text(shown) {
            return Some(shown);
        }
        match self.review_text(path)? {
            FullRead::Loaded(full) => Some(full),
            FullRead::Failed => Some(shown),
            FullRead::Loading => None,
        }
    }

    /// The diff shown for `path` as placement may read it: the loaded one, or while it loads
    /// again the one it had (not an old side listed under the other comparison).
    fn shown_for_placement(&self, path: &str, side: AnchorSide) -> Option<&Arc<FileDiff>> {
        match self.states.get(path)? {
            FileState::Loaded { diff, .. } => Some(diff),
            FileState::Unloaded | FileState::Loading { .. } => self
                .previous
                .get(path)
                .filter(|previous| {
                    side == AnchorSide::New || previous.comparison == self.comparison
                })
                .map(|previous| &previous.shown.diff),
            FileState::Failed(_) => None,
        }
    }

    /// `diff`'s `side` of `path` as placement reads it: its lines (with the side's fuzzy
    /// table), or `Unavailable` for a diff without lines (binary, too large, a submodule,
    /// unmerged).
    pub(super) fn lines_of<'a>(
        &'a self,
        path: &str,
        side: AnchorSide,
        diff: &'a Arc<FileDiff>,
    ) -> SideText<KeptSide<'a>> {
        match diff.as_text() {
            Some(text) => SideText::Lines(KeptSide {
                lines: side_lines(text, side),
                fuzzy: self.fuzzy_table(path, side, diff),
            }),
            None => SideText::Unavailable,
        }
    }

    /// The fuzzy table of `path`'s `side` as `diff` has it, made (empty, filled on first use)
    /// when there is none for that diff yet.
    fn fuzzy_table(&self, path: &str, side: AnchorSide, diff: &Arc<FileDiff>) -> Rc<FuzzyLines> {
        let mut tables = self.fuzzy.borrow_mut();
        let key = (path.to_string(), side);
        match tables.get(&key) {
            Some((kept, table)) if Arc::ptr_eq(kept, diff) => table.clone(),
            _ => {
                let table = Rc::new(FuzzyLines::default());
                tables.insert(key, (diff.clone(), table.clone()));
                table
            }
        }
    }

    /// The live threads that show in `path`'s rows (inline, or as an outdated card heading
    /// it), in the order they were added.
    pub fn threads_in<'a>(&'a self, path: &'a str) -> impl Iterator<Item = &'a Thread> + 'a {
        self.review
            .live_threads()
            .filter(move |thread| self.placement(thread).file() == Some(path))
    }

    /// The live threads whose file left the review's list ([`ThreadPlacement::is_not_in_diff`]),
    /// in the order they were added: the stream's "Not in the diff" section (DESIGN §4.1).
    pub fn threads_not_in_diff(&self) -> impl Iterator<Item = &Thread> + '_ {
        self.review
            .live_threads()
            .filter(move |thread| self.placement(thread).is_not_in_diff())
    }

    /// The listed file `thread` shows in, if any.
    pub fn file_of(&self, thread: &Thread) -> Option<&FileEntry> {
        self.placement(thread)
            .file()
            .and_then(|path| self.file(path))
    }

    /// Open threads showing in `path`.
    pub fn open_threads_in(&self, path: &str) -> usize {
        self.threads_in(path)
            .filter(|thread| thread.state == ThreadState::Open)
            .count()
    }

    /// `path:31`, `path:12–14`: where `thread` stands now (its side's path in the file that
    /// holds it, at the lines it shows at; for a thread the interdiff doesn't show, where it
    /// stands in all changes; for an outdated one with a trail, where its code went, PLAN §8),
    /// else where it was anchored. An old-side thread's ends in "(old)",
    /// as the CLI prints it, so its lines aren't taken for the file's lines now. The comments
    /// list, the "Finish round" sheet and "Copy as prompt" all name a thread so (DESIGN §4.1);
    /// [`ThreadPlacement::is_pending`] says when it hasn't been placed yet. A comment on the
    /// whole file is `path (file)` under its file's name now (`(file, old)` on the old side).
    pub fn location(&self, thread: &Thread) -> String {
        let placement = self.placement(thread);
        let entry = self.thread_entry(thread);
        if thread.anchor.is_file_level() {
            let path = entry.map_or_else(
                || thread.anchor.path.clone(),
                |entry| AnchorPlace::for_entry(entry, thread.anchor.side, 0).path,
            );
            return file_location(&path, thread.anchor.side);
        }
        let label = match (entry, placement.named_lines(&thread.anchor)) {
            (Some(entry), Some((start, end))) => {
                let place = AnchorPlace::for_entry(entry, thread.anchor.side, 0);
                path_lines(&place.path, start, end)
            }
            // In a file the review doesn't change.
            (None, Some((start, end))) => match placement.placement().unlisted_path() {
                Some(path) => path_lines(path, start, end),
                None => location_label(thread),
            },
            _ => location_label(thread),
        };
        match thread.anchor.side {
            AnchorSide::Old => format!("{label} (old)"),
            AnchorSide::New => label,
        }
    }

    /// The anchor a reviewer's write on `thread` records (PLAN §8): its anchor re-taken where
    /// relocation found it, in the round in progress with fresh context, while it shows moved
    /// now, or under its file's name now when that file was renamed since
    /// (`dv_core::review::relocated_anchor`); `None` otherwise. Only from text that is current:
    /// not the diff a file had while it loads again. The review's own entry for the file names
    /// its paths, as the CLI's writes do, whichever comparison is shown. A comment on the whole
    /// file reads no text: it is re-taken under its file's name now when that was renamed.
    pub fn relocated_anchor(&self, thread: &ThreadId) -> Option<Anchor> {
        let thread = self.review.thread(thread)?;
        let entry = self.thread_entry(thread)?;
        if thread.anchor.is_file_level() {
            return relocated_anchor(
                &thread.anchor,
                self.placement(thread).placement(),
                entry,
                &[] as &[&str],
                self.review.current_round(),
            );
        }
        let drawn = self.draws(thread, entry);
        if drawn && !matches!(self.states.get(&entry.path), Some(FileState::Loaded { .. })) {
            return None;
        }
        let placement = self.placement(thread);
        let diff = self.basis(&entry.path, thread.anchor.side, drawn)?;
        let text = diff.as_text()?;
        relocated_anchor(
            &thread.anchor,
            placement.placement(),
            entry,
            &side_lines(text, thread.anchor.side),
            self.review.current_round(),
        )
    }

    /// Drops the fuzzy tables of diffs no longer read for placement.
    pub(super) fn forget_fuzzy_tables(&mut self) {
        let current: Vec<((String, AnchorSide), Arc<FileDiff>)> = self
            .fuzzy
            .get_mut()
            .iter()
            .map(|(key, (diff, _))| (key.clone(), diff.clone()))
            .collect();
        for ((path, side), diff) in current {
            let read = match self.review_text(&path) {
                Some(FullRead::Loaded(full)) => Arc::ptr_eq(full, &diff),
                _ => false,
            };
            let kept = read
                || self
                    .placement_diff(&path, side)
                    .is_some_and(|now| Arc::ptr_eq(now, &diff));
            if !kept {
                self.fuzzy.get_mut().remove(&(path, side));
            }
        }
    }
}
