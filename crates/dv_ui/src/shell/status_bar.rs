//! The status bar (DESIGN §2, §4.1): where the review's repository lives and how dv reaches it
//! (the location and the backend's state), the review's status, round and threads, and the key
//! that finishes the round.

use dv_core::Location;
use dv_core::review::ThreadCounts;
use gpui::{
    AnyElement, App, Context, Hsla, ParentElement as _, Styled as _, div,
    prelude::FluentBuilder as _, px,
};

use super::Shell;
use crate::automation::AutomationTargetExt as _;
use crate::components::{kbd, status_label};
use crate::keymap;
use crate::theme::{DvTheme, StyledText as _};

/// How the backend is doing, as the status bar's dot shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// Nothing open.
    Idle,
    /// Opened and following changes.
    Live,
    /// Opened, but the watch can't follow the worktree (the review refreshes on focus).
    Degraded,
    /// It couldn't be opened, or its last refresh failed.
    Failing,
}

impl Health {
    /// `"idle"`, `"live"`, `"degraded"` or `"failing"`, for automation state.
    pub fn key(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Live => "live",
            Self::Degraded => "degraded",
            Self::Failing => "failing",
        }
    }

    fn color(self, theme: &DvTheme) -> Hsla {
        let c = &theme.colors;
        match self {
            Self::Idle => c.fg3,
            Self::Live => c.green,
            Self::Degraded => c.yellow,
            Self::Failing => c.red,
        }
    }
}

/// What the status bar says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusLine {
    pub health: Health,
    /// "Local · watching for changes", "Ubuntu · can't be opened", "No repo".
    pub location: String,
    /// "Changes requested · round 2 · 3 open · 1 resolved", with a review open.
    pub review: Option<String>,
    /// Whether "Ctrl Enter finish review" shows (an open review that isn't finished).
    pub finish: bool,
}

/// `Local`, or a WSL repository's distro.
fn place(location: &Location) -> String {
    match location {
        Location::Local(_) => "Local".to_string(),
        Location::Wsl { distro, .. } => distro.clone(),
    }
}

/// "3 open · 1 addressed · 1 resolved", leaving out what is zero; "no comments" when all are.
pub fn thread_counts(counts: ThreadCounts) -> String {
    let parts: Vec<String> = [
        (counts.open, "open"),
        (counts.addressed, "addressed"),
        (counts.resolved, "resolved"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, what)| format!("{count} {what}"))
    .collect();
    if parts.is_empty() {
        "no comments".to_string()
    } else {
        parts.join(" · ")
    }
}

impl Shell {
    /// What the status bar says now.
    pub fn status_line(&self, cx: &App) -> StatusLine {
        if let Some(opening) = &self.opening {
            return StatusLine {
                health: Health::Idle,
                location: format!("{} · opening…", place(&opening.location)),
                review: None,
                finish: false,
            };
        }
        if let Some(session) = &self.session {
            let session = session.read(cx);
            let (health, state) = if session.error().is_some() {
                (Health::Failing, "refresh failed")
            } else if session.watch_notice().is_some() {
                (Health::Degraded, "refreshes when focused")
            } else {
                (Health::Live, "watching for changes")
            };
            let review = session.review();
            // A finished review names the round that finished it, not one that never opens.
            return StatusLine {
                health,
                location: format!("{} · {state}", place(&session.backend().repo().location)),
                review: Some(format!(
                    "{} · round {} · {}",
                    status_label(review.status),
                    review.shown_round(),
                    thread_counts(review.summary().threads),
                )),
                finish: !session.is_read_only(),
            };
        }
        if let Some((location, _)) = self.banner.as_ref().and_then(|b| b.retry.as_ref()) {
            return StatusLine {
                health: Health::Failing,
                location: format!("{} · can't be opened", place(location)),
                review: None,
                finish: false,
            };
        }
        StatusLine {
            health: Health::Idle,
            location: "No repo".to_string(),
            review: None,
            finish: false,
        }
    }

    pub(super) fn render_status_bar(&self, theme: &DvTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let line = self.status_line(cx);
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(14.))
            .h(theme.sizes.status_bar_height)
            .px(theme.spacing.lg)
            .bg(c.chrome)
            .border_t_1()
            .border_color(c.line)
            .text_spec(&theme.text.secondary)
            .text_color(c.fg2)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.sm)
                    .child(
                        div()
                            .size(px(7.))
                            .rounded_full()
                            .bg(line.health.color(theme)),
                    )
                    .child(line.location)
                    .automation_target("statusbar/location"),
            )
            .when_some(line.review, |this, review| {
                this.child(div().flex_none().text_color(c.fg3).child("|"))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(review)
                            .automation_target("statusbar/review"),
                    )
            })
            .child(div().flex_1())
            .when(line.finish, |this| {
                this.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(kbd(keymap::label("secondary-enter"), theme))
                        .child("finish review")
                        .automation_target("statusbar/finish"),
                )
            })
            .automation_target("statusbar")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_counts_leave_out_zeros() {
        let counts = |open, addressed, resolved| ThreadCounts {
            open,
            addressed,
            resolved,
        };
        assert_eq!(thread_counts(counts(0, 0, 0)), "no comments");
        assert_eq!(thread_counts(counts(3, 0, 1)), "3 open · 1 resolved");
        assert_eq!(
            thread_counts(counts(1, 2, 3)),
            "1 open · 2 addressed · 3 resolved"
        );
    }
}
