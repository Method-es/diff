//! The "Finish round" sheet (DESIGN §4.2, local variant; PLAN §2.3, §3 workflow A step 3):
//! `Ctrl Enter` while no text field has the keyboard opens it over the scrim. On the left a
//! Summary (Write / Preview, Markdown) and the Verdict as radio cards, Approve and Request
//! changes, Request changes chosen when threads are still open, else Approve; on the right the
//! comments the round hands the agent: those opened in it and the earlier ones not yet
//! resolved. The footer finishes the round: `review::ops::finish_round` snapshots
//! the target's new side at `refs/dv/reviews/<id>/<n>` and closes the round with the verdict
//! and summary, which moves the review's status (`changes_requested`, `approved`).
//!
//! The keyboard starts on the verdicts: `↑`/`↓` (or `1`/`2`) choose, `Tab` moves into the
//! summary and back, `Ctrl Enter` finishes, `Esc` closes (`crate::actions::sheet`). While the
//! round is being finished nothing more can be submitted and the sheet doesn't close (`Esc`,
//! Cancel and the close button do nothing, the shell keeps it), so how it went always shows:
//! it closes once the round lands, and a failure shows in it. The shell opens and closes the
//! sheet and holds it like its other overlays; a menu opened over it leaves it, and its
//! summary, as they were (`crate::shell`).
//!
//! Automation targets: `sheet`, `sheet/summary`, `sheet/write`, `sheet/preview`,
//! `sheet/verdict/approve`, `sheet/verdict/request_changes`, `sheet/cancel`, `sheet/close`,
//! `sheet/submit`, `sheet/error`.

mod render;

#[cfg(test)]
mod tests;

use dv_core::review::{FinishedRound, Thread, ThreadState, Verdict};
use gpui::{AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Window};
use gpui_component::input::TextareaState;

use crate::actions::sheet::{
    Dismiss, PickApprove, PickRequestChanges, SelectNext, SelectPrevious, Submit, SwitchFocus,
};
use crate::failure::Failure;
use crate::session::ReviewSession;

/// The sheet's width (DESIGN §4.2).
pub const SHEET_WIDTH: gpui::Pixels = gpui::px(980.);

/// How far below the title bar the sheet hangs: the board's 92px in a 900px window, less in a
/// shorter one (down to 24px at DESIGN's 700px minimum), so the sheet fits.
pub fn sheet_top(viewport_height: gpui::Pixels) -> gpui::Pixels {
    let height = f32::from(viewport_height);
    gpui::px((24. + (height - 700.) * 68. / 200.).clamp(24., 92.))
}

/// The verdicts a local round closes with, in the order the sheet lists them.
pub const VERDICTS: [Verdict; 2] = [Verdict::Approve, Verdict::RequestChanges];

/// The verdict the sheet starts on: Request changes while a thread is open, else Approve.
pub fn default_verdict(open_threads: usize) -> Verdict {
    if open_threads > 0 {
        Verdict::RequestChanges
    } else {
        Verdict::Approve
    }
}

/// The primary button's words for `verdict`.
pub fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Approve => "Approve",
        Verdict::RequestChanges => "Request changes",
        Verdict::Comment => "Comment",
    }
}

/// The round a thread was opened in: its first anchor's, which a move recorded since (PLAN
/// §8, re-taken in the round of the event that recorded it) doesn't change.
pub fn opened_in(thread: &Thread) -> u32 {
    thread.first_anchor().round
}

/// The comments the "Comments" column lists for `round`, the one being finished: the threads
/// opened in it and earlier rounds' threads not yet resolved (the agent gets both with the
/// round), in the order given; never an annotation nobody asked about, which goes nowhere
/// (PLAN §19.1).
pub fn round_comments<'a>(
    threads: impl IntoIterator<Item = &'a Thread>,
    round: u32,
) -> Vec<&'a Thread> {
    threads
        .into_iter()
        .filter(|thread| thread.state != ThreadState::Annotation)
        .filter(|thread| opened_in(thread) == round || thread.state != ThreadState::Resolved)
        .collect()
}

/// Which way the summary shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryTab {
    Write,
    Preview,
}

/// What the sheet tells the shell.
#[derive(Clone, Debug)]
pub enum SheetEvent {
    /// Closed without finishing anything.
    Dismiss,
    /// The round closed.
    Finished(Box<FinishedRound>),
}

/// The sheet (see the module docs).
pub struct FinishSheet {
    session: Entity<ReviewSession>,
    /// The verdicts' radio group, which has the keyboard when the summary doesn't.
    focus_handle: FocusHandle,
    summary: Entity<TextareaState>,
    tab: SummaryTab,
    verdict: Verdict,
    submitting: bool,
    error: Option<Failure>,
}

impl EventEmitter<SheetEvent> for FinishSheet {}

impl Focusable for FinishSheet {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl FinishSheet {
    /// A sheet over `session`'s round in progress, its verdict chosen by the open threads.
    pub fn new(
        session: Entity<ReviewSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let open = session
            .read(cx)
            .review()
            .live_threads()
            .filter(|thread| thread.state == ThreadState::Open)
            .count();
        let summary = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(6, 6)
                .placeholder("A note for the agent (optional)")
        });
        cx.observe(&summary, |_, _, cx| cx.notify()).detach();
        Self {
            session,
            focus_handle: cx.focus_handle(),
            summary,
            tab: SummaryTab::Write,
            verdict: default_verdict(open),
            submitting: false,
            error: None,
        }
    }

    pub fn verdict(&self) -> Verdict {
        self.verdict
    }

    pub fn tab(&self) -> SummaryTab {
        self.tab
    }

    /// The summary as typed.
    pub fn summary_text(&self, cx: &gpui::App) -> String {
        self.summary.read(cx).value().to_string()
    }

    /// Whether Submit would do anything now: not while a round is being finished, and never on a
    /// finished review.
    pub fn can_submit(&self, cx: &gpui::App) -> bool {
        !self.submitting && !self.session.read(cx).is_read_only()
    }

    /// A round is being finished.
    pub fn is_submitting(&self) -> bool {
        self.submitting
    }

    /// Why the last attempt failed.
    pub fn error(&self) -> Option<&Failure> {
        self.error.as_ref()
    }

    /// Puts the keyboard on the verdicts.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
    }

    pub fn set_verdict(&mut self, verdict: Verdict, cx: &mut Context<Self>) {
        if self.verdict != verdict && VERDICTS.contains(&verdict) {
            self.verdict = verdict;
            cx.notify();
        }
    }

    pub fn set_tab(&mut self, tab: SummaryTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        // The field goes away in Preview; the keyboard returns to the verdicts.
        if tab == SummaryTab::Preview && self.summary.read(cx).focus_handle(cx).is_focused(window) {
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
    }

    /// Finishes the round (`ops::finish_round`) with the verdict and the summary.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.can_submit(cx) {
            return;
        }
        let body = self.summary.read(cx).value().trim().to_string();
        let body = (!body.is_empty()).then_some(body);
        let verdict = self.verdict;
        let finishing = self.session.update(cx, |session, cx| {
            session.finish_round(Some(verdict), body, cx)
        });
        self.submitting = true;
        self.error = None;
        cx.spawn(async move |this, cx| {
            let result = finishing.await;
            this.update(cx, |this, cx| {
                this.submitting = false;
                match result {
                    Ok(finished) => cx.emit(SheetEvent::Finished(Box::new(finished))),
                    Err(failure) => this.error = Some(failure),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn on_submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        self.submit(cx);
    }

    fn on_dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        self.dismiss(cx);
    }

    /// Asks the shell to close the sheet (`Esc`, Cancel, the close button); nothing while the
    /// round is being finished.
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        if !self.submitting {
            cx.emit(SheetEvent::Dismiss);
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step(VERDICTS.len() - 1, cx);
    }

    fn step(&mut self, by: usize, cx: &mut Context<Self>) {
        let at = VERDICTS
            .iter()
            .position(|v| *v == self.verdict)
            .unwrap_or(0);
        self.set_verdict(VERDICTS[(at + by) % VERDICTS.len()], cx);
    }

    fn pick_approve(&mut self, _: &PickApprove, _: &mut Window, cx: &mut Context<Self>) {
        self.set_verdict(Verdict::Approve, cx);
    }

    fn pick_request_changes(
        &mut self,
        _: &PickRequestChanges,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_verdict(Verdict::RequestChanges, cx);
    }

    /// `Tab`: the keyboard moves between the verdicts and the summary (Write shows first).
    fn switch_focus(&mut self, _: &SwitchFocus, window: &mut Window, cx: &mut Context<Self>) {
        let in_summary = self.summary.read(cx).focus_handle(cx).is_focused(window);
        if in_summary {
            self.focus_handle.focus(window, cx);
        } else {
            self.tab = SummaryTab::Write;
            self.summary
                .update(cx, |summary, cx| summary.focus(window, cx));
        }
        cx.notify();
    }
}
