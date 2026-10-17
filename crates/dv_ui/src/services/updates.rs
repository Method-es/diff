//! dv's own updates as the app shows them (docs/PLAN.md D9, §13.3; DESIGN §4.1's status bar):
//! nothing, an update downloaded and ready, which the status bar's **Restart to update**
//! applies, or a restart that failed, which says why and can be tried again.
//!
//! The binary does the work ([`Updater`]): an installed copy checks its feed in the background
//! and downloads what is newer (`dv`'s `updates`, Velopack), then says so here
//! ([`Updates::set_ready`]); everything else (a development build, an automation run) has
//! none, and the item never shows.
//!
//! **Applying ends every dv process running from the install** (`Update.exe` stops them all
//! before it replaces the files): other dv windows, and an agent's `dv review wait` or `dv
//! comment reply`. So Restart to update asks first when that would lose something
//! ([`closes`]): other dv processes running from the install, or this window's unsent comment
//! or open Finish round sheet. The confirmation says what will close, with **Restart anyway**
//! and **Not now**; nothing is dropped without it.
//!
//! Under automation a scenario makes an update ready through the seam (`update_ready`), with
//! [`Updater::Scripted`]: a stand-in count of other processes and, when asked, a failure; it
//! records the restart instead of doing it (`state.update`).

use gpui::{App, Context};

/// The binary's restart into a downloaded update; an error says why it couldn't.
pub type Restart = Box<dyn Fn(&mut App) -> Result<(), String>>;

/// The other dv processes running from the install, which applying an update stops.
pub type CountOthers = Box<dyn Fn() -> usize>;

/// What restarting into a downloaded update does.
pub enum Updater {
    /// No updates: a development build, an automation run without the seam.
    None,
    /// Velopack's: counts the other dv processes of the install, and applies and restarts.
    Real {
        others: CountOthers,
        restart: Restart,
    },
    /// Automation's stand-in: `others` other dv processes, and `fail` the error a restart
    /// reports (`None`: it is recorded as done).
    Scripted { others: usize, fail: Option<String> },
}

/// Where the update stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateStatus {
    /// Nothing to apply.
    None,
    /// `version` is downloaded; a restart applies it.
    Ready { version: String },
}

/// What this window holds that a restart would lose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unsaved {
    /// A composer with text not posted.
    pub draft: bool,
    /// The Finish round sheet open.
    pub sheet: bool,
}

/// What applying an update would close, in words, for the confirmation: `others` other dv
/// processes running from the install, and this window's `unsaved` text; empty when nothing
/// would be lost, and the restart needs no question.
pub fn closes(others: usize, unsaved: Unsaved) -> Vec<String> {
    let mut closes = Vec::new();
    match others {
        0 => {}
        1 => closes.push("1 other dv window or command running from this install".to_string()),
        n => closes.push(format!(
            "{n} other dv windows or commands running from this install"
        )),
    }
    if unsaved.draft {
        closes.push("your unsent comment in this window".to_string());
    }
    if unsaved.sheet {
        closes.push("the Finish round sheet in this window".to_string());
    }
    closes
}

/// The update service (module docs).
pub struct Updates {
    status: UpdateStatus,
    updater: Updater,
    /// A restart was asked for: the real one is under way, or the scripted one recorded.
    restart_requested: bool,
    /// What would close, while the confirmation asks ([`closes`]).
    confirm: Option<Vec<String>>,
    /// Why the last restart failed, until another is tried.
    failure: Option<String>,
}

impl Updates {
    pub fn new() -> Self {
        Self {
            status: UpdateStatus::None,
            updater: Updater::None,
            restart_requested: false,
            confirm: None,
            failure: None,
        }
    }

    pub fn status(&self) -> &UpdateStatus {
        &self.status
    }

    /// The version a restart applies, when one is ready.
    pub fn ready_version(&self) -> Option<&str> {
        match &self.status {
            UpdateStatus::Ready { version } => Some(version),
            UpdateStatus::None => None,
        }
    }

    pub fn restart_requested(&self) -> bool {
        self.restart_requested
    }

    /// What would close, while the confirmation asks.
    pub fn confirm(&self) -> Option<&[String]> {
        self.confirm.as_deref()
    }

    /// Why the last restart failed.
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Whether the updater is automation's stand-in.
    pub fn is_scripted(&self) -> bool {
        matches!(self.updater, Updater::Scripted { .. })
    }

    /// Installs what a restart does.
    pub fn set_updater(&mut self, updater: Updater) {
        self.updater = updater;
    }

    /// `version` is downloaded and ready (the status bar shows Restart to update).
    pub fn set_ready(&mut self, version: String, cx: &mut Context<Self>) {
        let status = UpdateStatus::Ready { version };
        if self.status != status {
            self.status = status;
            cx.notify();
        }
    }

    /// Restart to update (or its Retry), with what this window holds (`unsaved`): asks first
    /// when applying would close something (module docs), else restarts. Nothing without an
    /// update ready.
    pub fn request_restart(&mut self, unsaved: Unsaved, cx: &mut Context<Self>) {
        if self.ready_version().is_none() || self.restart_requested {
            return;
        }
        let others = match &self.updater {
            Updater::None => 0,
            Updater::Real { others, .. } => others(),
            Updater::Scripted { others, .. } => *others,
        };
        let closes = closes(others, unsaved);
        if closes.is_empty() {
            self.restart_now(cx);
        } else {
            self.confirm = Some(closes);
            cx.notify();
        }
    }

    /// The confirmation's Restart anyway.
    pub fn confirm_restart(&mut self, cx: &mut Context<Self>) {
        if self.confirm.take().is_some() {
            self.restart_now(cx);
        }
    }

    /// The confirmation's Not now: nothing happens; the item stays.
    pub fn cancel_restart(&mut self, cx: &mut Context<Self>) {
        if self.confirm.take().is_some() {
            cx.notify();
        }
    }

    fn restart_now(&mut self, cx: &mut Context<Self>) {
        self.restart_requested = true;
        self.failure = None;
        cx.notify();
        let result = match &self.updater {
            Updater::None => Err("this dv has no updater".to_string()),
            Updater::Real { restart, .. } => restart(cx),
            Updater::Scripted { fail, .. } => fail.clone().map_or(Ok(()), Err),
        };
        if let Err(error) = result {
            log::error!(
                "couldn't restart into dv {}: {error}",
                self.ready_version().unwrap_or("?")
            );
            self.restart_requested = false;
            self.failure = Some(error);
            cx.notify();
        }
    }
}

impl Default for Updates {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{AppContext as _, TestAppContext};
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn a_restart_asks_when_it_would_close_something() {
        let clean = Unsaved::default();
        assert_eq!(closes(0, clean), Vec::<String>::new());
        assert_eq!(
            closes(1, clean),
            ["1 other dv window or command running from this install"]
        );
        assert_eq!(
            closes(
                2,
                Unsaved {
                    draft: true,
                    sheet: true
                }
            ),
            [
                "2 other dv windows or commands running from this install",
                "your unsent comment in this window",
                "the Finish round sheet in this window",
            ]
        );
        assert_eq!(
            closes(
                0,
                Unsaved {
                    draft: true,
                    sheet: false
                }
            ),
            ["your unsent comment in this window"]
        );
    }

    #[gpui::test]
    fn a_restart_needs_an_update_ready_and_a_scripted_one_is_recorded(cx: &mut TestAppContext) {
        let updates = cx.new(|_| Updates::new());
        updates.update(cx, |updates, cx| {
            updates.set_updater(Updater::Scripted {
                others: 0,
                fail: None,
            });
            updates.request_restart(Unsaved::default(), cx);
            assert!(!updates.restart_requested(), "nothing is ready");
            updates.set_ready("0.1.0-local.20261018120000".into(), cx);
            assert_eq!(updates.ready_version(), Some("0.1.0-local.20261018120000"));
            updates.request_restart(Unsaved::default(), cx);
            assert!(updates.restart_requested());
            assert_eq!(updates.confirm(), None);
            assert!(updates.is_scripted());
        });
    }

    #[gpui::test]
    fn other_processes_or_a_draft_ask_first_and_not_now_keeps_everything(cx: &mut TestAppContext) {
        let updates = cx.new(|_| Updates::new());
        updates.update(cx, |updates, cx| {
            updates.set_updater(Updater::Scripted {
                others: 2,
                fail: None,
            });
            updates.set_ready("2.0.0".into(), cx);
            updates.request_restart(Unsaved::default(), cx);
            assert_eq!(
                updates.confirm(),
                Some(&["2 other dv windows or commands running from this install".to_string()][..])
            );
            assert!(!updates.restart_requested());
            updates.cancel_restart(cx);
            assert_eq!(updates.confirm(), None);
            assert!(!updates.restart_requested());
            assert_eq!(updates.ready_version(), Some("2.0.0"));
            // Asked again, then Restart anyway.
            updates.request_restart(Unsaved::default(), cx);
            updates.confirm_restart(cx);
            assert!(updates.restart_requested());
            assert_eq!(updates.confirm(), None);
        });
    }

    #[gpui::test]
    fn the_real_restart_counts_the_others_and_runs_once(cx: &mut TestAppContext) {
        let ran = Rc::new(Cell::new(0));
        let updates = cx.new(|_| Updates::new());
        updates.update(cx, |updates, cx| {
            let counter = ran.clone();
            updates.set_updater(Updater::Real {
                others: Box::new(|| 0),
                restart: Box::new(move |_| {
                    counter.set(counter.get() + 1);
                    Ok(())
                }),
            });
            updates.set_ready("2.0.0".into(), cx);
            updates.request_restart(Unsaved::default(), cx);
            updates.request_restart(Unsaved::default(), cx);
        });
        assert_eq!(ran.get(), 1);
    }

    #[gpui::test]
    fn a_failed_restart_says_why_and_can_be_tried_again(cx: &mut TestAppContext) {
        let updates = cx.new(|_| Updates::new());
        updates.update(cx, |updates, cx| {
            updates.set_updater(Updater::Scripted {
                others: 0,
                fail: Some("Update.exe isn't there".into()),
            });
            updates.set_ready("2.0.0".into(), cx);
            updates.request_restart(Unsaved::default(), cx);
            assert_eq!(updates.failure(), Some("Update.exe isn't there"));
            assert!(!updates.restart_requested());
            updates.set_updater(Updater::Scripted {
                others: 0,
                fail: None,
            });
            updates.request_restart(Unsaved::default(), cx);
            assert_eq!(updates.failure(), None);
            assert!(updates.restart_requested());
        });
    }
}
