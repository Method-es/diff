//! The text output's lines (docs/PLAN.md §10.3): short, stable, no colour. Fields are
//! separated by two spaces, so a line splits cleanly on runs of whitespace except within a
//! label or a comment's summary, which come last or between fixed words.

use crate::dto::{ReviewDto, ThreadCountsDto, ThreadDto};

/// `r-…  working tree vs main  changes_requested  round 2  1 open, 0 addressed, 2 resolved`.
pub(crate) fn review_line(review: &ReviewDto) -> String {
    format!(
        "{}  {}  {}  round {}  {}",
        review.id,
        review.target.label,
        review.status,
        review.round,
        counts(&review.threads)
    )
}

/// `1 open, 0 addressed, 2 resolved`.
pub(crate) fn counts(counts: &ThreadCountsDto) -> String {
    format!(
        "{} open, {} addressed, {} resolved",
        counts.open, counts.addressed, counts.resolved
    )
}

/// A thread's place: `src/a.ts:12`, `src/a.ts:12-14`, `src/a.ts:14-16 (moved from 12-14)`, or
/// `src/a.ts (outdated, was 12-14)`. A thread on the old side says so first, `src/a.ts:12
/// (old)`, `src/a.ts:14 (old, moved from 12)`, so its lines aren't taken for the file's lines
/// as they are now; the GUI names it the same way (DESIGN §4.1).
pub(crate) fn location(thread: &ThreadDto) -> String {
    let original = &thread.anchor.original;
    let was = lines(original.start_line, original.end_line);
    let mut notes: Vec<String> = Vec::new();
    if thread.side == "old" {
        notes.push("old".to_string());
    }
    let place = match &thread.anchor.current {
        Some(current) => {
            if (current.start_line, current.end_line) != (original.start_line, original.end_line) {
                notes.push(format!("moved from {was}"));
            }
            format!(
                "{}:{}",
                current.path,
                lines(current.start_line, current.end_line)
            )
        }
        None => {
            notes.push(format!("outdated, was {was}"));
            thread.path.clone()
        }
    };
    if notes.is_empty() {
        place
    } else {
        format!("{place} ({})", notes.join(", "))
    }
}

/// `12` or `12-14`.
pub(crate) fn lines(start: u32, end: u32) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    }
}

/// The first line of `text` that has anything on it, trimmed and cut at 100 characters.
pub(crate) fn first_line(text: &str) -> String {
    const MAX: usize = 100;
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if line.chars().count() > MAX {
        let cut: String = line.chars().take(MAX - 1).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

/// `text`'s lines, each indented by `indent`.
pub(crate) fn indented(text: &str, indent: &str) -> Vec<String> {
    text.lines().map(|line| format!("{indent}{line}")).collect()
}
