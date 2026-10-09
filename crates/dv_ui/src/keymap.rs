//! The keymap (DESIGN §5). One table, [`SHORTCUTS`], drives both the gpui bindings and the
//! keycaps views draw ([`label`]), so the two can't drift.
//!
//! gpui gives every matching binding a turn, deepest key context first, and stops at the first
//! handler that consumes the key. gpui-component's text fields (key context `Input`) matter
//! here: a single-line field takes `↑`/`↓` without using them, so list keys driven from a filter
//! field are bound ahead of it ([`Bind::AheadOfInputs`]); it lets `Enter` and an idle `Esc`
//! through, so those rows need nothing special; and single-letter keys must stay out of text
//! fields ([`Bind::OutsideInputs`], DESIGN §5: "single-letter keys are inactive while a text
//! field has focus"). A multi-line field (the composer, the sheet's summary) uses `Enter` and
//! `Ctrl Enter` itself, inserting a newline, so `Ctrl Enter` rows there sit ahead of it too.
//!
//! `secondary` is gpui's platform modifier: `Ctrl` on Windows and Linux, `Cmd` on macOS
//! (DESIGN §5: "macOS uses Cmd for every Ctrl").

use gpui::{
    Action, App, DummyKeyboardMapper, KeyBinding, KeyBindingContextPredicate, SharedString,
};

use crate::actions::{composer, diff_pane, file_list, sheet, shell, source_picker, switcher};

/// Key context of the shell's root element: everything in the main window.
pub const SHELL: &str = "Shell";
/// Key context of the main area, where the diff stream is (inside [`SHELL`]). It has the
/// keyboard whenever no overlay or text field does.
pub const DIFF_PANE: &str = "DiffPane";
/// Key context of the open switcher (inside [`SHELL`]).
pub const SWITCHER: &str = "Switcher";
/// Key context of the open source picker (inside [`SHELL`]).
pub const SOURCE_PICKER: &str = "SourcePicker";
/// Key context of the panel's file list (inside [`SHELL`]).
pub const FILE_LIST: &str = "FileList";
/// Key context of the composer, a new thread's or a reply's (inside [`DIFF_PANE`]).
pub const COMPOSER: &str = "Composer";
/// Key context of the open "Finish round" sheet (inside [`SHELL`]).
pub const SHEET: &str = "Sheet";
/// Key context gpui-component sets on its text fields; gpui-base keeps its constant private.
pub const INPUT: &str = "Input";

/// How a [`Shortcut`] reaches gpui.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bind {
    /// Bound in the row's context, text fields included: it fires from a focused field whenever
    /// the field doesn't consume the key itself.
    InContext,
    /// Bound in the row's context except while a text field has focus (`Context && !Input`).
    OutsideInputs,
    /// Bound on the text fields inside the row's context (`Context > Input`), ahead of the
    /// field's own binding for the key: for keys a field takes without using them.
    AheadOfInputs,
}

/// One row of the keymap.
#[derive(Clone, Copy)]
pub struct Shortcut {
    /// gpui keystroke syntax, e.g. `secondary-o`.
    pub keys: &'static str,
    /// The key context the row belongs to (one of the constants above).
    pub context: &'static str,
    pub action: fn() -> Box<dyn Action>,
    pub bind: Bind,
}

impl std::fmt::Debug for Shortcut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shortcut")
            .field("keys", &self.keys)
            .field("context", &self.context)
            .field("action", &(self.action)().name())
            .field("bind", &self.bind)
            .finish()
    }
}

impl Shortcut {
    /// The context predicate the row is bound under.
    pub fn predicate(&self) -> String {
        match self.bind {
            Bind::InContext => self.context.to_string(),
            Bind::OutsideInputs => format!("{} && !{INPUT}", self.context),
            Bind::AheadOfInputs => format!("{} > {INPUT}", self.context),
        }
    }

    /// The gpui binding for this row.
    pub fn key_binding(&self) -> anyhow::Result<KeyBinding> {
        let predicate = KeyBindingContextPredicate::parse(&self.predicate())?;
        Ok(KeyBinding::load(
            self.keys,
            (self.action)(),
            Some(predicate.into()),
            false,
            None,
            &DummyKeyboardMapper,
        )?)
    }

    /// The keycap text, e.g. `Ctrl O`.
    pub fn label(&self) -> SharedString {
        label(self.keys)
    }
}

macro_rules! shortcut {
    ($keys:literal, $context:expr, $action:expr) => {
        shortcut!($keys, $context, $action, InContext)
    };
    ($keys:literal, $context:expr, $action:expr, $bind:ident) => {
        Shortcut {
            keys: $keys,
            context: $context,
            action: || Box::new($action),
            bind: Bind::$bind,
        }
    };
}

/// Every binding dv defines.
pub const SHORTCUTS: &[Shortcut] = &[
    // The shell. Text fields bind neither, so both work from a focused field too.
    shortcut!("secondary-o", SHELL, shell::ToggleSwitcher),
    shortcut!("secondary-b", SHELL, shell::TogglePanel),
    // Finishing the round stays out of text fields: in the composer `Ctrl Enter` posts, in the
    // sheet it submits (both bound in their own contexts, below).
    shortcut!("secondary-enter", SHELL, shell::FinishReview, OutsideInputs),
    // The switcher: its filter field has the keyboard and takes ↑/↓ itself, so those rows sit
    // ahead of it; Enter and an idle Esc pass through the field to the plain rows.
    shortcut!("down", SWITCHER, switcher::SelectNext, AheadOfInputs),
    shortcut!("up", SWITCHER, switcher::SelectPrevious, AheadOfInputs),
    shortcut!("enter", SWITCHER, switcher::Confirm),
    shortcut!("escape", SWITCHER, switcher::Dismiss),
    // The source picker has the keyboard itself (no text field).
    shortcut!("down", SOURCE_PICKER, source_picker::SelectNext),
    shortcut!("up", SOURCE_PICKER, source_picker::SelectPrevious),
    shortcut!("enter", SOURCE_PICKER, source_picker::Confirm),
    shortcut!("escape", SOURCE_PICKER, source_picker::Dismiss),
    // The file list's filter field lets Enter and an idle Esc through.
    shortcut!("enter", FILE_LIST, file_list::RevealFirst),
    shortcut!("escape", FILE_LIST, file_list::ClearFilter),
    // The diff stream (DESIGN §5). No text field lives in the main area, but its keys stay out
    // of fields anyway, so a composer there never loses its letters to them.
    shortcut!("j", DIFF_PANE, diff_pane::NextFile, OutsideInputs),
    shortcut!("k", DIFF_PANE, diff_pane::PreviousFile, OutsideInputs),
    shortcut!("n", DIFF_PANE, diff_pane::NextChange, OutsideInputs),
    shortcut!("p", DIFF_PANE, diff_pane::PreviousChange, OutsideInputs),
    shortcut!("v", DIFF_PANE, diff_pane::ToggleViewed, OutsideInputs),
    shortcut!("e", DIFF_PANE, diff_pane::ExpandGap, OutsideInputs),
    shortcut!(
        "shift-e",
        DIFF_PANE,
        diff_pane::ExpandGapFully,
        OutsideInputs
    ),
    shortcut!("s", DIFF_PANE, diff_pane::ToggleSplit, OutsideInputs),
    shortcut!("up", DIFF_PANE, diff_pane::CursorUp, OutsideInputs),
    shortcut!("down", DIFF_PANE, diff_pane::CursorDown, OutsideInputs),
    shortcut!("shift-up", DIFF_PANE, diff_pane::SelectUp, OutsideInputs),
    shortcut!(
        "shift-down",
        DIFF_PANE,
        diff_pane::SelectDown,
        OutsideInputs
    ),
    shortcut!("left", DIFF_PANE, diff_pane::CursorLeft, OutsideInputs),
    shortcut!("right", DIFF_PANE, diff_pane::CursorRight, OutsideInputs),
    shortcut!("pageup", DIFF_PANE, diff_pane::PageUp, OutsideInputs),
    shortcut!("pagedown", DIFF_PANE, diff_pane::PageDown, OutsideInputs),
    shortcut!(
        "escape",
        DIFF_PANE,
        diff_pane::ClearSelection,
        OutsideInputs
    ),
    shortcut!("c", DIFF_PANE, diff_pane::Comment, OutsideInputs),
    shortcut!("r", DIFF_PANE, diff_pane::Reply, OutsideInputs),
    shortcut!("]", DIFF_PANE, diff_pane::NextThread, OutsideInputs),
    shortcut!("[", DIFF_PANE, diff_pane::PreviousThread, OutsideInputs),
    shortcut!("x", DIFF_PANE, diff_pane::ResolveThread, OutsideInputs),
    shortcut!("i", DIFF_PANE, diff_pane::ToggleComparison, OutsideInputs),
    // The composer's text field inserts a newline on `Ctrl Enter` itself (it is multi-line),
    // so posting sits ahead of it; it lets an idle `Esc` through to the plain row.
    shortcut!("secondary-enter", COMPOSER, composer::Post, AheadOfInputs),
    shortcut!("escape", COMPOSER, composer::Cancel),
    // The sheet: its verdicts have the keyboard until `Tab` moves it into the summary field,
    // where `Ctrl Enter` and `Tab` sit ahead of the field's own newline and indent.
    shortcut!("secondary-enter", SHEET, sheet::Submit),
    shortcut!("secondary-enter", SHEET, sheet::Submit, AheadOfInputs),
    shortcut!("escape", SHEET, sheet::Dismiss),
    shortcut!("down", SHEET, sheet::SelectNext, OutsideInputs),
    shortcut!("up", SHEET, sheet::SelectPrevious, OutsideInputs),
    shortcut!("1", SHEET, sheet::PickApprove, OutsideInputs),
    shortcut!("2", SHEET, sheet::PickRequestChanges, OutsideInputs),
    shortcut!("tab", SHEET, sheet::SwitchFocus, OutsideInputs),
    shortcut!("tab", SHEET, sheet::SwitchFocus, AheadOfInputs),
    shortcut!("shift-tab", SHEET, sheet::SwitchFocus, OutsideInputs),
    shortcut!("shift-tab", SHEET, sheet::SwitchFocus, AheadOfInputs),
];

/// Binds [`SHORTCUTS`]. Call after `gpui_component::init`, so dv's rows are registered last
/// and win within a context depth.
pub fn init(cx: &mut App) -> anyhow::Result<()> {
    let bindings = SHORTCUTS
        .iter()
        .map(Shortcut::key_binding)
        .collect::<anyhow::Result<Vec<_>>>()?;
    cx.bind_keys(bindings);
    Ok(())
}

/// The first row for `action` in `context`.
pub fn shortcut_for(action: &dyn Action, context: &str) -> Option<&'static Shortcut> {
    SHORTCUTS
        .iter()
        .find(|shortcut| shortcut.context == context && (shortcut.action)().partial_eq(action))
}

/// Keycap text for gpui keystroke syntax (DESIGN §5 writes `Ctrl O`, `Ctrl Enter`): `secondary`
/// is `Ctrl`, or `Cmd` on macOS; arrows are drawn as arrows; a sequence joins with spaces.
pub fn label(keys: &str) -> SharedString {
    keys.split_whitespace()
        .map(keystroke_label)
        .collect::<Vec<_>>()
        .join(" ")
        .into()
}

fn keystroke_label(stroke: &str) -> String {
    let (modifiers, key) = match stroke.rsplit_once('-') {
        Some((modifiers, key)) if !key.is_empty() => (modifiers, key),
        _ => ("", stroke),
    };
    let mut parts: Vec<String> = modifiers
        .split('-')
        .filter(|modifier| !modifier.is_empty())
        .map(|modifier| match modifier {
            "secondary" if cfg!(target_os = "macos") => "Cmd".to_string(),
            "secondary" | "ctrl" => "Ctrl".to_string(),
            "cmd" | "platform" => "Cmd".to_string(),
            "alt" => "Alt".to_string(),
            "shift" => "Shift".to_string(),
            other => capitalize(other),
        })
        .collect();
    parts.push(match key {
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "escape" => "Esc".to_string(),
        "pageup" => "PgUp".to_string(),
        "pagedown" => "PgDn".to_string(),
        other if other.chars().count() > 1 => capitalize(other),
        other => other.to_uppercase(),
    });
    parts.join(" ")
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_row_binds_and_no_keystroke_is_bound_twice_under_one_predicate() {
        let mut seen = HashSet::new();
        for shortcut in SHORTCUTS {
            shortcut
                .key_binding()
                .unwrap_or_else(|error| panic!("{shortcut:?}: {error}"));
            assert!(
                seen.insert((shortcut.keys, shortcut.predicate())),
                "{shortcut:?} is bound twice"
            );
        }
    }

    #[test]
    fn labels_read_as_design_writes_them() {
        let ctrl = if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Ctrl"
        };
        assert_eq!(label("secondary-o"), format!("{ctrl} O"));
        assert_eq!(label("secondary-enter"), format!("{ctrl} Enter"));
        assert_eq!(label("shift-down"), "Shift ↓");
        assert_eq!(label("escape"), "Esc");
        assert_eq!(label("v j"), "V J");
        assert_eq!(
            shortcut_for(&shell::ToggleSwitcher, SHELL).map(Shortcut::label),
            Some(format!("{ctrl} O").into())
        );
    }

    #[test]
    fn ctrl_enter_posts_ahead_of_the_composer_field_and_finishes_only_outside_fields() {
        let post = shortcut_for(&composer::Post, COMPOSER).unwrap();
        assert_eq!(post.predicate(), "Composer > Input");
        let finish = shortcut_for(&shell::FinishReview, SHELL).unwrap();
        assert_eq!(finish.predicate(), "Shell && !Input");
        let submits: Vec<String> = SHORTCUTS
            .iter()
            .filter(|shortcut| (shortcut.action)().partial_eq(&sheet::Submit))
            .map(Shortcut::predicate)
            .collect();
        assert_eq!(submits, ["Sheet", "Sheet > Input"]);
    }

    #[test]
    fn list_keys_sit_ahead_of_the_filter_field() {
        let down = shortcut_for(&switcher::SelectNext, SWITCHER).unwrap();
        assert_eq!(down.predicate(), "Switcher > Input");
        let enter = shortcut_for(&switcher::Confirm, SWITCHER).unwrap();
        assert_eq!(enter.predicate(), "Switcher");
    }
}
