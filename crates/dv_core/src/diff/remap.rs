//! Carrying a reader's place across a file's reload (PLAN §11 step 7): the agent saves a file
//! under review and its diff loads again with other lines, so whatever named a line of the
//! earlier diff by its index (the cursor, the selection, the reading position, the context the
//! reviewer opened) is found again in the later diff.
//!
//! Both diffs describe the same file, so on a side whose text didn't change (the base, while
//! the agent edits the working tree; the working tree, when the agent commits part of it) every
//! line keeps its number: a line with a number there maps by that number. A line only on the
//! other side (an added line, while the base holds still) maps by its place after the last line
//! that has one, within the run of such lines that follows that line in the later diff, or onto
//! the run's last line when the run is shorter (onto that line itself when there is no run).
//! When both sides changed, a line maps by its own number: its new one, else its old one.

use super::DiffLine;

/// A line's number on one side.
type Number = fn(&DiffLine) -> Option<u32>;

fn old_number(line: &DiffLine) -> Option<u32> {
    line.old_no
}

fn new_number(line: &DiffLine) -> Option<u32> {
    line.new_no
}

/// For every line of `from`, the index of the line it stands for in `to`, both diffs of one
/// file (module docs). Empty when `to` has no lines.
pub fn remap_lines(from: &[DiffLine], to: &[DiffLine]) -> Vec<usize> {
    if to.is_empty() {
        return Vec::new();
    }
    if same_side(from, to, old_number) {
        by_kept_side(from, to, old_number)
    } else if same_side(from, to, new_number) {
        by_kept_side(from, to, new_number)
    } else {
        by_own_number(from, to)
    }
}

/// Whether the side `number` reads has the same text in both diffs.
fn same_side(from: &[DiffLine], to: &[DiffLine], number: Number) -> bool {
    let on_side = move |line: &&DiffLine| number(line).is_some();
    from.iter()
        .filter(on_side)
        .map(|line| &line.text)
        .eq(to.iter().filter(on_side).map(|line| &line.text))
}

/// `lines`' indexes by their number on the side `number` reads (`index[n]` for number `n`).
fn indexes_by_number(lines: &[DiffLine], number: Number) -> Vec<Option<usize>> {
    let mut index = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if let Some(n) = number(line).and_then(|n| usize::try_from(n).ok()) {
            if index.len() <= n {
                index.resize(n + 1, None);
            }
            index[n] = Some(i);
        }
    }
    index
}

/// The mapping when the side `number` reads is the same in both diffs (module docs).
fn by_kept_side(from: &[DiffLine], to: &[DiffLine], number: Number) -> Vec<usize> {
    let index = indexes_by_number(to, number);
    let found = |line: &DiffLine| {
        number(line)
            .and_then(|n| usize::try_from(n).ok())
            .and_then(|n| index.get(n).copied().flatten())
    };
    let mut out = Vec::with_capacity(from.len());
    // The last line of `from` with a number on the kept side, and where it went.
    let mut last: Option<(usize, usize)> = None;
    for (i, line) in from.iter().enumerate() {
        if let Some(j) = found(line) {
            last = Some((i, j));
            out.push(j);
            continue;
        }
        // Only on the other side: the same place in the run after the last kept line.
        let (place, start) = match last {
            Some((p, q)) => (i - p, q + 1),
            None => (i + 1, 0),
        };
        let run = to[start..]
            .iter()
            .take_while(|line| number(line).is_none())
            .count();
        out.push(if run > 0 {
            start + place.min(run) - 1
        } else {
            start.saturating_sub(1)
        });
    }
    out
}

/// The mapping when both sides changed: a line goes to the line with its own number (its new
/// one, else its old one), or the last line on that side when the later diff is shorter there.
fn by_own_number(from: &[DiffLine], to: &[DiffLine]) -> Vec<usize> {
    let old = indexes_by_number(to, old_number);
    let new = indexes_by_number(to, new_number);
    let last = |index: &[Option<usize>]| index.iter().rev().find_map(|&i| i);
    from.iter()
        .map(|line| {
            let (index, number) = match (line.new_no, line.old_no) {
                (Some(n), _) => (&new, n),
                (None, Some(n)) => (&old, n),
                (None, None) => (&new, 0),
            };
            usize::try_from(number)
                .ok()
                .and_then(|n| index.get(n).copied().flatten())
                .or_else(|| last(index))
                .unwrap_or(to.len() - 1)
        })
        .collect()
}

#[cfg(test)]
mod tests;
