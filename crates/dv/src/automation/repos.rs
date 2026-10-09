//! Fixture repos for scenarios (PLAN §14.2): `seed_repo {name, steps, backend?}` builds one at
//! `<data_dir>/repos/<name>` with `dv_test_support::FixtureRepo::persistent`, and
//! `repo_edit {name, steps}` runs more steps on it, which is how a scenario plays the agent
//! editing and committing. `"backend": "loopback"` reaches it through the loopback
//! `dv backend --stdio` (`super::backends`); a `<name>.loopback` file beside it says so after a
//! restart. Steps are small JSON objects over the fixture's own methods:
//!
//! | Step | Does |
//! |---|---|
//! | `{"write": p, "text": t}` / `{"write": p, "bytes": [n, …]}` | writes a file; `"repeat": n` writes the text or bytes `n` times over (a large file in a short step) |
//! | `{"write_crlf": p, "text": t}` | writes a file with CRLF line endings |
//! | `{"remove": p}` | deletes a file or directory |
//! | `{"rename": [a, b]}` | moves a file |
//! | `{"commit": message}` | `git add -A` and commits (the fixture's identity and dates) |
//! | `{"branch": b}` | creates a branch at `HEAD` |
//! | `{"checkout": rev}` | checks out a branch or revision |
//! | `{"config": [key, value]}` | sets a key in the repo's `.git/config` |
//! | `{"attributes": text}` | replaces the root `.gitattributes` |
//! | `{"worktree": [name, branch]}` | adds a linked worktree on a new branch at `HEAD`, at `<data_dir>/repos/<repo>.fixture/worktrees/<name>` |
//! | `{"git": [args…]}` | any other git command, the escape hatch |
//!
//! The work runs off the UI thread. A fixture method panics on failure (it is a test helper);
//! the panic is caught and becomes the command's error.
//!
//! A seeded repo's linked worktrees, wherever they are (a `worktree` step, or `git worktree
//! add` through the escape hatch), get the fixture's environment too: every backend opened on
//! one, and every `cli` child started in one, runs git with it ([`roots`]).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, anyhow, bail};
use dv_test_support::FixtureRepo;
use dv_ui::ActiveServices as _;
use gpui::{AppContext as _, Context, Task};
use serde_json::{Map, Value, json};

use super::{DvAutomation, SeededBackend};

/// Where seeded repos go, inside the data dir.
const REPOS_DIR: &str = "repos";

/// What marks a seeded repo reached through the loopback, beside it: `<name>.loopback`.
const LOOPBACK_MARK: &str = "loopback";

/// One step of a `seed_repo` or `repo_edit`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Write { path: String, contents: Vec<u8> },
    WriteCrlf { path: String, text: String },
    Remove(String),
    Rename(String, String),
    Commit(String),
    Branch(String),
    Checkout(String),
    Config(String, String),
    Attributes(String),
    Worktree(String, String),
    Git(Vec<String>),
}

impl Step {
    /// Parses one step object; unknown keys are errors.
    pub fn parse(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .with_context(|| format!("a step is an object, not {value}"))?;
        let verbs = [
            "write",
            "write_crlf",
            "remove",
            "rename",
            "commit",
            "branch",
            "checkout",
            "config",
            "attributes",
            "worktree",
            "git",
        ];
        let named: Vec<&str> = verbs
            .iter()
            .copied()
            .filter(|verb| object.contains_key(*verb))
            .collect();
        let [verb] = named[..] else {
            bail!("a step has exactly one of {}: {value}", verbs.join(", "));
        };
        let extra: &[&str] = match verb {
            "write" => &["text", "bytes", "repeat"],
            "write_crlf" => &["text"],
            _ => &[],
        };
        if let Some(unknown) = object
            .keys()
            .find(|key| key.as_str() != verb && !extra.contains(&key.as_str()))
        {
            bail!("unknown key {unknown:?} in a {verb:?} step: {value}");
        }
        let text = |key: &str| -> Result<String> {
            object
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .with_context(|| format!("{verb:?} needs a string {key:?}: {value}"))
        };
        let strings = |count: Option<usize>| -> Result<Vec<String>> {
            let list = object[verb]
                .as_array()
                .with_context(|| format!("{verb:?} takes a list of strings: {value}"))?;
            let strings: Vec<String> = list
                .iter()
                .map(|item| item.as_str().map(str::to_string))
                .collect::<Option<_>>()
                .with_context(|| format!("{verb:?} takes a list of strings: {value}"))?;
            if count.is_some_and(|count| strings.len() != count) {
                bail!("{verb:?} takes {} strings: {value}", count.unwrap_or(0));
            }
            Ok(strings)
        };
        Ok(match verb {
            "write" => {
                let path = text("write")?;
                let contents = match (object.get("text"), object.get("bytes")) {
                    (Some(Value::String(text)), None) => text.clone().into_bytes(),
                    (None, Some(Value::Array(bytes))) => bytes
                        .iter()
                        .map(|byte| byte.as_u64().and_then(|byte| u8::try_from(byte).ok()))
                        .collect::<Option<_>>()
                        .with_context(|| format!("\"bytes\" are numbers 0-255: {value}"))?,
                    _ => bail!("\"write\" needs a string \"text\" or a list of \"bytes\": {value}"),
                };
                let repeat = match object.get("repeat") {
                    None => 1,
                    Some(count) => count
                        .as_u64()
                        .and_then(|count| usize::try_from(count).ok())
                        .filter(|count| (1..=1_000_000).contains(count))
                        .with_context(|| {
                            format!("\"repeat\" is a count from 1 to 1,000,000: {value}")
                        })?,
                };
                Self::Write {
                    path,
                    contents: contents.repeat(repeat),
                }
            }
            "write_crlf" => Self::WriteCrlf {
                path: text("write_crlf")?,
                text: text("text")?,
            },
            "remove" => Self::Remove(text("remove")?),
            "rename" => {
                let [from, to] = <[String; 2]>::try_from(strings(Some(2))?)
                    .map_err(|_| anyhow!("\"rename\" takes [from, to]"))?;
                Self::Rename(from, to)
            }
            "commit" => Self::Commit(text("commit")?),
            "branch" => Self::Branch(text("branch")?),
            "checkout" => Self::Checkout(text("checkout")?),
            "config" => {
                let [key, value] = <[String; 2]>::try_from(strings(Some(2))?)
                    .map_err(|_| anyhow!("\"config\" takes [key, value]"))?;
                Self::Config(key, value)
            }
            "attributes" => Self::Attributes(text("attributes")?),
            "worktree" => {
                let [name, branch] = <[String; 2]>::try_from(strings(Some(2))?)
                    .map_err(|_| anyhow!("\"worktree\" takes [name, branch]"))?;
                Self::Worktree(name, branch)
            }
            _ => {
                let args = strings(None)?;
                if args.is_empty() {
                    bail!("\"git\" needs at least one argument");
                }
                Self::Git(args)
            }
        })
    }

    /// Runs the step on `repo`; a commit's id comes back. Panics as the fixture does.
    fn apply(&self, repo: &FixtureRepo) -> Option<String> {
        match self {
            Self::Write { path, contents } => {
                repo.write(path, contents);
            }
            Self::WriteCrlf { path, text } => {
                repo.write_crlf(path, text);
            }
            Self::Remove(path) => {
                repo.remove(path);
            }
            Self::Rename(from, to) => {
                repo.rename(from, to);
            }
            Self::Commit(message) => return Some(repo.commit(message)),
            Self::Branch(name) => {
                repo.branch(name);
            }
            Self::Checkout(rev) => {
                repo.checkout(rev);
            }
            Self::Config(key, value) => {
                repo.config(key, value);
            }
            Self::Attributes(text) => {
                repo.set_attributes(text);
            }
            Self::Worktree(name, branch) => {
                repo.add_worktree(name, branch);
            }
            Self::Git(args) => {
                repo.git(args);
            }
        }
        None
    }
}

/// Parses a command's `steps` list.
pub fn parse_steps(args: &Map<String, Value>) -> Result<Vec<Step>> {
    match args.get("steps") {
        None => Ok(Vec::new()),
        Some(Value::Array(steps)) => steps.iter().map(Step::parse).collect(),
        Some(other) => bail!("\"steps\" is a list, not {other}"),
    }
}

/// A seeded repo's name: a plain file name, so it can't leave `<data_dir>/repos`.
fn check_name(name: &str) -> Result<()> {
    let plain = !name.is_empty()
        && !name.starts_with('.')
        && !name.ends_with(".fixture")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if plain {
        Ok(())
    } else {
        bail!("a repo name is a plain file name (letters, digits, '-', '_', '.'), not {name:?}")
    }
}

/// Runs `steps` on `repo`, turning a fixture panic into an error; returns the commits made.
fn run_steps(repo: &FixtureRepo, steps: &[Step]) -> Result<Vec<String>> {
    catch_unwind(AssertUnwindSafe(|| {
        steps.iter().filter_map(|step| step.apply(repo)).collect()
    }))
    .map_err(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|text| text.to_string()))
            .unwrap_or_else(|| "the fixture panicked".to_string());
        anyhow!("{message}")
    })
}

/// The roots a seeded repo's environment applies to: its work tree and each of its linked
/// worktrees (`git worktree list`, read with the fixture's environment; blocking). A list git
/// can't give leaves the work tree alone.
pub fn roots(fixture: &FixtureRepo) -> Vec<PathBuf> {
    let mut roots = vec![fixture.path().to_path_buf()];
    let listed = fixture
        .git_command()
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()
        .filter(|output| output.status.success());
    if let Some(output) = listed {
        roots.extend(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.strip_prefix("worktree "))
                .map(PathBuf::from),
        );
    }
    roots
}

/// The seeded repos already in `data_dir` (an earlier launch of the same scenario), by name,
/// with how each is reached.
pub fn reopen_seeded(data_dir: &Path) -> Vec<(String, FixtureRepo, SeededBackend)> {
    let dir = data_dir.join(REPOS_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if check_name(&name).is_err()
            || !path.join(".git").is_dir()
            || !dir.join(format!("{name}.fixture")).is_dir()
        {
            continue;
        }
        let backend = if dir.join(format!("{name}.{LOOPBACK_MARK}")).is_file() {
            SeededBackend::Loopback
        } else {
            SeededBackend::Local
        };
        match catch_unwind(|| FixtureRepo::reopen(&path)) {
            Ok(fixture) => found.push((name, fixture, backend)),
            Err(_) => log::warn!("couldn't reopen the seeded repo {}", path.display()),
        }
    }
    found
}

impl DvAutomation {
    /// `seed_repo {name, steps, backend?}`: builds `<data_dir>/repos/<name>` and runs `steps`
    /// on it; replies `{path, commits}` once it is registered with `Locations`, reached in
    /// process or, with `"backend": "loopback"`, through the loopback server.
    pub(super) fn seed_repo(
        &mut self,
        args: Value,
        cx: &mut Context<Self>,
    ) -> Result<Task<Result<Value>>> {
        let args = args.as_object().cloned().unwrap_or_default();
        if let Some(unknown) = args
            .keys()
            .find(|key| !["name", "steps", "backend"].contains(&key.as_str()))
        {
            bail!("seed_repo takes name, steps and backend, not {unknown:?}");
        }
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .context("seed_repo needs a \"name\"")?
            .to_string();
        check_name(&name)?;
        let steps = parse_steps(&args)?;
        let backend = match args.get("backend") {
            None | Some(Value::Null) => SeededBackend::Local,
            Some(Value::String(kind)) if kind == "local" => SeededBackend::Local,
            Some(Value::String(kind)) if kind == "loopback" => SeededBackend::Loopback,
            Some(other) => bail!("seed_repo's backend is \"local\" or \"loopback\", not {other}"),
        };
        let repos = cx.services().data_dir().join(REPOS_DIR);
        let path: PathBuf = repos.join(&name);
        if self.seeded.contains_key(&name) || path.exists() {
            bail!("a repo named {name:?} is already seeded");
        }
        let mark = repos.join(format!("{name}.{LOOPBACK_MARK}"));
        let job = cx.background_spawn(async move {
            let fixture = catch_unwind(|| FixtureRepo::persistent(&path))
                .map_err(|_| anyhow!("couldn't build a repo at {}", path.display()))?;
            let commits = run_steps(&fixture, &steps)?;
            let roots = roots(&fixture);
            if backend == SeededBackend::Loopback {
                std::fs::write(&mark, "")
                    .with_context(|| format!("couldn't write {}", mark.display()))?;
            }
            anyhow::Ok((fixture, commits, roots))
        });
        Ok(cx.spawn(async move |this, cx| {
            let (fixture, commits, roots) = job.await?;
            let path = fixture.path().display().to_string();
            this.update(cx, |this, cx| {
                this.register(name, fixture, backend, roots, cx)
            })?;
            Ok(json!({"path": path, "commits": commits}))
        }))
    }

    /// `repo_edit {name, steps}`: runs `steps` on a seeded repo; replies `{path, commits}`.
    pub(super) fn repo_edit(
        &mut self,
        args: Value,
        cx: &mut Context<Self>,
    ) -> Result<Task<Result<Value>>> {
        let args = args.as_object().cloned().unwrap_or_default();
        if let Some(unknown) = args
            .keys()
            .find(|key| !["name", "steps"].contains(&key.as_str()))
        {
            bail!("repo_edit takes name and steps, not {unknown:?}");
        }
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .context("repo_edit needs a \"name\"")?;
        let steps = parse_steps(&args)?;
        let (fixture, backend): (Arc<Mutex<FixtureRepo>>, SeededBackend) = self
            .seeded
            .get(name)
            .cloned()
            .with_context(|| format!("no repo named {name:?} was seeded"))?;
        let job = cx.background_spawn(async move {
            let fixture = fixture
                .lock()
                .map_err(|_| anyhow!("an earlier step on this repo panicked"))?;
            let commits = run_steps(&fixture, &steps)?;
            let env = fixture.env_overlay();
            anyhow::Ok((fixture.path().to_path_buf(), commits, roots(&fixture), env))
        });
        Ok(cx.spawn(async move |this, cx| {
            let (path, commits, roots, env) = job.await?;
            // A worktree the steps added gets the fixture's environment from now on.
            this.update(cx, |this, cx| this.register_roots(roots, env, backend, cx))?;
            Ok(json!({"path": path.display().to_string(), "commits": commits}))
        }))
    }
}
