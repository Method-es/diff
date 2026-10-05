//! The commands that open repositories and reviews, and the one that plays the agent:
//!
//! - `open {path}`: like `dv open <path>`: the repository becomes a known one (its worktree
//!   root; a WSL path as given), its switcher row is computed, and the switcher opens with its
//!   first row selected. Replies `{path, location}`.
//! - `open_review {path, target?}`: opens or resumes the review of `target` (default
//!   `"working"`; any target in its stored shape, e.g. `"staged"` or
//!   `{"branch": {"branch": "feat", "base": null}}`) and shows it, through the shell as the
//!   switcher would. Replies `{review_id, created}`; `settle` then waits for its file list.
//! - `review_op {op, thread, body?, note?, author?}`: an agent's change to the open review
//!   through `dv_core::review::ops` (until the CLI exists, this is how scenarios play the
//!   agent): `reply` (with `body`), `address` (with an optional `note`) or `done` (the review's
//!   status to `ready`, as `dv review done`; no `thread`). `author` is the agent's name
//!   (`agent` by default). The open review sees the change through its watch, as it would an
//!   agent's, so scenarios `wait_for` it.

use anyhow::{Context as _, Result, anyhow, bail};
use dv_core::review::{Author, AuthorKind, ReviewStatus, ThreadId, ThreadState, ops};
use dv_core::{Location, Target};
use dv_ui::services::reviews::OpenRequest;
use dv_ui::{ActiveServices as _, Shell};
use gpui::{AppContext as _, Context, Entity, Task, Window};
use serde_json::{Map, Value, json};

use super::DvAutomation;

/// `args` as an object whose keys are all in `known`.
fn object(cmd: &str, args: Value, known: &[&str]) -> Result<Map<String, Value>> {
    let object = match args {
        Value::Object(object) => object,
        Value::Null => Map::new(),
        other => bail!("{cmd} takes an object, not {other}"),
    };
    if let Some(unknown) = object.keys().find(|key| !known.contains(&key.as_str())) {
        bail!("{cmd} takes {}, not {unknown:?}", known.join(", "));
    }
    Ok(object)
}

fn string<'a>(cmd: &str, args: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("{cmd} needs a string {key:?}"))
}

/// `open {path}`.
pub fn open(
    shell: Entity<Shell>,
    args: Value,
    window: &mut Window,
    cx: &mut Context<DvAutomation>,
) -> Result<Task<Result<Value>>> {
    let args = object("open", args, &["path"])?;
    let location = Location::parse(string("open", &args, "path")?)?;
    let reviews = cx.services().reviews.clone();
    let added = reviews.update(cx, |reviews, cx| reviews.add_repo(location, cx));
    Ok(cx.spawn_in(window, async move |_, cx| {
        let known = added.await.map_err(|failure| anyhow!("{failure}"))?;
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.open_switcher(Some(&known), window, cx)
            })
        })?;
        Ok(json!({"path": known.display_name(), "location": known}))
    }))
}

/// The `target` of `open_review`: `working` when absent.
fn target(args: &Map<String, Value>) -> Result<Target> {
    match args.get("target") {
        None | Some(Value::Null) => Ok(Target::Working),
        Some(value) => serde_json::from_value(value.clone()).with_context(|| {
            format!(
                "{value} isn't a target (\"working\", \"staged\", \
                 {{\"branch\": {{\"branch\": \"feat\", \"base\": null}}}}, …)"
            )
        }),
    }
}

/// `open_review {path, target?}`.
pub fn open_review(
    shell: Entity<Shell>,
    args: Value,
    window: &mut Window,
    cx: &mut Context<DvAutomation>,
) -> Result<Task<Result<Value>>> {
    let args = object("open_review", args, &["path", "target"])?;
    let location = Location::parse(string("open_review", &args, "path")?)?;
    let target = target(&args)?;
    let opening = shell.update(cx, |shell, cx| {
        shell.open(location, OpenRequest::Target(target), window, cx)
    });
    Ok(cx.background_spawn(async move {
        let opened = opening.await.map_err(|failure| anyhow!("{failure}"))?;
        Ok(json!({"review_id": opened.review, "created": opened.created}))
    }))
}

/// `review_op {op, thread, body?, note?, author?}`.
pub fn review_op(
    shell: Entity<Shell>,
    args: Value,
    cx: &mut Context<DvAutomation>,
) -> Result<Task<Result<Value>>> {
    let args = object(
        "review_op",
        args,
        &["op", "thread", "body", "note", "author"],
    )?;
    let session = shell
        .read(cx)
        .session()
        .cloned()
        .context("review_op needs an open review (open_review first)")?;
    let (store, review) = {
        let session = session.read(cx);
        (session.store().clone(), session.review_id().clone())
    };
    let author = Author::agent(
        args.get("author")
            .and_then(Value::as_str)
            .unwrap_or("agent"),
    );
    let thread = || -> Result<ThreadId> {
        let id = string("review_op", &args, "thread")?;
        ThreadId::parse(id).map_err(|error| anyhow!("{error}"))
    };
    let op = string("review_op", &args, "op")?;
    let job: Box<dyn FnOnce() -> Result<Value> + Send> = match op {
        "reply" => {
            let (thread, body) = (thread()?, string("review_op", &args, "body")?.to_string());
            Box::new(move || {
                let id = ops::reply(&store, &review, &thread, &body, author)?;
                Ok(json!({"comment": id}))
            })
        }
        "address" => {
            let thread = thread()?;
            let note = args.get("note").and_then(Value::as_str).map(str::to_string);
            Box::new(move || {
                let changed = ops::set_thread_state(
                    &store,
                    &review,
                    &thread,
                    ThreadState::Addressed,
                    note.as_deref(),
                    author,
                )?;
                Ok(json!({"changed": changed}))
            })
        }
        "done" => Box::new(move || {
            let changed = ops::set_status(&store, &review, ReviewStatus::Ready, AuthorKind::Agent)?;
            Ok(json!({"changed": changed}))
        }),
        other => bail!("review_op's op is \"reply\", \"address\" or \"done\", not {other:?}"),
    };
    Ok(cx.background_spawn(async move { job() }))
}
