//! Anchors from what the reviewer selected (docs/PLAN.md §2.5, §8): the anchored lines' text and
//! up to [`ANCHOR_CONTEXT`] lines either side, taken from the side's full text, so a thread can be
//! relocated when the file moves on and shown with its original code when it can't.

use super::model::{Anchor, AnchorSide};
use crate::diff::{FileEntry, TextDiff};

/// Lines of context an anchor keeps before and after its range (PLAN §2.5).
pub const ANCHOR_CONTEXT: usize = 3;

/// Why a selection can't be anchored.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AnchorError {
    #[error("lines are 1-based")]
    ZeroLine,
    #[error("line {line} is past the end of the file's side ({len} lines)")]
    PastEnd {
        line: u32,
        len: usize,
        side: AnchorSide,
    },
}

/// Where an anchor points, apart from its lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorPlace {
    /// The round the line numbers refer to.
    pub round: u32,
    /// The file's path on the anchored side.
    pub path: String,
    /// The file's old path, for a renamed file.
    pub old_path: Option<String>,
    pub side: AnchorSide,
}

impl AnchorPlace {
    /// `side` of the listed file `entry` in `round`: the new side by the file's path, the old
    /// side by its old path when it was renamed.
    pub fn for_entry(entry: &FileEntry, side: AnchorSide, round: u32) -> Self {
        let path = match side {
            AnchorSide::New => entry.path.clone(),
            AnchorSide::Old => entry.old_path.clone().unwrap_or_else(|| entry.path.clone()),
        };
        Self {
            round,
            path,
            old_path: entry.old_path.clone(),
            side,
        }
    }

    /// The file-level anchor here: a comment on the whole file ([`Anchor::file_level`]).
    pub fn file_level(self) -> Anchor {
        Anchor::file_level(self.round, self.path, self.old_path, self.side)
    }
}

/// The anchor for lines `from` to `to` (1-based, inclusive, in either order: a selection
/// extended upwards ends above where it started) of `place`'s side, whose full text is `lines`
/// (`lines[0]` is line 1, [`side_lines`]).
pub fn anchor_selection<S: AsRef<str>>(
    place: AnchorPlace,
    lines: &[S],
    from: u32,
    to: u32,
) -> Result<Anchor, AnchorError> {
    let (start, end) = (from.min(to), from.max(to));
    if start == 0 {
        return Err(AnchorError::ZeroLine);
    }
    if end as usize > lines.len() {
        return Err(AnchorError::PastEnd {
            line: end,
            len: lines.len(),
            side: place.side,
        });
    }
    let text = |range: std::ops::Range<usize>| -> Vec<String> {
        lines[range]
            .iter()
            .map(|line| line.as_ref().to_string())
            .collect()
    };
    // 0-based: the range is `first..past`.
    let (first, past) = (start as usize - 1, end as usize);
    Ok(Anchor {
        round: place.round,
        path: place.path,
        old_path: place.old_path,
        side: place.side,
        start_line: start,
        end_line: end,
        text: text(first..past),
        context_before: text(first.saturating_sub(ANCHOR_CONTEXT)..first),
        context_after: text(past..(past + ANCHOR_CONTEXT).min(lines.len())),
    })
}

/// One side's full text from a text diff, which holds both sides complete (PLAN D4): the lines
/// that have a number on that side, in order, so index `i` is line `i + 1`.
pub fn side_lines(diff: &TextDiff, side: AnchorSide) -> Vec<&str> {
    diff.lines
        .iter()
        .filter(|line| match side {
            AnchorSide::Old => line.old_no.is_some(),
            AnchorSide::New => line.new_no.is_some(),
        })
        .map(|line| line.text.as_str())
        .collect()
}

impl Anchor {
    /// Whether the anchored text still stands at the recorded line numbers in `lines`, the
    /// side's current full text ([`side_lines`]). When it doesn't, relocation looks for it
    /// (PLAN §8, `super::relocate`).
    pub fn matches<S: AsRef<str>>(&self, lines: &[S]) -> bool {
        let Some(first) = (self.start_line as usize).checked_sub(1) else {
            return false;
        };
        let past = self.end_line as usize;
        past >= first
            && lines.get(first..past).is_some_and(|current| {
                current.len() == self.text.len()
                    && current
                        .iter()
                        .zip(&self.text)
                        .all(|(line, text)| line.as_ref() == text)
            })
    }
}
