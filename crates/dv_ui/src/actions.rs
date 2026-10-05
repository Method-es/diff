//! dv's actions (DESIGN §5), one module per key context; the bindings live in
//! [`crate::keymap`]. Each module is also the action namespace, so names read
//! `shell::ToggleSwitcher` or `diff_pane::NextFile`, and automation's `actions` command lists
//! them all.

/// The shell's own actions, dispatched in the `Shell` key context.
pub mod shell {
    use gpui::actions;

    actions!(
        shell,
        [
            /// Open the switcher (repos and reviews, DESIGN §4.1), or close it (`Ctrl O`).
            ToggleSwitcher,
            /// Show or hide the panel (`Ctrl B`, DESIGN §2).
            TogglePanel,
            /// Open the "Finish round" sheet (`Ctrl Enter` while no text field has the keyboard,
            /// DESIGN §4.2).
            FinishReview,
        ]
    );
}

/// The switcher's list, dispatched in the `Switcher` key context while it is open.
pub mod switcher {
    use gpui::actions;

    actions!(
        switcher,
        [
            /// Select the next row (`↓`), wrapping to the first.
            SelectNext,
            /// Select the previous row (`↑`), wrapping to the last.
            SelectPrevious,
            /// Open what the selected row stands for (`Enter`).
            Confirm,
            /// Close without opening anything (`Esc`).
            Dismiss,
        ]
    );
}

/// The source picker's menu, dispatched in the `SourcePicker` key context while it is open.
pub mod source_picker {
    use gpui::actions;

    actions!(
        source_picker,
        [
            /// Select the next target (`↓`), wrapping to the first.
            SelectNext,
            /// Select the previous target (`↑`), wrapping to the last.
            SelectPrevious,
            /// Open or resume the selected target's review (`Enter`).
            Confirm,
            /// Close without opening anything (`Esc`).
            Dismiss,
        ]
    );
}

/// The diff stream's keys (DESIGN §5), dispatched in the `DiffPane` key context: the main
/// area, which has the keyboard whenever no overlay or text field does.
pub mod diff_pane {
    use gpui::actions;

    actions!(
        diff_pane,
        [
            /// Show the next file: its header at the top, the cursor on its first change (`J`).
            NextFile,
            /// Show the previous file (`K`).
            PreviousFile,
            /// Move the cursor to the next change block (`N`).
            NextChange,
            /// Move the cursor to the previous change block (`P`).
            PreviousChange,
            /// Mark the current file viewed, or not viewed (`V`).
            ToggleViewed,
            /// Reveal one step of the gap nearest the cursor, on the cursor's side (`E`).
            ExpandGap,
            /// Reveal the whole gap nearest the cursor (`Shift E`).
            ExpandGapFully,
            /// Switch between split and unified, remembered per review (`S`).
            ToggleSplit,
            /// Move the cursor up a line (`↑`).
            CursorUp,
            /// Move the cursor down a line (`↓`).
            CursorDown,
            /// Extend the selection up a line (`Shift ↑`).
            SelectUp,
            /// Extend the selection down a line (`Shift ↓`).
            SelectDown,
            /// Put the cursor on the old side, in split view (`←`).
            CursorLeft,
            /// Put the cursor on the new side, in split view (`→`).
            CursorRight,
            /// Scroll up a page, carrying the cursor (`PageUp`).
            PageUp,
            /// Scroll down a page, carrying the cursor (`PageDown`).
            PageDown,
            /// Close the file menu, else cancel the composer, else clear the selection (`Esc`).
            ClearSelection,
            /// Comment on the selected lines, else the cursor's line (`C`).
            Comment,
            /// Reply to the focused thread (`R`).
            Reply,
            /// Focus the next unresolved thread (`]`).
            NextThread,
            /// Focus the previous unresolved thread (`[`).
            PreviousThread,
            /// Resolve the focused thread, or reopen it when it is resolved (`X`).
            ResolveThread,
        ]
    );
}

/// The composer (a new thread or a reply, DESIGN §4.1), dispatched in the `Composer` key
/// context while its text field has the keyboard.
pub mod composer {
    use gpui::actions;

    actions!(
        composer,
        [
            /// Post the comment (`Ctrl Enter`).
            Post,
            /// Discard the comment and give the keyboard back to the stream (`Esc`).
            Cancel,
        ]
    );
}

/// The "Finish round" sheet (DESIGN §4.2), dispatched in the `Sheet` key context while it is
/// open.
pub mod sheet {
    use gpui::actions;

    actions!(
        sheet,
        [
            /// Finish the round with the chosen verdict (`Ctrl Enter`).
            Submit,
            /// Close the sheet without finishing anything (`Esc`).
            Dismiss,
            /// Choose the next verdict (`↓`).
            SelectNext,
            /// Choose the previous verdict (`↑`).
            SelectPrevious,
            /// Choose Approve (`1`).
            PickApprove,
            /// Choose Request changes (`2`).
            PickRequestChanges,
            /// Move the keyboard between the verdicts and the summary (`Tab`).
            SwitchFocus,
        ]
    );
}

/// The file list's filter field, dispatched in the `FileList` key context.
pub mod file_list {
    use gpui::actions;

    actions!(
        file_list,
        [
            /// Show the first file the filter leaves and hand the keyboard back to the stream
            /// (`Enter` in the filter).
            RevealFirst,
            /// Clear the filter and hand the keyboard back to the stream (`Esc` in the filter).
            ClearFilter,
        ]
    );
}
