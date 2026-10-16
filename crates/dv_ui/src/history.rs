//! The history of positions (PLAN §18.1, DESIGN §5 `Alt ←` / `Alt →`): one list shared by the
//! stream and the file view, so a jump and back returns to the line it left.
//!
//! A position is where the reader was: the stream or the file view, a file, its side and a line
//! of it. Only jumps push one ([`History::jump`]): opening the file view (the header menu's View
//! file, `F`, `Ctrl P`) and leaving it (`Esc`, its close button, a panel row that shows the
//! stream); moving within a view (the keys, find's `Enter`, `J`/`K`, scrolling) doesn't. A jump
//! pushes the position it leaves and drops the positions ahead of it, as a browser's history
//! does. [`History::back`] and [`History::forward`] step through it, each trading the position
//! they leave for the one they return to. It keeps the last [`HISTORY_CAP`] positions.

use std::collections::VecDeque;

use dv_core::review::AnchorSide;

/// How many positions the history keeps behind (and ahead of) the current one.
pub const HISTORY_CAP: usize = 100;

/// Which view a position is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Stream,
    FileView,
}

/// Where the reader was: a view, a file, its side and a line (its number on that side).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub view: View,
    pub path: String,
    pub side: AnchorSide,
    pub line: u32,
    /// A file view reading the working tree as it stands (go to definition in a review whose
    /// new side isn't it, PLAN §18.1).
    pub worktree: bool,
}

/// The positions behind and ahead of the current one (module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    back: VecDeque<Position>,
    forward: Vec<Position>,
}

impl History {
    /// A jump away from `from`: it goes behind, and the positions ahead go.
    pub fn jump(&mut self, from: Position) {
        if self.back.back() != Some(&from) {
            self.back.push_back(from);
        }
        while self.back.len() > HISTORY_CAP {
            self.back.pop_front();
        }
        self.forward.clear();
    }

    /// `Alt ←`: the position before, `current` going ahead of it; `None` with nothing behind.
    pub fn back(&mut self, current: Option<Position>) -> Option<Position> {
        let to = self.back.pop_back()?;
        if let Some(current) = current {
            self.forward.push(current);
            if self.forward.len() > HISTORY_CAP {
                self.forward.remove(0);
            }
        }
        Some(to)
    }

    /// `Alt →`: the position a step back left, `current` going behind it; `None` with nothing
    /// ahead.
    pub fn forward(&mut self, current: Option<Position>) -> Option<Position> {
        let to = self.forward.pop()?;
        if let Some(current) = current {
            self.back.push_back(current);
            while self.back.len() > HISTORY_CAP {
                self.back.pop_front();
            }
        }
        Some(to)
    }

    /// The position [`History::back`] would return to, without stepping.
    pub fn peek_back(&self) -> Option<&Position> {
        self.back.back()
    }

    /// The position [`History::forward`] would return to, without stepping.
    pub fn peek_forward(&self) -> Option<&Position> {
        self.forward.last()
    }

    /// How many positions are behind and ahead (`state.history`).
    pub fn counts(&self) -> (usize, usize) {
        (self.back.len(), self.forward.len())
    }

    /// Forgets every position (another review shows).
    pub fn clear(&mut self) {
        self.back.clear();
        self.forward.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(view: View, path: &str, line: u32) -> Position {
        Position {
            view,
            path: path.to_string(),
            side: AnchorSide::New,
            line,
            worktree: false,
        }
    }

    #[test]
    fn back_and_forward_trade_positions() {
        let mut history = History::default();
        assert_eq!(history.back(Some(at(View::Stream, "a", 1))), None);
        // The stream at a:10, then the file view of b at 3.
        history.jump(at(View::Stream, "a", 10));
        assert_eq!(history.counts(), (1, 0));
        let back = history.back(Some(at(View::FileView, "b", 3)));
        assert_eq!(back, Some(at(View::Stream, "a", 10)));
        assert_eq!(history.counts(), (0, 1));
        let forward = history.forward(Some(at(View::Stream, "a", 12)));
        assert_eq!(forward, Some(at(View::FileView, "b", 3)));
        assert_eq!(history.counts(), (1, 0));
        // Back again returns to where forward left.
        assert_eq!(
            history.back(Some(at(View::FileView, "b", 3))),
            Some(at(View::Stream, "a", 12))
        );
    }

    #[test]
    fn a_jump_drops_the_positions_ahead_and_the_oldest_beyond_the_cap() {
        let mut history = History::default();
        history.jump(at(View::Stream, "a", 1));
        history.jump(at(View::FileView, "b", 2));
        history.back(Some(at(View::Stream, "c", 3)));
        assert_eq!(history.counts(), (1, 1));
        history.jump(at(View::Stream, "d", 4));
        assert_eq!(history.counts(), (2, 0));
        // The same position twice in a row is kept once.
        history.jump(at(View::Stream, "d", 4));
        assert_eq!(history.counts(), (2, 0));
        for line in 0..(HISTORY_CAP as u32 + 10) {
            history.jump(at(View::Stream, "e", line));
        }
        assert_eq!(history.counts(), (HISTORY_CAP, 0));
        history.clear();
        assert_eq!(history.counts(), (0, 0));
    }
}
