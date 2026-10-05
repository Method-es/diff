//! The `state` document's parts (`tests/e2e/README.md` "dv's `state`" lists every key).

use dv_core::diff::{FileEntry, FileStatus};
use dv_core::review::{AnchorSide, Author, ReviewStatus, Thread};
use dv_ui::services::reviews::{RepoRow, ReviewRow};
use dv_ui::session::{FileState, Placement, ReviewSession, diff_kind};
use dv_ui::submit::SummaryTab;
use dv_ui::switcher::items::{ItemKind, ReviewBadge};
use dv_ui::system::SystemRequest;
use dv_ui::{ActiveServices as _, Failure, Material, Shell};
use gpui::{App, Focusable as _, Window};
use serde_json::{Value, json};

/// `state.settings`.
pub fn settings(cx: &App) -> Value {
    let settings = cx.services().settings.read(cx);
    let resolved = dv_ui::appearance::resolved(cx);
    let available: Vec<&str> = dv_ui::appearance::support(cx)
        .available()
        .into_iter()
        .map(Material::key)
        .collect();
    json!({
        "material": settings.material().key(),
        "mode": settings.mode().key(),
        "resolved_material": resolved.material.key(),
        "resolved_mode": resolved.appearance.key(),
        "available_materials": available,
        "path": settings.path().display().to_string(),
        "save_error": settings.last_error(),
    })
}

/// `state.shell`.
pub fn shell(shell: &Shell, cx: &App) -> Value {
    let status = shell.status_line(cx);
    json!({
        "panel_visible": shell.panel_visible(),
        "panel_width": cx.services().settings.read(cx).panel_width(),
        "empty_state": shell.empty_state(cx).map(|empty| empty.key()),
        "window_material": shell.applied_material().key(),
        "opening": shell.opening().is_some(),
        "banner": shell.banner().map(|banner| failure(Some(&banner.failure))),
        "status": {
            "health": status.health.key(),
            "location": status.location,
            "review": status.review,
            "finish": status.finish,
        },
    })
}

fn badge(badge: Option<&ReviewBadge>) -> Value {
    match badge {
        Some(badge) => json!({
            "id": badge.id,
            "status": badge.status.as_str(),
            "round": badge.round,
            "open_threads": badge.open_threads,
        }),
        None => Value::Null,
    }
}

/// `state.switcher`.
pub fn switcher(shell: &Shell, cx: &App) -> Value {
    let Some(switcher) = shell.switcher() else {
        return json!({"open": false, "query": null, "selected": null, "items": []});
    };
    let switcher = switcher.read(cx);
    let listing = switcher.listing();
    let items: Vec<Value> = listing
        .items
        .iter()
        .map(|item| {
            let repo = item.repo.and_then(|repo| listing.repos.get(repo));
            json!({
                "key": item.key,
                "kind": item.kind_key(),
                "target": match &item.kind {
                    ItemKind::Target(offer) => Some(offer.kind.key()),
                    _ => None,
                },
                "label": item.label(),
                "repo": repo.map(|repo| repo.path.clone()),
                "review": badge(item.review()),
            })
        })
        .collect();
    json!({
        "open": true,
        "query": switcher.query(),
        "selected": switcher.selected_key(),
        "items": items,
    })
}

/// `state.source_picker`.
pub fn source_picker(shell: &Shell, cx: &App) -> Value {
    let Some(picker) = shell.source_picker() else {
        return json!({"open": false, "selected": null, "items": []});
    };
    let picker = picker.read(cx);
    let items: Vec<Value> = picker
        .offers()
        .iter()
        .map(|offer| {
            json!({
                "kind": offer.kind.key(),
                "label": offer.label,
                "review": badge(offer.review.as_ref()),
            })
        })
        .collect();
    json!({
        "open": true,
        "selected": picker.selected().map(|offer| offer.kind.key()),
        "items": items,
    })
}

/// `state.file_list`.
pub fn file_list(shell: &Shell, cx: &App) -> Value {
    let list = shell.file_list().read(cx);
    json!({
        "query": list.query(),
        "shown": list.shown_paths(cx),
        "current_file": shell.diff_pane().read(cx).current_file(),
    })
}

/// `state.system`.
pub fn system(cx: &App) -> Value {
    let (requests, answers_left) = dv_ui::system::requests(cx);
    let requests: Vec<Value> = requests
        .iter()
        .map(|request| match request {
            SystemRequest::OpenUrl(url) => json!({"kind": request.kind(), "url": url}),
            SystemRequest::Reveal(path) => {
                json!({"kind": request.kind(), "path": path.display().to_string()})
            }
            SystemRequest::PickFolder { prompt } => {
                json!({"kind": request.kind(), "prompt": prompt.as_ref()})
            }
            SystemRequest::Clipboard(text) => json!({"kind": request.kind(), "text": text}),
        })
        .collect();
    json!({"requests": requests, "answers_left": answers_left})
}

fn failure(failure: Option<&Failure>) -> Value {
    match failure {
        Some(failure) => json!({"message": failure.message, "detail": failure.detail}),
        None => Value::Null,
    }
}

/// `state.repos`: the known repositories, most recently used first.
pub fn repos(cx: &App) -> Value {
    let reviews = cx.services().reviews.read(cx);
    Value::Array(reviews.rows().into_iter().map(repo).collect())
}

fn repo(row: &RepoRow) -> Value {
    let reviews: Vec<Value> = row
        .reviews
        .iter()
        .map(|review| match review {
            ReviewRow::Ok { summary, label } => json!({
                "id": summary.id,
                "target": summary.target,
                "target_label": label,
                "status": summary.status.as_str(),
                "round": summary.current_round,
                "threads": {
                    "open": summary.threads.open,
                    "addressed": summary.threads.addressed,
                    "resolved": summary.threads.resolved,
                },
            }),
            ReviewRow::Err { id, error } => json!({"id": id, "error": failure(Some(error))}),
        })
        .collect();
    json!({
        "path": row.path,
        "name": row.name,
        "location": row.location,
        "loading": row.loading,
        "error": failure(row.error.as_ref()),
        "working": row.working.map(|working| json!({
            "files": working.files,
            "additions": working.additions,
            "deletions": working.deletions,
        })),
        "reviews": reviews,
    })
}

/// `state.session`: the open review, or `null`.
pub fn session(shell: &Shell, cx: &App) -> Value {
    let Some(session) = shell.session() else {
        return Value::Null;
    };
    let session = session.read(cx);
    let review = session.review();
    let files: Vec<Value> = session
        .files()
        .iter()
        .map(|entry| file(session, entry))
        .collect();
    let threads: Vec<Value> = review
        .live_threads()
        .map(|thread| thread_state(session, thread))
        .collect();
    let rounds: Vec<Value> = review
        .rounds
        .iter()
        .map(|round| {
            json!({
                "n": round.n,
                "verdict": round.verdict.map(|verdict| verdict.as_str()),
                "old_oid": round.old_oid,
                "new_oid": round.new_oid,
                "body": round.body,
            })
        })
        .collect();
    json!({
        "review_id": review.id,
        "repo": session.backend().repo().toplevel,
        "target": review.target,
        "target_label": session.target_label(),
        "status": status(review.status),
        "round": review.shown_round(),
        "read_only": session.is_read_only(),
        "loading": session.is_loading(),
        "error": failure(session.error()),
        "viewed_error": failure(session.viewed_error()),
        "watch_notice": session.watch_notice(),
        "generation": session.generation(),
        "warnings": session.warnings().len(),
        "files": files,
        "viewed": session.viewed_count(),
        "total": session.files().len(),
        "current_file": shell.diff_pane().read(cx).current_file(),
        "view_mode": shell.diff_pane().read(cx).view_mode().key(),
        "cursor": shell.diff_pane().read(cx).cursor().map(|cursor| json!({
            "path": cursor.path,
            "side": side(cursor.side),
            "line": cursor.line,
        })),
        "selection": shell.diff_pane().read(cx).selection().map(|selection| json!({
            "path": selection.path,
            "side": side(selection.side),
            "start_line": selection.start_line,
            "end_line": selection.end_line,
        })),
        "threads": threads,
        "composer": composer(shell, cx),
        "focused_thread": shell.diff_pane().read(cx).focused_thread(),
        "sheet": sheet(shell, cx),
        "rounds": rounds,
    })
}

/// `state.session.composer`: the open composer, or `null`.
fn composer(shell: &Shell, cx: &App) -> Value {
    let pane = shell.diff_pane().read(cx);
    let Some(composer) = pane.composer() else {
        return Value::Null;
    };
    let target = &composer.target;
    json!({
        "path": target.path,
        "side": side(target.side),
        "start_line": target.start_line,
        "end_line": target.end_line,
        "reply_to": target.reply_to,
        "text": pane.composer_text(cx),
        "posting": composer.posting,
        "error": failure(composer.error.as_ref()),
    })
}

/// `state.session.sheet`: the "Finish round" sheet, or `null`.
fn sheet(shell: &Shell, cx: &App) -> Value {
    let Some(sheet) = shell.sheet() else {
        return Value::Null;
    };
    let sheet = sheet.read(cx);
    json!({
        "verdict": sheet.verdict().as_str(),
        "can_submit": sheet.can_submit(cx),
        "submitting": sheet.is_submitting(),
        "tab": match sheet.tab() {
            SummaryTab::Write => "write",
            SummaryTab::Preview => "preview",
        },
        "summary": sheet.summary_text(cx),
        "error": failure(sheet.error()),
    })
}

/// `state.comments_list`: the panel's comments (`null` with no review open): the rows shown,
/// in order, and whether the resolved threads are among them.
pub fn comments_list(shell: &Shell, cx: &App) -> Value {
    if shell.session().is_none() {
        return Value::Null;
    }
    let list = shell.comments_list().read(cx);
    let rows: Vec<Value> = list
        .rows(cx)
        .into_iter()
        .map(|row| {
            json!({
                "id": row.id,
                "state": row.state.as_str(),
                "outdated": row.outdated,
                "location": row.location,
                "summary": row.summary,
                "snippet": row.snippet,
            })
        })
        .collect();
    json!({"show_resolved": list.shows_resolved(), "rows": rows})
}

fn side(side: AnchorSide) -> &'static str {
    match side {
        AnchorSide::Old => "old",
        AnchorSide::New => "new",
    }
}

/// `state.stream`: the diff stream's own state (`null` with no review open): the file at the
/// top, the one whose header is pinned, the sideways scroll, and per file whether it is
/// collapsed, its rows, the lines each gap hides and the notice it shows.
pub fn stream(shell: &Shell, window: &Window, cx: &App) -> Value {
    if shell.session().is_none() {
        return Value::Null;
    }
    let pane = shell.diff_pane().read(cx);
    let files: Vec<Value> = pane
        .file_summaries()
        .into_iter()
        .map(|file| {
            json!({
                "path": file.path,
                "collapsed": file.collapsed,
                "rows": file.rows,
                "gaps": file.gaps,
                "notice": file.notice,
            })
        })
        .collect();
    json!({
        "top_file": pane.top_file(),
        "pinned_file": pane.pinned_file(),
        "h_offset": pane.h_offset(),
        "focused": shell.focus_handle(cx).is_focused(window),
        "files": files,
    })
}

fn status(status: ReviewStatus) -> &'static str {
    status.as_str()
}

/// A file status as one word: `added`, `modified`, `deleted`, `renamed`, `copied`,
/// `type_changed`, `untracked` or `unmerged`.
fn file_status(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Added => "added",
        FileStatus::Modified => "modified",
        FileStatus::Deleted => "deleted",
        FileStatus::Renamed { .. } => "renamed",
        FileStatus::Copied { .. } => "copied",
        FileStatus::TypeChanged => "type_changed",
        FileStatus::Untracked => "untracked",
        FileStatus::Unmerged => "unmerged",
    }
}

fn file(session: &ReviewSession, entry: &FileEntry) -> Value {
    let state = session.file_state(&entry.path);
    json!({
        "path": entry.path,
        "old_path": entry.old_path,
        "status": file_status(entry.status),
        "additions": entry.additions,
        "deletions": entry.deletions,
        "binary": entry.binary,
        "viewed": session.is_viewed(&entry.path),
        "open_threads": session.open_threads_in(&entry.path),
        "state": state.map_or("unloaded", FileState::key),
        "kind": state.and_then(FileState::diff).map(|diff| diff_kind(diff)),
        "highlighted": state.and_then(FileState::highlights).is_some(),
    })
}

fn author(author: &Author) -> Value {
    json!({"kind": author.kind.as_str(), "name": author.name})
}

fn thread_state(session: &ReviewSession, thread: &Thread) -> Value {
    let anchor = &thread.anchor;
    let replies: Vec<Value> = thread
        .replies
        .iter()
        .filter(|reply| !reply.is_deleted())
        .map(|reply| json!({"id": reply.id, "author": author(&reply.author), "body": reply.body}))
        .collect();
    json!({
        "id": thread.id,
        "path": anchor.path,
        "side": side(anchor.side),
        "start_line": anchor.start_line,
        "end_line": anchor.end_line,
        "state": thread.state.as_str(),
        "note": thread.last_change.as_ref().and_then(|change| change.note.clone()),
        "author": author(&thread.author),
        "body": thread.body,
        "replies": replies,
        "outdated": match session.placement(thread) {
            Placement::Inline => json!(false),
            Placement::Outdated => json!(true),
            Placement::Unknown => Value::Null,
        },
        "round": anchor.round,
    })
}
