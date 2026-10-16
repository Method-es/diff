//! Where threads stand in a review's current diff (docs/PLAN.md §8), as the GUI shows them:
//! the review's target resolved now (all changes, never an interdiff) in its home worktree
//! (`Repo::for_review`, PLAN §2.2), its file list, and the full text of each side a
//! thread is anchored on (`file_diff` without caps), handed to
//! `dv_core::review::place_thread`, the one function the GUI places threads with too. A thread
//! found there carries the excerpt of that same diff around its lines (`dv_core::review::excerpt`,
//! PLAN §10.3).
//!
//! A new-side thread of a closed round that the current text alone can't place (its file not
//! listed, or relocation calls it outdated) is read against its round's snapshot
//! (`dv_core::review::read_round_history`, [`Sides::read_history_for`]): only those rounds, for
//! those threads' paths, so its file is followed through a rename since and an outdated one
//! says where its code went, with the excerpt around the lines that replaced it.
//!
//! A write on a thread (`reply`, `address`, `resolve`, `reopen`) records where relocation found
//! it ([`Sides::relocated`], PLAN §8), and `comment add` anchors on a side's text read the same
//! way ([`Sides::side_text`]).

use std::collections::{BTreeMap, HashMap};

use dv_core::diff::{DiffLimits, FileDiff, FileEntry, file_diff, file_list};
use dv_core::git::GitRepo;
use dv_core::review::{
    Anchor, AnchorPlace, AnchorSide, History, Placement, Review, RoundHistory, SideText, Thread,
    ThreadFile, Went, anchored_entry, entry_for, excerpt, excerpt_lines, place_thread,
    read_round_history, relocated_anchor, side_lines, thread_file,
};
use dv_core::target::DiffSpec;

use crate::context::Context;
use crate::dto::{CurrentAnchorDto, CurrentCode, Located, TrailCode, side_str};
use crate::error::CliError;
use crate::output;
use crate::repo::Repo;

/// A review's current diff: its file list, the diffs of the files read so far, and the round
/// histories read.
pub(crate) struct Sides {
    git: GitRepo,
    spec: DiffSpec,
    pub entries: Vec<FileEntry>,
    diffs: HashMap<String, FileDiff>,
    /// The round in progress, which a relocated anchor is taken in.
    round: u32,
    /// The rounds closed, which a thread's history is read from.
    closed: u32,
    histories: HashMap<u32, RoundHistory>,
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
            closed: review.closed_rounds(),
            histories: HashMap::new(),
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

    /// Reads the round histories `threads` (threads of `review`, their files read:
    /// [`Sides::read_for`]) need (module docs): for each closed round, the paths of its
    /// new-side threads that the current text alone leaves outdated; then the diff of each file
    /// a history says such a thread's file is now. A round whose history can't be read is
    /// warned about on stderr, and its threads are placed without it (PLAN §8).
    pub fn read_history_for(
        &mut self,
        review: &Review,
        threads: &[&Thread],
        ctx: &mut Context,
    ) -> Result<(), CliError> {
        let mut wanted: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        for thread in threads {
            let anchor = &thread.anchor;
            if self.history_round(anchor).is_none()
                || !self.place(thread, History::None).is_outdated()
            {
                continue;
            }
            let paths = wanted.entry(anchor.round).or_default();
            if !paths.contains(&anchor.path) {
                paths.push(anchor.path.clone());
            }
        }
        for (n, paths) in wanted {
            let Some(round) = review.rounds.get(n as usize - 1) else {
                continue;
            };
            match read_round_history(&self.git, &self.spec, round, &paths, None) {
                Ok(history) => {
                    self.histories.insert(n, history);
                }
                Err(error) => output::warning(
                    ctx,
                    &format!(
                        "couldn't read round {n}'s snapshot, so its threads are placed without \
                         it: {error}"
                    ),
                )?,
            }
        }
        let followed: Vec<String> = threads
            .iter()
            .filter_map(|thread| {
                match thread_file(&thread.anchor, self.history(thread), |path, old| {
                    entry_for(path, old, &self.entries)
                }) {
                    ThreadFile::Found(entry) => Some(entry.path.clone()),
                    ThreadFile::Pending(_) | ThreadFile::Gone => None,
                }
            })
            .collect();
        for path in followed {
            self.read(&path)?;
        }
        Ok(())
    }

    /// The closed round whose history `anchor` may be read against: its own, on the new side.
    fn history_round(&self, anchor: &Anchor) -> Option<u32> {
        (anchor.side == AnchorSide::New && anchor.round >= 1 && anchor.round <= self.closed)
            .then_some(anchor.round)
    }

    /// `thread`'s round history as read ([`Sides::read_history_for`]): none when it wasn't
    /// read for its path (the thread didn't need it).
    fn history(&self, thread: &Thread) -> History<'_> {
        let anchor = &thread.anchor;
        match self
            .history_round(anchor)
            .and_then(|round| self.histories.get(&round))
        {
            Some(history) if history.file(&anchor.path).is_some() => History::Read(history),
            _ => History::None,
        }
    }

    /// `side` of the listed file at `path`, line 1 first, when its diff is read and is text.
    pub fn side_text(&self, path: &str, side: AnchorSide) -> Option<Vec<&str>> {
        self.diffs
            .get(path)
            .and_then(FileDiff::as_text)
            .map(|text| side_lines(text, side))
    }

    /// `thread` placed against the diffs read so far (a file not read counts as one without
    /// text) with `history`.
    fn place(&self, thread: &Thread, history: History<'_>) -> Placement {
        let side = thread.anchor.side;
        let text = |entry: &FileEntry| match self.side_text(&entry.path, side) {
            Some(lines) => SideText::Lines(lines),
            None => SideText::Unavailable,
        };
        place_thread(thread, &self.entries, history, |entry, _| text(entry))
    }

    /// The listed entry at `path`.
    fn entry(&self, path: &str) -> Option<&FileEntry> {
        self.entries.iter().find(|entry| entry.path == path)
    }

    /// Where `thread` stands now, from the diffs and histories read so far (a file not read
    /// counts as one without text, so read the threads' files and histories first:
    /// [`Sides::read_for`], [`Sides::read_history_for`]).
    pub fn locate(&self, thread: &Thread) -> Located {
        let side = thread.anchor.side;
        let placement = self.place(thread, self.history(thread));
        let file = placement.file().map(str::to_string);
        let entry = file.as_deref().and_then(|path| self.entry(path));
        let current = match (placement.lines(&thread.anchor), entry) {
            (Some((start, end)), Some(entry)) => self.current_code(entry, side, start, end),
            _ => None,
        };
        let trail = match (placement.trail(), entry) {
            (Some(trail), Some(entry)) => Some(TrailCode {
                trail: *trail,
                path: AnchorPlace::for_entry(entry, AnchorSide::New, self.round).path,
                current: match trail.went {
                    Went::Replaced {
                        start_line,
                        end_line,
                    } => self.current_code(entry, AnchorSide::New, start_line, end_line),
                    Went::Removed { .. } => None,
                },
                at_end: match trail.went {
                    Went::Removed { before_line } => self
                        .side_text(&entry.path, AnchorSide::New)
                        .is_some_and(|lines| before_line as usize > lines.len()),
                    Went::Replaced { .. } => false,
                },
            }),
            _ => None,
        };
        Located {
            placement: match placement {
                // Every side is read in full here, so nothing is left pending.
                Placement::Pending { file } => Placement::Outdated { file, trail: None },
                other => other,
            },
            file,
            current,
            trail,
        }
    }

    /// Lines `start..=end` of `side` of the listed `entry` now: where they are, their text and
    /// the excerpt of the diff around them (PLAN §10.3), all from the one diff read, so the
    /// excerpt never disagrees with the anchor. `None` when the file's diff isn't read or isn't
    /// text.
    fn current_code(
        &self,
        entry: &FileEntry,
        side: AnchorSide,
        start: u32,
        end: u32,
    ) -> Option<CurrentCode> {
        let text = self.diffs.get(&entry.path).and_then(FileDiff::as_text)?;
        let rows = excerpt(text, side, start, end)?;
        let code = side_lines(text, side)
            .get((start as usize).saturating_sub(1)..end as usize)
            .map(|lines| lines.join("\n"))
            .unwrap_or_default();
        let place = AnchorPlace::for_entry(entry, side, self.round);
        Some(CurrentCode {
            anchor: CurrentAnchorDto {
                path: place.path,
                side: side_str(side),
                start_line: start,
                end_line: end,
            },
            code,
            excerpt: excerpt_lines(text, &rows).join("\n"),
        })
    }

    /// The anchor a write on `thread` records (PLAN §8): its anchor re-taken where relocation
    /// found it, in the round in progress with fresh context, when `located` says it moved (or
    /// stands in a file renamed since, its round's history included); `None` otherwise, an
    /// outdated thread with a trail included.
    pub fn relocated(&self, thread: &Thread, located: &Located) -> Option<Anchor> {
        let entry = self.entry(located.file.as_deref()?)?;
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

/// The current diff of `review` with what `threads` need read: their files, and the round
/// histories those that the current text leaves outdated need.
fn read(
    repo: &Repo,
    review: &Review,
    threads: &[&Thread],
    ctx: &mut Context,
) -> Result<Sides, CliError> {
    let mut sides = Sides::list(repo, review, ctx)?;
    sides.read_for(threads)?;
    sides.read_history_for(review, threads, ctx)?;
    Ok(sides)
}

/// Where each of `threads` (threads of `review`) stands now, in the same order. Blocking: one
/// file list and one diff per file a thread is anchored in, plus the round histories outdated
/// threads need.
pub(crate) fn locate(
    repo: &Repo,
    review: &Review,
    threads: &[&Thread],
    ctx: &mut Context,
) -> Result<Vec<Located>, CliError> {
    let sides = read(repo, review, threads, ctx)?;
    Ok(threads.iter().map(|thread| sides.locate(thread)).collect())
}

/// Where `thread` (a thread of `review`) stands now, and the anchor a write on it records when
/// it moved ([`Sides::relocated`]).
pub(crate) fn locate_for_write(
    repo: &Repo,
    review: &Review,
    thread: &Thread,
    ctx: &mut Context,
) -> Result<(Located, Option<Anchor>), CliError> {
    let sides = read(repo, review, &[thread], ctx)?;
    let located = sides.locate(thread);
    let relocated = sides.relocated(thread, &located);
    Ok((located, relocated))
}
