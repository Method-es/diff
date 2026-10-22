//! The palette's commands (DESIGN §4.1 "Palette"): the shell's and the stream's actions worth
//! finding by name, each run as its key would run it. The stream's run in the main area, which
//! gets the keyboard back first; they are offered only while a review is open.

use gpui::{Action, SharedString};

use crate::actions::{diff_pane, shell};
use crate::keymap::{self, DIFF_PANE, SHELL};

/// A command the palette lists.
#[derive(Clone, Copy)]
pub struct Command {
    /// What its row reads.
    pub title: &'static str,
    /// Other words it is found by, beside its title's.
    pub words: &'static str,
    pub action: fn() -> Box<dyn Action>,
    /// The key context it runs in: [`SHELL`] (anywhere) or [`DIFF_PANE`] (the main area).
    pub context: &'static str,
    /// Whether it needs an open review.
    pub needs_review: bool,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Command")
            .field("title", &self.title)
            .field("action", &(self.action)().name())
            .finish()
    }
}

impl PartialEq for Command {
    fn eq(&self, other: &Self) -> bool {
        self.title == other.title && (self.action)().partial_eq(&*(other.action)())
    }
}

impl Eq for Command {}

impl Command {
    /// The action's name, `diff_pane::NextFile`: the row's key.
    pub fn name(&self) -> &'static str {
        (self.action)().name()
    }

    /// Its keycap, from the keymap row that runs it in its context.
    pub fn keys(&self) -> Option<SharedString> {
        keymap::shortcut_for(&*(self.action)(), self.context).map(keymap::Shortcut::label)
    }

    /// Its title and words, lowercased, for filtering.
    pub fn text(&self) -> String {
        format!("{} {}", self.title, self.words).to_lowercase()
    }
}

macro_rules! command {
    ($title:literal, $words:literal, $action:expr, $context:expr, $needs_review:literal) => {
        Command {
            title: $title,
            words: $words,
            action: || Box::new($action),
            context: $context,
            needs_review: $needs_review,
        }
    };
}

/// Every command, in the order an empty palette lists them.
pub const COMMANDS: &[Command] = &[
    command!("Go to file", "find open", shell::FindFile, SHELL, true),
    command!(
        "Switch repository or review",
        "open switcher",
        shell::ToggleSwitcher,
        SHELL,
        false
    ),
    command!(
        "Open folder…",
        "add repository",
        shell::OpenFolder,
        SHELL,
        false
    ),
    command!(
        "Finish round",
        "approve request changes submit review",
        shell::FinishReview,
        SHELL,
        true
    ),
    command!("Next file", "", diff_pane::NextFile, DIFF_PANE, true),
    command!(
        "Previous file",
        "",
        diff_pane::PreviousFile,
        DIFF_PANE,
        true
    ),
    command!(
        "Next change",
        "hunk",
        diff_pane::NextChange,
        DIFF_PANE,
        true
    ),
    command!(
        "Previous change",
        "hunk",
        diff_pane::PreviousChange,
        DIFF_PANE,
        true
    ),
    command!(
        "Next unresolved thread",
        "comment",
        diff_pane::NextThread,
        DIFF_PANE,
        true
    ),
    command!(
        "Previous unresolved thread",
        "comment",
        diff_pane::PreviousThread,
        DIFF_PANE,
        true
    ),
    command!(
        "Comment on the file",
        "thread",
        diff_pane::CommentOnFile,
        DIFF_PANE,
        true
    ),
    command!(
        "Mark file viewed",
        "unviewed",
        diff_pane::ToggleViewed,
        DIFF_PANE,
        true
    ),
    command!(
        "Split or unified view",
        "toggle diff layout",
        diff_pane::ToggleSplit,
        DIFF_PANE,
        true
    ),
    command!(
        "Since last round or all changes",
        "interdiff comparison toggle",
        diff_pane::ToggleComparison,
        DIFF_PANE,
        true
    ),
    command!(
        "Show or hide annotations",
        "toggle agent",
        diff_pane::ToggleAnnotations,
        DIFF_PANE,
        true
    ),
    command!(
        "Expand the gap",
        "context lines",
        diff_pane::ExpandGap,
        DIFF_PANE,
        true
    ),
    command!(
        "Find in the review",
        "search",
        diff_pane::Find,
        DIFF_PANE,
        true
    ),
    command!(
        "View the whole file",
        "file view open",
        diff_pane::ViewFile,
        DIFF_PANE,
        true
    ),
    command!("Go back", "history", diff_pane::GoBack, DIFF_PANE, true),
    command!(
        "Go forward",
        "history",
        diff_pane::GoForward,
        DIFF_PANE,
        true
    ),
    command!(
        "Go to definition",
        "symbol",
        diff_pane::GoToDefinition,
        DIFF_PANE,
        true
    ),
    command!(
        "Find references",
        "symbol usages",
        diff_pane::FindReferences,
        DIFF_PANE,
        true
    ),
    command!(
        "Show or hide the panel",
        "toggle sidebar files comments",
        shell::TogglePanel,
        SHELL,
        false
    ),
    command!(
        "Settings",
        "appearance theme mode material dark light setup preferences",
        shell::ToggleSettings,
        SHELL,
        false
    ),
];
