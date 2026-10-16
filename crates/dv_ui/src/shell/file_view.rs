//! The file view, the file finder and the history in the shell (PLAN §18.1, DESIGN §4.1, §5).
//!
//! **The main area** shows the stream, or the file view over it: a second pane in a one-file
//! scope ([`crate::diff_pane::file_view`]) that the shell opens from the stream's View file and
//! `F`, from `Ctrl P` (the file finder, a menu like the switcher, hung under the command field),
//! and from the history. While the file view shows, the main area renders its pane, with its
//! focus handle and key context, so the stream's keys act on it; the stream keeps its place
//! meanwhile, and showing it again (`Esc` at the end of the file view's chain, its close
//! button, a panel row that shows something in the stream) puts the keyboard back on it.
//!
//! **The history** ([`crate::history`]): opening the file view and leaving it are jumps, each
//! pushing the position left; `Alt ←` / `Alt →` and the mouse's back and forward buttons step
//! through it, restoring the view, the file and the line.

use dv_core::review::AnchorSide;
use gpui::{AppContext as _, Context, Entity, FocusHandle, Focusable as _, Window};

use super::{Overlay, Shell};
use crate::actions::diff_pane::{GoBack, GoForward};
use crate::actions::shell::FindFile;
use crate::diff_pane::{DiffPane, DiffPaneEvent};
use crate::file_finder::{FileFinder, FileFinderEvent};
use crate::history::{History, Position, View};

impl Shell {
    /// The pane the main area shows: the file view's while it is open, else the stream's.
    pub fn main_pane(&self) -> &Entity<DiffPane> {
        if self.file_view_open {
            &self.file_view
        } else {
            &self.diff_pane
        }
    }

    /// The file view's pane (shown or not).
    pub fn file_view(&self) -> &Entity<DiffPane> {
        &self.file_view
    }

    /// Whether the file view shows over the stream.
    pub fn is_file_view_open(&self) -> bool {
        self.file_view_open
    }

    /// The main area's focus handle: the shown pane's, where the keyboard rests whenever no
    /// overlay or text field has it.
    pub(super) fn main_focus(&self, cx: &gpui::App) -> FocusHandle {
        self.main_pane().read(cx).focus_handle(cx)
    }

    /// The history (`state.history`).
    pub fn history(&self) -> &History {
        &self.history
    }

    /// The file finder, while it is open.
    pub fn file_finder(&self) -> Option<&Entity<FileFinder>> {
        self.file_finder.as_ref().map(|overlay| &overlay.view)
    }

    /// Where the reader is now, for the history.
    pub(super) fn position(&self, cx: &gpui::App) -> Option<Position> {
        let (view, (path, side, line)) = if self.file_view_open {
            (
                View::FileView,
                self.file_view.read(cx).file_view_position()?,
            )
        } else {
            (View::Stream, self.diff_pane.read(cx).position()?)
        };
        let worktree = self.file_view_open && self.file_view.read(cx).file_view_worktree();
        Some(Position {
            view,
            path,
            side,
            line,
            worktree,
        })
    }

    /// Shows `path` whole in the file view, at `line` of its side (else its first line); a jump
    /// (`jump`) pushes the position it leaves onto the history.
    pub fn open_file_view(
        &mut self,
        path: &str,
        line: Option<u32>,
        jump: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_file_view_at(path, line, false, jump, window, cx);
    }

    /// [`Shell::open_file_view`], the file read from the working tree as it stands when
    /// `worktree` (`intel`).
    pub fn open_file_view_at(
        &mut self,
        path: &str,
        line: Option<u32>,
        worktree: bool,
        jump: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.is_none() || self.keeps_draft(Some(path), window, cx) {
            return;
        }
        if jump && let Some(from) = self.position(cx) {
            self.history.jump(from);
        }
        self.file_view_open = true;
        self.file_view
            .update(cx, |pane, cx| pane.open_file_at(path, line, worktree, cx));
        self.file_view.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Shows the stream again, where it was; a jump (`jump`) pushes the file view's position
    /// onto the history.
    pub fn close_file_view(&mut self, jump: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.file_view_open || self.keeps_draft(None, window, cx) {
            return;
        }
        if jump && let Some(from) = self.position(cx) {
            self.history.jump(from);
        }
        self.file_view_open = false;
        self.file_view.update(cx, |pane, cx| pane.close_file(cx));
        self.diff_pane.update(cx, |pane, cx| pane.shown_again(cx));
        // The stream may have taken the keyboard already (a reply the comments list opened in
        // a card, which is what asked for the stream): it keeps it.
        if !self.diff_pane.read(cx).owns_focus(window, cx) {
            self.diff_pane.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// Whether the file view's draft keeps it from showing `path` (or, `None`, from closing):
    /// a composer holding text there takes the keyboard instead, and nothing switches (DESIGN
    /// §5: a draft is never dropped but by `Esc`, its explicit discard).
    fn keeps_draft(&self, path: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.file_view_open || !self.file_view.read(cx).draft_blocks(path, cx) {
            return false;
        }
        self.file_view
            .update(cx, |pane, cx| pane.take_up_draft(window, cx));
        true
    }

    /// `Alt ←` (and the mouse's back button): the position before the last jump.
    pub fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(to) = self.history.peek_back().cloned()
            && self.blocked(&to, window, cx)
        {
            return;
        }
        let current = self.position(cx);
        if let Some(to) = self.history.back(current) {
            self.go_to(to, window, cx);
        }
    }

    /// `Alt →` (and the mouse's forward button): the position a step back left.
    pub fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(to) = self.history.peek_forward().cloned()
            && self.blocked(&to, window, cx)
        {
            return;
        }
        let current = self.position(cx);
        if let Some(to) = self.history.forward(current) {
            self.go_to(to, window, cx);
        }
    }

    /// Whether going to `to` would replace or close a file view holding a draft, which then
    /// takes the keyboard instead ([`Shell::keeps_draft`]), the history left as it was.
    fn blocked(&mut self, to: &Position, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match to.view {
            View::FileView => self.keeps_draft(Some(&to.path), window, cx),
            View::Stream => self.keeps_draft(None, window, cx),
        }
    }

    /// Restores `to`: its view, file and line.
    fn go_to(&mut self, to: Position, window: &mut Window, cx: &mut Context<Self>) {
        match to.view {
            View::FileView => {
                self.open_file_view_at(&to.path, Some(to.line), to.worktree, false, window, cx)
            }
            View::Stream => {
                self.close_file_view(false, window, cx);
                let (path, side, line): (String, AnchorSide, u32) = (to.path, to.side, to.line);
                self.diff_pane
                    .update(cx, |pane, cx| pane.show_line(&path, side, line, cx));
            }
        }
    }

    pub(super) fn on_go_back(&mut self, _: &GoBack, window: &mut Window, cx: &mut Context<Self>) {
        self.go_back(window, cx);
    }

    pub(super) fn on_go_forward(
        &mut self,
        _: &GoForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.go_forward(window, cx);
    }

    /// What the stream asks of the shell: the file view opened on a file, or shown no more
    /// because the panel showed something in the stream.
    pub(super) fn on_stream_event(
        &mut self,
        event: &DiffPaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            DiffPaneEvent::ComposerClosed => self.run_pending(window, cx),
            DiffPaneEvent::ViewFile { path, line } => {
                self.open_file_view(path, *line, true, window, cx)
            }
            DiffPaneEvent::Revealing => self.close_file_view(true, window, cx),
            DiffPaneEvent::CurrentFileChanged | DiffPaneEvent::CloseFileView => {}
        }
    }

    /// What the file view asks of the shell: to show the stream again.
    pub(super) fn on_file_view_event(
        &mut self,
        event: &DiffPaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            DiffPaneEvent::CloseFileView => self.close_file_view(true, window, cx),
            DiffPaneEvent::ComposerClosed => self.run_pending(window, cx),
            DiffPaneEvent::ViewFile { path, line } => {
                self.open_file_view(path, *line, true, window, cx)
            }
            DiffPaneEvent::CurrentFileChanged | DiffPaneEvent::Revealing => {}
        }
    }

    /// `Ctrl P`: the file finder, or closed when it is open; nothing with no review open.
    pub(super) fn toggle_file_finder(
        &mut self,
        _: &FindFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_finder.is_some() {
            self.close_menu(window, cx);
        } else {
            self.open_file_finder(window, cx);
        }
    }

    /// Opens the file finder over the open review's new side: in place of an open menu, and
    /// not over the "Finish round" sheet, which the file view would open under.
    pub fn open_file_finder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        if self.sheet.is_some() {
            return;
        }
        let return_focus = self.take_menu().unwrap_or_else(|| window.focused(cx));
        self.close_comments_menu(cx);
        let view = cx.new(|cx| FileFinder::new(session, window, cx));
        let subscription = cx.subscribe_in(&view, window, Self::on_file_finder_event);
        view.update(cx, |finder, cx| finder.focus(window, cx));
        self.file_finder = Some(Overlay {
            view,
            return_focus,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn on_file_finder_event(
        &mut self,
        _: &Entity<FileFinder>,
        event: &FileFinderEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            FileFinderEvent::Dismiss => {
                self.close_menu(window, cx);
            }
            FileFinderEvent::Open(path) => {
                // The keyboard goes to the file view, not back where the finder opened from.
                self.file_finder = None;
                self.open_file_view(path, None, true, window, cx);
            }
        }
    }
}
