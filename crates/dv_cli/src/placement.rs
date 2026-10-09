//! Where threads stand in a review's current diff (docs/PLAN.md §8), as the GUI shows them:
//! the review's target resolved now (all changes, never an interdiff) in its home worktree
//! (`Repo::for_review`, PLAN §2.2), its file list, and the full text of each side a
//! thread is anchored on (`file_diff` without caps), handed to
//! `dv_core::review::place_thread`, the one function the GUI places threads with too.
//!
//! A write on a thread (`reply`, `address`, `resolve`, `reopen`) records where relocation found
//! it ([`Sides::relocated`], PLAN §8), and `comment add` anchors on a side's text read the same
//! way ([`Sides::side_text`]).

use std::collections::HashMap;

use dv_core::diff::{DiffLimits, FileDiff, FileEntry, file_diff, file_list};
use dv_core::git::GitRepo;
use dv_core::review::{
    Anchor, AnchorPlace, AnchorSide, Placement, Review, SideText, Thread, anchored_entry,
    place_thread, relocated_anchor, side_lines,
};
use dv_core::target::DiffSpec;

use crate::context::Context;
use crate::dto::{CurrentAnchorDto, Located, side_str};
use crate::error::CliError;
use crate::repo::Repo;

/// A review's current diff: its file list, and the diffs of the files read so far.
pub(crate) struct Sides {
    git: GitRepo,
    spec: DiffSpec,
    pub entries: Vec<FileEntry>,
    diffs: HashMap<String, FileDiff>,
    /// The round in progress, which a relocated anchor is taken in.
    round: u32,
}

impl Sides {
    /// The file list of `review`'s target as it resolves now (blocking: the target's git calls
    /// and one file list). A target that no longer resolves is `target` (exit 1).
    pub fn list(repo: &Repo, review: &Review, ctx: &Context) -> Result<Self, CliError> {
        let repo = repo.for_review(review, ctx)?;
        let spec = review.target.resolve(&repo.git).map_err(CliError::target)?;
        let entries = file_list(&repo.git, &spec)?;
        Ok(Self {
            git: repo.git,
            spec,
            entries,
            diffs: HashMap::new(),
            round: review.current_round(),
        })
    }

    /// Reads the diff of the listed file at `path` (an entry's `path`) unless it is read already
    /// (blocking: one diff).
    pub fn read(&mut self, path: &str) -> Result<&FileDiff, CliError> {
        if !self.diffs.contains_key(path) {
            let entry = self
                .entries
                .iter()
                .find(|entry| entry.path == path)
                .ok_or_else(|| {
                    CliError::not_found(format!("{path} isn't among the review's changed files"))
                })?;
            let diff = file_diff(&self.git, &self.spec, entry, &DiffLimits::UNLIMITED)?;
            self.diffs.insert(path.to_string(), diff);
        }
        Ok(&self.diffs[path])
    }

    /// Reads the diff of the file each of `threads` is anchored in.
    pub fn read_for(&mut self, threads: &[&Thread]) -> Result<(), CliError> {
        let paths: Vec<String> = threads
            .iter()
            .filter_map(|thread| anchored_entry(&thread.anchor, &self.entries))
            .map(|entry| entry.path.clone())
            .collect();
        for path in paths {
            self.read(&path)?;
        }
        Ok(())
    }

    /// `side` of the listed file at `path`, line 1 first, when its diff is read and is text.
    pub fn side_text(&self, path: &str, side: AnchorSide) -> Option<Vec<&str>> {
        self.diffs
            .get(path)
            .and_then(FileDiff::as_text)
            .map(|text| side_lines(text, side))
    }

    /// Where `thread` stands now, from the diffs read so far (a file not read counts as one
    /// without text, so read the threads' files first: [`Sides::read_for`]).
    pub fn locate(&self, thread: &Thread) -> Located {
        let side = thread.anchor.side;
        let text = |entry: &FileEntry| match self.side_text(&entry.path, side) {
            Some(lines) => SideText::Lines(lines),
            None => SideText::Unavailable,
        };
        let placement = place_thread(thread, &self.entries, |entry, _| text(entry));
        let file = placement.file().map(str::to_string);
        let entry = anchored_entry(&thread.anchor, &self.entries);
        let current = match (placement.lines(&thread.anchor), entry) {
            (Some((start, end)), Some(entry)) => match text(entry) {
                SideText::Lines(lines) => {
                    let code = lines
                        .get((start as usize).saturating_sub(1)..end as usize)
                        .map(|lines| lines.join("\n"))
                        .unwrap_or_default();
                    let place = AnchorPlace::for_entry(entry, side, self.round);
                    Some((
                        CurrentAnchorDto {
                            path: place.path,
                            side: side_str(side),
                            start_line: start,
                            end_line: end,
                        },
                        code,
                    ))
                }
                SideText::Pending | SideText::Unavailable => None,
            },
            _ => None,
        };
        Located {
            placement: match placement {
                // Every side is read in full here, so nothing is left pending.
                Placement::Pending { file } => Placement::Outdated { file: Some(file) },
                other => other,
            },
            file,
            current,
        }
    }

    /// The anchor a write on `thread` records (PLAN §8): its anchor re-taken where relocation
    /// found it, in the round in progress with fresh context, when `located` says it moved;
    /// `None` otherwise.
    pub fn relocated(&self, thread: &Thread, located: &Located) -> Option<Anchor> {
        let entry = anchored_entry(&thread.anchor, &self.entries)?;
        let lines = self.side_text(&entry.path, thread.anchor.side)?;
        relocated_anchor(
            &thread.anchor,
            &located.placement,
            entry,
            &lines,
            self.round,
        )
    }
}

/// Where each of `threads` (threads of `review`) stands now, in the same order. Blocking: one
/// file list and one diff per file a thread is anchored in.
pub(crate) fn locate(
    repo: &Repo,
    review: &Review,
    threads: &[&Thread],
    ctx: &Context,
) -> Result<Vec<Located>, CliError> {
    let mut sides = Sides::list(repo, review, ctx)?;
    sides.read_for(threads)?;
    Ok(threads.iter().map(|thread| sides.locate(thread)).collect())
}

/// Where `thread` (a thread of `review`) stands now, and the anchor a write on it records when
/// it moved ([`Sides::relocated`]).
pub(crate) fn locate_for_write(
    repo: &Repo,
    review: &Review,
    thread: &Thread,
    ctx: &Context,
) -> Result<(Located, Option<Anchor>), CliError> {
    let mut sides = Sides::list(repo, review, ctx)?;
    sides.read_for(&[thread])?;
    let located = sides.locate(thread);
    let relocated = sides.relocated(thread, &located);
    Ok((located, relocated))
}
