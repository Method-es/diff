//! `dv setup agents [--yes]` (docs/PLAN.md §10.5, §12): installs the `dv-review` skill for
//! each coding agent set up in the home directory, idempotently, repairing a copy that drifted.
//!
//! - **Claude Code** (its home `~/.claude` exists): `~/.claude/skills/dv-review/SKILL.md`.
//! - **Codex** (its home `$CODEX_HOME`, else `~/.codex`, exists): `~/.agents/skills/dv-review/
//!   SKILL.md`, the user-level skills folder Codex reads (its documentation, checked
//!   2026-10-09; `$CODEX_HOME` holds Codex's own state, not the skills a user adds).
//!
//! It writes to the home directory, so it asks first: without `--yes` a terminal is shown the
//! plan and asked, and anything else (a pipe, `--json`) is shown the plan and refused with exit
//! 2. Nothing to change needs no answer. The hidden `--home <dir>` stands in for the home
//! directory in tests, which never touch the real one.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};

use crate::args::{GlobalArgs, SetupAgentsArgs};
use crate::context::Context;
use crate::dto::{AgentSetupDto, SetupAgentsDoc};
use crate::error::{CliError, Exit, code};
use crate::output;
use crate::skill::{SKILL_MD, SKILL_NAME};

/// What installing the skill does for one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Install,
    Update,
    Unchanged,
    /// The agent isn't set up here; the reason.
    Skip(String),
}

/// One agent's line of the plan.
#[derive(Debug, Clone)]
struct Plan {
    agent: &'static str,
    path: PathBuf,
    action: Action,
}

impl Plan {
    fn pending(&self) -> bool {
        matches!(self.action, Action::Install | Action::Update)
    }

    /// The line the plan shows before anything is written.
    fn planned(&self) -> String {
        let verb = match &self.action {
            Action::Install => "install",
            Action::Update => "update (it differs)",
            Action::Unchanged => "up to date",
            Action::Skip(_) => "skip",
        };
        self.line(verb)
    }

    fn line(&self, verb: &str) -> String {
        match &self.action {
            Action::Skip(reason) => format!("{}  {verb}  ({reason})", self.agent),
            _ => format!("{}  {verb}  {}", self.agent, self.path.display()),
        }
    }

    fn dto(&self) -> AgentSetupDto {
        let (action, reason) = match &self.action {
            Action::Install => ("installed", None),
            Action::Update => ("updated", None),
            Action::Unchanged => ("unchanged", None),
            Action::Skip(reason) => ("skipped", Some(reason.clone())),
        };
        AgentSetupDto {
            agent: self.agent,
            path: self.path.display().to_string(),
            action,
            reason,
        }
    }
}

pub(crate) fn agents(
    global: &GlobalArgs,
    args: &SetupAgentsArgs,
    ctx: &mut Context,
) -> Result<Exit, CliError> {
    let home = match &args.home {
        Some(home) => ctx.cwd.join(home),
        None => home_dir(ctx)?,
    };
    let plans = plan(&home, ctx)?;
    if plans.iter().any(Plan::pending) && !args.yes.yes {
        let lines: Vec<String> = plans.iter().map(Plan::planned).collect();
        if global.json || !ctx.stdin_is_terminal || !ctx.stdout_is_terminal {
            if !global.json {
                output::text(ctx, &lines)?;
            }
            return Err(CliError::usage(
                "dv setup agents writes the dv-review skill into your home directory: pass \
                 --yes to go ahead",
            ));
        }
        output::text(ctx, &lines)?;
        if !confirm(ctx, "Install the dv-review skill? [y/N] ")? {
            return Err(CliError::new(
                Exit::Failure,
                code::CANCELLED,
                "nothing installed",
            ));
        }
    }
    for plan in plans.iter().filter(|plan| plan.pending()) {
        install(&plan.path)?;
    }
    let doc = SetupAgentsDoc {
        agents: plans.iter().map(Plan::dto).collect(),
    };
    if global.json {
        output::json(ctx, &doc)?;
    } else {
        let lines: Vec<String> = plans
            .iter()
            .zip(&doc.agents)
            .map(|(plan, dto)| plan.line(dto.action))
            .collect();
        output::text(ctx, &lines)?;
    }
    Ok(Exit::Ok)
}

/// The home directory: `%USERPROFILE%` on Windows, `$HOME` elsewhere.
fn home_dir(ctx: &Context) -> Result<PathBuf, CliError> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    ctx.env.non_empty(name).map(PathBuf::from).ok_or_else(|| {
        CliError::new(
            Exit::Failure,
            code::IO,
            format!("can't tell the home directory: ${name} isn't set"),
        )
    })
}

/// Each agent's plan (module docs).
fn plan(home: &Path, ctx: &Context) -> Result<Vec<Plan>, CliError> {
    let codex_home = match ctx.env.non_empty("CODEX_HOME") {
        Some(dir) => ctx.cwd.join(dir),
        None => home.join(".codex"),
    };
    let agents = [
        (
            "claude-code",
            home.join(".claude"),
            home.join(".claude").join("skills"),
        ),
        ("codex", codex_home, home.join(".agents").join("skills")),
    ];
    agents
        .into_iter()
        .map(|(agent, agent_home, skills)| {
            let path = skills.join(SKILL_NAME).join("SKILL.md");
            let action = if !agent_home.is_dir() {
                Action::Skip(format!("no {} here", agent_home.display()))
            } else {
                match std::fs::read(&path) {
                    Ok(bytes) if bytes == SKILL_MD.as_bytes() => Action::Unchanged,
                    Ok(_) => Action::Update,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Action::Install,
                    Err(error) => {
                        return Err(io_error(format!("reading {}: {error}", path.display())));
                    }
                }
            };
            Ok(Plan {
                agent,
                path,
                action,
            })
        })
        .collect()
}

/// Writes the skill at `path`: into a temporary file beside it, then renamed over it, so an
/// agent never reads half a skill.
fn install(path: &Path) -> Result<(), CliError> {
    let dir = path
        .parent()
        .ok_or_else(|| io_error(format!("{} has no folder", path.display())))?;
    std::fs::create_dir_all(dir)
        .map_err(|error| io_error(format!("creating {}: {error}", dir.display())))?;
    let temp = dir.join(format!(".SKILL.md.{}.tmp", std::process::id()));
    std::fs::write(&temp, SKILL_MD)
        .and_then(|()| std::fs::rename(&temp, path))
        .map_err(|error| {
            let _ = std::fs::remove_file(&temp);
            io_error(format!("writing {}: {error}", path.display()))
        })
}

/// Asks `question` on the terminal; `true` for an answer starting with `y`.
pub(crate) fn confirm(ctx: &mut Context, question: &str) -> Result<bool, CliError> {
    ctx.stdout
        .write_all(question.as_bytes())
        .and_then(|()| ctx.stdout.flush())
        .map_err(|error| io_error(format!("writing the question: {error}")))?;
    let mut answer = String::new();
    BufReader::new(&mut ctx.stdin)
        .read_line(&mut answer)
        .map_err(|error| io_error(format!("reading the answer: {error}")))?;
    Ok(answer.trim().to_ascii_lowercase().starts_with('y'))
}

pub(crate) fn io_error(message: String) -> CliError {
    CliError::new(Exit::Failure, code::IO, message)
}
