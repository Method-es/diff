//! Threads in the stream (DESIGN §4.1 "Thread card", PLAN §2.5): the inline cards, the composer
//! that opens a thread or replies to one, and the words both use.
//!
//! A card shows a thread's state and anchor, its opening comment and replies (Markdown through
//! [`crate::markdown`], sanitised), the reviewer's actions (Resolve, Reopen) and a reply field.
//! `open` threads wear the accent ring dot, `addressed` ones the agent's blue note with Resolve
//! and Reopen, `resolved` ones a green check and collapse to one line until opened, and an
//! outdated one (its anchored text no longer stands at its lines, PLAN §8) a yellow chip and its
//! original lines. On a finished review the actions and the reply field stay, disabled.
//!
//! The composer is one multi-line field: on a new thread it sits under the selected lines with
//! "Comment on new · lines 12–14"; as a reply it takes the place of the card's reply field.
//! `Ctrl Enter` posts, `Esc` cancels (`crate::actions::composer`).
//!
//! The views here hold no state of their own: the stream ([`crate::diff_pane`]) keeps which
//! thread is focused, which resolved ones are open and the composer, and hands each card its
//! handlers, as [`crate::banner::Banner`] does.
//!
//! Automation targets: `thread/<id>` with `/resolve`, `/reopen`, `/toggle` and `/reply`;
//! `composer` with `/input`, `/cancel` and `/post`.

mod render;

#[cfg(test)]
mod tests;

use std::fmt::Write as _;

use dv_core::review::{AnchorSide, Author, AuthorKind, Thread, ThreadId, ThreadState};
use gpui::{Entity, Subscription};
use gpui_component::input::TextareaState;

use crate::failure::Failure;
pub use render::{ComposerBox, ThreadCard};

/// Rows the composer's field starts with and grows to.
pub const COMPOSER_ROWS: (usize, usize) = (3, 12);
/// Rows a reply's field starts with and grows to.
pub const REPLY_ROWS: (usize, usize) = (2, 10);

/// What the composer writes: a new thread on lines of one side of one file, or a reply to
/// `reply_to` (whose anchor the other fields repeat).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerTarget {
    pub path: String,
    pub side: AnchorSide,
    pub start_line: u32,
    pub end_line: u32,
    pub reply_to: Option<ThreadId>,
}

impl ComposerTarget {
    /// The composer's heading: "Comment on new · lines 12–14", or "Reply".
    pub fn title(&self) -> String {
        match self.reply_to {
            Some(_) => "Reply".to_string(),
            None => format!(
                "Comment on {}",
                anchor_label(self.side, self.start_line, self.end_line)
            ),
        }
    }
}

/// The open composer: where it writes, its field, and how its last post went.
pub struct Composer {
    pub target: ComposerTarget,
    pub input: Entity<TextareaState>,
    /// A post is on its way to the store.
    pub posting: bool,
    /// Why the last post failed.
    pub error: Option<Failure>,
    pub _subscription: Subscription,
}

/// `old` or `new`, as anchors and the composer name a side.
pub fn side_word(side: AnchorSide) -> &'static str {
    match side {
        AnchorSide::Old => "old",
        AnchorSide::New => "new",
    }
}

/// "new · line 31", "old · lines 12–14" (DESIGN §4.1).
pub fn anchor_label(side: AnchorSide, start: u32, end: u32) -> String {
    let side = side_word(side);
    if start == end {
        format!("{side} · line {start}")
    } else {
        format!("{side} · lines {start}–{end}")
    }
}

/// `path:31` or `path:12–14`: where a thread sits, as the comments list and prompts name it.
pub fn location_label(thread: &Thread) -> String {
    let anchor = &thread.anchor;
    if anchor.start_line == anchor.end_line {
        format!("{}:{}", anchor.path, anchor.start_line)
    } else {
        format!("{}:{}–{}", anchor.path, anchor.start_line, anchor.end_line)
    }
}

/// How long ago `at` was, from `now` (both ms since the epoch): "just now", "5m ago", "3h ago",
/// "2d ago", "3w ago".
pub fn relative_time(at: u64, now: u64) -> String {
    let seconds = now.saturating_sub(at) / 1000;
    match seconds {
        0..60 => "just now".to_string(),
        60..3_600 => format!("{}m ago", seconds / 60),
        3_600..86_400 => format!("{}h ago", seconds / 3_600),
        86_400..604_800 => format!("{}d ago", seconds / 86_400),
        _ => format!("{}w ago", seconds / 604_800),
    }
}

/// The letter a round avatar shows: the name's first letter or digit, lowercase as the board
/// draws it; `?` for a name without one.
pub fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map_or_else(|| "?".to_string(), |c| c.to_lowercase().collect())
}

/// The word beside an author's name: `reviewer`, `agent` or `github`.
pub fn role_word(author: &Author) -> &'static str {
    author.kind.as_str()
}

/// What one-line rows show of a Markdown body: its first line as plain text
/// ([`crate::markdown::first_line_plain`]), so `**cap**` reads as `cap` there as it does,
/// bold, on the card.
pub fn first_line(body: &str) -> String {
    crate::markdown::first_line_plain(body)
}

/// A thread's state chip text: "Open", "Addressed by claude-code", "Resolved".
pub fn state_label(thread: &Thread) -> String {
    match thread.state {
        ThreadState::Open => "Open".to_string(),
        ThreadState::Addressed => match &thread.last_change {
            Some(change) if change.author.kind == AuthorKind::Agent => {
                format!("Addressed by {}", change.author.name)
            }
            Some(change) if change.author.kind == AuthorKind::Reviewer => "Addressed".to_string(),
            _ => "Addressed by agent".to_string(),
        },
        ThreadState::Resolved => "Resolved".to_string(),
    }
}

/// The note the last state change left (the agent's "what I did"), if any.
pub fn state_note(thread: &Thread) -> Option<&str> {
    thread
        .last_change
        .as_ref()
        .and_then(|change| change.note.as_deref())
        .filter(|note| !note.trim().is_empty())
}

/// The Markdown "Copy as prompt" puts on the clipboard (DESIGN §4.1 panel): the open threads,
/// each with where it sits, the anchored code, the comment and its replies, so an agent can be
/// handed the review in one paste. `label` names the target ("working tree vs main").
pub fn prompt_markdown(label: &str, threads: &[&Thread]) -> String {
    let mut out = String::new();
    let count = threads.len();
    let _ = writeln!(
        out,
        "Open review comments on {label} ({count} {}):",
        if count == 1 { "comment" } else { "comments" }
    );
    for thread in threads {
        let anchor = &thread.anchor;
        let side = match anchor.side {
            AnchorSide::New => String::new(),
            AnchorSide::Old => " (old side)".to_string(),
        };
        let _ = write!(
            out,
            "\n## {}{side}\n\nThread `{}`.\n\n",
            location_label(thread),
            thread.id
        );
        if !anchor.text.is_empty() {
            let code = anchor.text.join("\n");
            let fence = fence_for(&code);
            let _ = writeln!(
                out,
                "{fence}{}\n{code}\n{fence}\n",
                fence_language(&anchor.path)
            );
        }
        let _ = writeln!(out, "{}", thread.body.trim());
        for reply in thread.replies.iter().filter(|reply| !reply.is_deleted()) {
            let _ = writeln!(
                out,
                "\n> **{}** ({}):",
                reply.author.name,
                role_word(&reply.author)
            );
            for line in reply.body.trim().lines() {
                if line.is_empty() {
                    out.push_str(">\n");
                } else {
                    let _ = writeln!(out, "> {line}");
                }
            }
        }
    }
    out
}

/// A code fence longer than any run of backticks in `code`, so the code can't close it.
fn fence_for(code: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in code.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

/// The fence's info string: the file's extension (`ts`, `rs`), or nothing.
fn fence_language(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, extension))
            if !stem.is_empty() && extension.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            extension
        }
        _ => "",
    }
}
