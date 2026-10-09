//! The reviewer's writes, each a checked `dv_core::review::ops` call on a background thread
//! (never a hand-built event), followed by a fold reload.
//!
//! **Viewed toggles** flip the state the reviewer last asked for, not the fold's: the fold only
//! moves once a write has landed and been read back, so `V V` pressed faster than that would
//! otherwise ask for the same state twice and lose the second press. One write per file runs
//! at a time; a toggle made while it runs changes what is asked for, and the write that
//! follows (if the ask moved) sets it. The ask is forgotten once a fold that started after the
//! file's last write landed has been read.

use dv_core::GitRepo;
use dv_core::review::{
    Anchor, Author, CommentId, FinishedRound, OpError, ReviewId, ReviewStore, ThreadId,
    ThreadState, Verdict, ops,
};
use futures::channel::oneshot;
use gpui::{AppContext as _, Context, Task};

use super::ReviewSession;
use crate::failure::Failure;

/// What a write runs with.
struct WriteCx {
    store: ReviewStore,
    repo: GitRepo,
    review: ReviewId,
    reviewer: Author,
}

/// The viewed state the reviewer asked for on one file (module docs).
#[derive(Clone, Copy, Debug)]
pub(super) struct ViewedIntent {
    /// Viewed or not.
    pub on: bool,
    /// The round it was asked for in.
    pub round: u32,
    /// A write for this file is running.
    writing: bool,
    /// How many fold reloads had started when the file's last write landed: a reload numbered
    /// above it reads that write.
    landed_at: u64,
}

impl ViewedIntent {
    /// Whether a fold reload numbered `fold` read everything asked for.
    pub(super) fn read_back_by(&self, fold: u64) -> bool {
        !self.writing && fold > self.landed_at
    }
}

impl ReviewSession {
    /// Marks `path` viewed (`on`) or not viewed in the round in progress (`V`). `false` when it
    /// already was.
    pub fn mark_viewed(
        &mut self,
        path: &str,
        on: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<bool, Failure>> {
        let path = path.to_string();
        self.write(cx, move |w| {
            ops::mark_viewed(&w.store, &w.review, &path, on)
        })
    }

    /// Toggles whether `path` is viewed in the round in progress (`V`, the panel's checkbox):
    /// it flips [`ReviewSession::intends_viewed`] (module docs). Nothing on a finished review
    /// or for a file the list doesn't hold. A failure is kept as
    /// [`ReviewSession::viewed_error`] for the main area's banner, since the checkbox that
    /// doesn't move can't say why; the next toggle that lands clears it.
    pub fn toggle_viewed(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.is_read_only() || self.file(path).is_none() {
            return;
        }
        let on = !self.intends_viewed(path);
        let round = self.review.current_round();
        let intent = self
            .viewed_intents
            .entry(path.to_string())
            .or_insert(ViewedIntent {
                on,
                round,
                writing: false,
                landed_at: 0,
            });
        intent.on = on;
        intent.round = round;
        if !intent.writing {
            self.write_viewed(path.to_string(), on, cx);
        }
    }

    /// Writes `path`'s viewed state, then, should the reviewer have asked for the other one
    /// meanwhile, that one.
    fn write_viewed(&mut self, path: String, on: bool, cx: &mut Context<Self>) {
        if let Some(intent) = self.viewed_intents.get_mut(&path) {
            intent.writing = true;
        }
        self.viewed_pending += 1;
        let op_path = path.clone();
        let writing = self.write_with(
            cx,
            move |w| ops::mark_viewed(&w.store, &w.review, &op_path, on),
            move |this, result, cx| {
                this.viewed_pending -= 1;
                this.viewed_error = result.as_ref().err().map(|failure| Failure {
                    message: format!(
                        "{path} couldn't be marked {}: {}",
                        if on { "viewed" } else { "not viewed" },
                        failure.message
                    ),
                    detail: failure.detail.clone(),
                });
                // A failed write leaves the stored state as it was, which the next toggle then
                // flips; an intent kept here would flip the state that failed to land instead.
                if result.is_err() {
                    this.viewed_intents.remove(&path);
                    return;
                }
                let folds_started = this.folds_started;
                let again = this.viewed_intents.get_mut(&path).and_then(|intent| {
                    intent.writing = false;
                    intent.landed_at = folds_started;
                    (intent.on != on).then_some(intent.on)
                });
                if let Some(again) = again {
                    this.write_viewed(path, again, cx);
                }
            },
        );
        writing.detach();
    }

    /// Opens a thread at `anchor` (from [`ReviewSession::anchor_for`]) with `body`.
    pub fn add_thread(
        &mut self,
        anchor: Anchor,
        body: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<ThreadId, Failure>> {
        self.write(cx, move |w| {
            ops::add_thread(&w.store, &w.review, anchor, &body, w.reviewer.clone())
        })
    }

    /// Replies to a thread as the reviewer, recording where it moved, or its file's name now
    /// (PLAN §8: relocation is display-only until the next event on the thread).
    pub fn reply(
        &mut self,
        thread: ThreadId,
        body: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<CommentId, Failure>> {
        let relocated = self.relocated_anchor(&thread);
        self.write(cx, move |w| {
            ops::reply(
                &w.store,
                &w.review,
                &thread,
                &body,
                w.reviewer.clone(),
                relocated,
            )
        })
    }

    /// Resolves or reopens a thread as the reviewer (PLAN §2.5), recording where it moved as
    /// a reply does. `false` when it already was.
    pub fn set_thread_state(
        &mut self,
        thread: ThreadId,
        state: ThreadState,
        note: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<bool, Failure>> {
        let relocated = self.relocated_anchor(&thread);
        self.write(cx, move |w| {
            ops::set_thread_state(
                &w.store,
                &w.review,
                &thread,
                state,
                note.as_deref(),
                w.reviewer.clone(),
                relocated,
            )
        })
    }

    /// Finishes the round in progress (the "Finish round" sheet, DESIGN §4.2): snapshots the
    /// target's new side and closes the round with the verdict and summary
    /// (`ops::finish_round`, PLAN §2.3).
    pub fn finish_round(
        &mut self,
        verdict: Option<Verdict>,
        body: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<FinishedRound, Failure>> {
        self.write(cx, move |w| {
            ops::finish_round(&w.store, &w.repo, &w.review, verdict, body, &w.reviewer)
        })
    }

    /// Runs `op` in the background as one write: counted for `settled` until it lands, and
    /// followed by a fold reload whether it succeeded or not (another writer may have moved
    /// the review). Dropping the returned task doesn't stop the write.
    fn write<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        op: impl FnOnce(&WriteCx) -> Result<T, OpError> + Send + 'static,
    ) -> Task<Result<T, Failure>> {
        self.write_with(cx, op, |_, _, _| {})
    }

    /// [`ReviewSession::write`], with `landed` run on the session as the write lands, before
    /// the fold reload that follows it starts.
    fn write_with<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        op: impl FnOnce(&WriteCx) -> Result<T, OpError> + Send + 'static,
        landed: impl FnOnce(&mut Self, &Result<T, Failure>, &mut Context<Self>) + 'static,
    ) -> Task<Result<T, Failure>> {
        self.writes += 1;
        let context = WriteCx {
            store: self.store.clone(),
            repo: self.repo.clone(),
            review: self.review.id.clone(),
            reviewer: self.reviewer.clone(),
        };
        let job = cx.background_spawn(async move {
            op(&context).map_err(|error| {
                log::warn!("review {}: {error}", context.review);
                Failure::from(&error)
            })
        });
        let (sender, receiver) = oneshot::channel();
        cx.spawn(async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| {
                this.writes -= 1;
                landed(this, &result, cx);
                this.reload_fold(cx);
                cx.notify();
            })
            .ok();
            let _ = sender.send(result);
        })
        .detach();
        cx.background_spawn(async move {
            receiver
                .await
                .unwrap_or_else(|_| Err(Failure::new("dv closed before the change was written")))
        })
    }
}
