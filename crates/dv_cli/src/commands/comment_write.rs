//! `dv comment add`, `reply`, `address`, `resolve` and `reopen` (docs/PLAN.md §10.1, §2.5).
//!
//! Who may do what is `dv_core::review::ops`'s rule: agents and the reviewer add, reply and
//! address; only the reviewer resolves and reopens. A write on a thread records where
//! relocation found it when it moved, or its file's name now when it was renamed (PLAN §8),
//! exactly as the GUI's writes do, and an agent's
//! first write on a `changes_requested` review moves it to `in_progress` (PLAN §2.4).

use dv_core::diff::{FileDiff, FileEntry, FileStatus};
use dv_core::review::{
    AnchorPlace, AnchorSide, Author, ReviewId, ThreadState, anchor_selection, has_side, ops,
};

use super::format;
use crate::args::{CommentAddArgs, CommentReplyArgs, GlobalArgs, Note, SideArg};
use crate::body;
use crate::context::Context;
use crate::dto::{CommentShowDoc, ReplyDoc, ThreadChangeDoc, ThreadDto};
use crate::error::{CliError, Exit, code};
use crate::identity;
use crate::output;
use crate::placement::{self, Sides};
use crate::repo::Repo;
use crate::select;

/// `dv comment add --file <p> [--lines N[:M]] [--side old|new] (--body | --body-file)`: a new
/// thread on lines of a file in the review's current diff (its full target, never an
/// interdiff), anchored on that side's text as it is now, in the round in progress. Without
/// `--lines` it is a comment on the whole file (a file-level anchor, PLAN §8), on its new side,
/// or with `--side old` on a deleted file's old side (refused on any other file, as the GUI
/// makes none), which reads none of the file's text, so any changed file takes one, a binary
/// one included.
pub(crate) fn add(
    global: &GlobalArgs,
    args: &CommentAddArgs,
    ctx: &mut Context,
) -> Result<Exit, CliError> {
    let body = body::required(&args.body, "comment", ctx)?;
    let repo = Repo::open(global, ctx)?;
    let author = identity::author(global, ctx, repo.backend.as_ref())?;
    let review = select::review(&repo, global, ctx, None)?;
    let file = repo.relative_path(&args.file, &review, ctx);
    let side = match args.side {
        SideArg::Old => AnchorSide::Old,
        SideArg::New => AnchorSide::New,
    };

    let mut sides = Sides::list(&repo, &review, ctx)?;
    let entry = sides
        .entries
        .iter()
        .find(|entry| entry.path == file)
        .or_else(|| {
            sides
                .entries
                .iter()
                .find(|entry| entry.old_path.as_deref() == Some(file.as_str()))
        })
        .cloned()
        .ok_or_else(|| {
            CliError::not_found(format!(
                "{file} isn't among the changed files of review {}",
                review.id
            ))
        })?;
    if !has_side(&entry, side) {
        return Err(rejected(missing_side(&entry, side)));
    }
    let place = AnchorPlace::for_entry(&entry, side, review.current_round());
    let anchor = match args.lines {
        // A comment on the whole file goes on its new side; only a deleted file's is its old.
        None if side == AnchorSide::Old && entry.status != FileStatus::Deleted => {
            return Err(rejected(format!(
                "{} isn't deleted: a comment on the whole file goes on its new side (leave                  `--side old` out, or name lines with `--lines`)",
                entry.path
            )));
        }
        None => place.file_level(),
        Some(range) => {
            if let Some(why) = no_text(sides.read(&entry.path)?) {
                return Err(rejected(format!(
                    "{} has no lines to comment on: {why}",
                    entry.path
                )));
            }
            let lines = sides.side_text(&entry.path, side).unwrap_or_default();
            anchor_selection(place, &lines, range.start, range.end).map_err(|error| {
                rejected(format!(
                    "--lines {}: {error}",
                    format::lines(range.start, range.end)
                ))
            })?
        }
    };

    let id = ops::add_thread(&repo.store, &review.id, anchor, &body, author.clone())?;
    started_work(&repo, &review.id, &author);
    let review = select::load(&repo, &review.id)?;
    let thread = select::live_thread(&review, &id)?;
    let doc = CommentShowDoc {
        thread: ThreadDto::new(&review, thread, &sides.locate(thread)),
    };
    if global.json {
        output::json(ctx, &doc)?;
    } else {
        output::text(
            ctx,
            &[format!(
                "added {}  {}",
                doc.thread.id,
                format::location(&doc.thread)
            )],
        )?;
    }
    Ok(Exit::Ok)
}

/// `dv comment reply <id> (--body | --body-file)`.
pub(crate) fn reply(
    global: &GlobalArgs,
    args: &CommentReplyArgs,
    ctx: &mut Context,
) -> Result<Exit, CliError> {
    let body = body::required(&args.body, "reply", ctx)?;
    let repo = Repo::open(global, ctx)?;
    let author = identity::author(global, ctx, repo.backend.as_ref())?;
    let (review, id) = select::thread(&repo, global, &args.thread.id)?;
    let thread = select::live_thread(&review, &id)?;
    let (located, relocated) = placement::locate_for_write(&repo, &review, thread, ctx)?;

    let reply_id = ops::reply(
        &repo.store,
        &review.id,
        &id,
        &body,
        author.clone(),
        relocated,
    )?;
    started_work(&repo, &review.id, &author);
    let review = select::load(&repo, &review.id)?;
    let thread = select::live_thread(&review, &id)?;
    let reply = thread.reply(&reply_id).ok_or_else(|| {
        CliError::not_found(format!("reply {reply_id} isn't in thread {id} any more"))
    })?;
    let doc = ReplyDoc {
        reply: reply.into(),
        thread: ThreadDto::new(&review, thread, &located),
    };
    if global.json {
        output::json(ctx, &doc)?;
    } else {
        output::text(
            ctx,
            &[format!(
                "replied to {}  {}  {}",
                doc.thread.id,
                doc.reply.id,
                format::location(&doc.thread)
            )],
        )?;
    }
    Ok(Exit::Ok)
}

/// `dv comment address <id> [--note | --note-file]`, `dv comment resolve <id>` and `dv comment
/// reopen <id>`: the thread moves to `state`, recording where it moved to.
pub(crate) fn set_state(
    global: &GlobalArgs,
    id: &str,
    state: ThreadState,
    note: Option<&Note>,
    ctx: &mut Context,
) -> Result<Exit, CliError> {
    let note = match note {
        Some(note) => body::note(note, ctx)?,
        None => None,
    };
    let repo = Repo::open(global, ctx)?;
    let author = identity::author(global, ctx, repo.backend.as_ref())?;
    let (review, id) = select::thread(&repo, global, id)?;
    let thread = select::live_thread(&review, &id)?;
    let (located, relocated) = placement::locate_for_write(&repo, &review, thread, ctx)?;

    let changed = ops::set_thread_state(
        &repo.store,
        &review.id,
        &id,
        state,
        note.as_deref(),
        author.clone(),
        relocated,
    )?;
    if changed {
        started_work(&repo, &review.id, &author);
    }
    let review = select::load(&repo, &review.id)?;
    let thread = select::live_thread(&review, &id)?;
    let doc = ThreadChangeDoc {
        changed,
        thread: ThreadDto::new(&review, thread, &located),
    };
    if global.json {
        output::json(ctx, &doc)?;
    } else {
        let unchanged = if changed { "" } else { " (unchanged)" };
        output::text(
            ctx,
            &[format!(
                "{}  {}{unchanged}  {}",
                doc.thread.id,
                doc.thread.state,
                format::location(&doc.thread)
            )],
        )?;
    }
    Ok(Exit::Ok)
}

/// An agent's first write on a `changes_requested` review moves it to `in_progress` (PLAN
/// §2.4), so the reviewer sees the agent at work. `ops::start_work` checks the status as the
/// review stands after the write and makes its event conditional on it, so a status set
/// meanwhile (the agent's own `dv review done` run beside this command, the reviewer's
/// verdict) wins even when it lands just before that event. Best effort: the write itself has
/// landed, so a failure is only logged.
fn started_work(repo: &Repo, review: &ReviewId, author: &Author) {
    if let Err(error) = ops::start_work(&repo.store, review, author) {
        log::warn!("review {review}: couldn't mark it in progress: {error}");
    }
}

/// Why `entry` has no `side` to comment on.
fn missing_side(entry: &FileEntry, side: AnchorSide) -> String {
    let what = match entry.status {
        FileStatus::Added | FileStatus::Untracked => "is new",
        FileStatus::Deleted => "is deleted",
        _ => "has no such side",
    };
    let side = match side {
        AnchorSide::Old => "old",
        AnchorSide::New => "new",
    };
    let hint = match entry.status {
        FileStatus::Deleted => " (`--side old` comments on it as it was)",
        _ => "",
    };
    format!("{} {what}: it has no {side} side{hint}", entry.path)
}

/// Why a file's diff has no lines, or `None` when it is text.
fn no_text(diff: &FileDiff) -> Option<&'static str> {
    match diff {
        FileDiff::Text(_) => None,
        FileDiff::Binary { .. } => Some("it is binary"),
        FileDiff::TooLarge { .. } => Some("it is too large"),
        FileDiff::Submodule { .. } => Some("it is a submodule"),
        FileDiff::Unmerged => Some("it has unresolved conflicts"),
    }
}

fn rejected(message: String) -> CliError {
    CliError::new(Exit::Failure, code::REJECTED, message)
}
