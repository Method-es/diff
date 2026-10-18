//! `dv setup [agents | path | all] [--yes]` (docs/PLAN.md §10.5, §12): the fixes `dv doctor`
//! lists, applied (`crate::checks`). `dv setup` alone is `dv setup all`; `dv setup wsl` is
//! [`super::setup_wsl`].
//!
//! - **`agents`** installs the `dv-review` skill for each coding agent set up in the home
//!   directory (Claude Code, Codex, Cursor: `checks::skills`), idempotently, repairing a copy
//!   that drifted. The hidden `--home <dir>` stands in for the home directory in tests.
//! - **`path [--remove]`** puts dv on `PATH` (`checks::path`): on Windows this executable's
//!   folder in the user `Path` (the hidden `--dir` names another folder: tests, and the
//!   installer naming the installed copy's), on Linux and in a distro `~/.local/bin/dv` and,
//!   when `~/.local/bin` isn't on the shell's `PATH`, a line in its startup file. `--remove`
//!   takes back exactly that.
//! - **`all`** applies every fix dv can apply, on this machine and in each running WSL distro
//!   (installing dv there first), and lists the ones it can't (`gh auth login`); a stopped
//!   distro is left alone. It exits 1 when a fix failed.
//!
//! Each writes outside the repository, so it asks first: without `--yes` a terminal is shown
//! the plan and asked, and anything else (a pipe, `--json`) is shown the plan and refused with
//! exit 2. Nothing to change needs no answer.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::PathBuf;

use crate::args::{GlobalArgs, SetupAgentsArgs, SetupPathArgs};
use crate::checks::{self, Environment, Kind, Seams, distro, path, skills};
use crate::context::Context;
use crate::dto::{AgentSetupDto, SetupAgentsDoc, SetupAllDoc, SetupPathDoc, SetupStepDto};
use crate::error::{CliError, Exit, code};
use crate::output;

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
    yes: bool,
    ctx: &mut Context,
) -> Result<Exit, CliError> {
    let mut seams = Seams::from_context(ctx);
    if let Some(home) = &args.home {
        seams.home = Some(ctx.cwd.join(home));
    }
    let home = seams.home().map_err(io_error)?.to_path_buf();
    let plans = plan(&home, &seams)?;
    if plans.iter().any(Plan::pending) && !(yes || args.yes.yes) {
        let lines: Vec<String> = plans.iter().map(Plan::planned).collect();
        ask(
            global,
            ctx,
            &lines,
            "dv setup agents writes the dv-review skill into your home directory",
            "Install the dv-review skill? [y/N] ",
            "nothing installed",
        )?;
    }
    for plan in plans.iter().filter(|plan| plan.pending()) {
        // Codex and Cursor share one file: the first install is the second's too.
        if skills::installed(&plan.path).map_err(io_error)? != skills::Installed::Same {
            skills::install(&plan.path).map_err(io_error)?;
        }
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

/// Each agent's plan (`checks::skills`).
fn plan(home: &std::path::Path, seams: &Seams) -> Result<Vec<Plan>, CliError> {
    skills::agents(home, seams.codex_home.as_deref())
        .into_iter()
        .map(|agent| {
            let action = if !agent.present() {
                Action::Skip(format!("no {} here", agent.home.display()))
            } else {
                match skills::installed(&agent.skill).map_err(io_error)? {
                    skills::Installed::Same => Action::Unchanged,
                    skills::Installed::Differs => Action::Update,
                    skills::Installed::Missing => Action::Install,
                }
            };
            Ok(Plan {
                agent: agent.id,
                path: agent.skill,
                action,
            })
        })
        .collect()
}

pub(crate) fn path(
    global: &GlobalArgs,
    args: &SetupPathArgs,
    yes: bool,
    ctx: &mut Context,
) -> Result<Exit, CliError> {
    let seams = Seams::from_context(ctx);
    let dir = args.dir.as_ref().map(|dir| ctx.cwd.join(dir));
    let (name, _) = checks::this_machine_name(&seams);
    let planned = path::planned(&seams, dir.as_deref(), args.remove).map_err(io_error)?;
    if let Some(step) = &planned
        && !(yes || args.yes.yes)
    {
        ask(
            global,
            ctx,
            &[format!("{name}  {step}")],
            if args.remove {
                "dv setup path --remove changes your PATH setup"
            } else {
                "dv setup path changes your PATH setup"
            },
            "Go ahead? [y/N] ",
            "nothing changed",
        )?;
    }
    let done = if args.remove {
        path::remove_dir(&seams, dir.as_deref())
    } else {
        path::add_dir(&seams, dir.as_deref())
    }
    .map_err(io_error)?;
    if global.json {
        output::json(
            ctx,
            &SetupPathDoc {
                dir: done.dir.display().to_string(),
                action: done.action,
                link: done.link.as_ref().map(|link| link.display().to_string()),
                rc_file: done.rc_file.as_ref().map(|rc| rc.display().to_string()),
            },
        )?;
    } else {
        output::text(ctx, &[format!("{name}  {}  {}", done.action, done.line())])?;
    }
    Ok(Exit::Ok)
}

/// A fix `dv setup all` applies, or can only name.
struct Step {
    environment: String,
    kind: Kind,
    check: String,
    /// The fix's button: dv applies it. `None`: only the command fixes it.
    action: Option<String>,
    command: Option<String>,
}

impl Step {
    fn planned(&self) -> String {
        match (&self.action, &self.command) {
            (Some(action), _) => format!("{}  {}  {action}", self.environment, self.check),
            (None, Some(command)) => {
                format!(
                    "{}  {}  run `{command}` yourself",
                    self.environment, self.check
                )
            }
            (None, None) => format!("{}  {}  fix it yourself", self.environment, self.check),
        }
    }
}

/// The fixes of `environments`' checks that need one, those dv applies first.
fn steps(environments: &[Environment]) -> Vec<Step> {
    let mut steps = Vec::new();
    for env in environments {
        for check in env.checks.iter().filter(|check| check.state.needs_fixing()) {
            let Some(fix) = &check.fix else {
                continue;
            };
            steps.push(Step {
                environment: env.name.clone(),
                kind: env.kind,
                check: check.id.clone(),
                action: fix.action.clone(),
                command: fix.command.clone(),
            });
        }
    }
    steps.sort_by_key(|step| step.action.is_none());
    steps
}

pub(crate) fn all(global: &GlobalArgs, yes: bool, ctx: &mut Context) -> Result<Exit, CliError> {
    let seams = Seams::from_context(ctx);
    let environments = checks::environments(&seams);
    let planned = steps(&environments);
    if planned.iter().any(|step| step.action.is_some()) && !yes {
        let mut lines: Vec<String> = planned.iter().map(Step::planned).collect();
        // A distro's own checks list once dv is there.
        lines.extend(
            planned
                .iter()
                .filter(|step| step.check == checks::id::DV)
                .map(|step| format!("{}  then what dv there finds to set up", step.environment)),
        );
        ask(
            global,
            ctx,
            &lines,
            "dv setup writes into your home directory, your PATH and your WSL distros",
            "Set these up? [y/N] ",
            "nothing changed",
        )?;
    }
    let mut done = Vec::new();
    apply_steps(&seams, &planned, &mut done);
    // A distro dv was just installed in has its own checks now: their fixes too.
    let installed: Vec<String> = done
        .iter()
        .filter(|step| step.check == checks::id::DV && step.outcome == "applied")
        .map(|step| step.environment.clone())
        .collect();
    for name in installed {
        let env = distro::environment(
            &seams,
            &dv_backend::wsl::Distro {
                name,
                running: true,
            },
        );
        let more: Vec<Step> = steps(std::slice::from_ref(&env))
            .into_iter()
            .filter(|step| step.check != checks::id::DV)
            .collect();
        apply_steps(&seams, &more, &mut done);
    }
    let failed = done.iter().any(|step| step.outcome == "failed");
    if global.json {
        output::json(ctx, &SetupAllDoc { steps: done })?;
    } else if done.is_empty() {
        output::text(ctx, &["nothing to set up".to_string()])?;
    } else {
        let lines: Vec<String> = done
            .iter()
            .map(|step| match step.outcome {
                "manual" => format!(
                    "{}  {}  run `{}` yourself",
                    step.environment,
                    step.check,
                    step.command.as_deref().unwrap_or("")
                ),
                outcome => format!(
                    "{}  {}  {outcome}  {}",
                    step.environment, step.check, step.detail
                ),
            })
            .collect();
        output::text(ctx, &lines)?;
    }
    Ok(if failed { Exit::Failure } else { Exit::Ok })
}

/// Applies each of `steps` that dv can, recording each in `done`, and records the rest as
/// manual.
fn apply_steps(seams: &Seams, steps: &[Step], done: &mut Vec<SetupStepDto>) {
    // In a distro one `dv setup agents` installs every agent's skill: once per distro.
    let mut skills_done: Vec<String> = Vec::new();
    for step in steps {
        let in_distro = step.kind == Kind::Wsl && seams.this_distro.is_none();
        let (outcome, detail) = if step.action.is_none() {
            ("manual", String::new())
        } else if in_distro
            && checks::id::is_skill(&step.check)
            && skills_done.contains(&step.environment)
        {
            ("applied", "with the other agents' skills".to_string())
        } else {
            let applied = if in_distro {
                if checks::id::is_skill(&step.check) {
                    skills_done.push(step.environment.clone());
                }
                distro::apply(seams, &step.environment, &step.check, &|_, _| {})
            } else {
                checks::apply_here(seams, &step.check)
            };
            match applied {
                Ok(line) => ("applied", line),
                Err(reason) => ("failed", reason),
            }
        };
        done.push(SetupStepDto {
            environment: step.environment.clone(),
            check: step.check.clone(),
            outcome,
            detail,
            command: step.command.clone(),
        });
    }
}

/// The ask-first rule (module docs): a terminal is shown `lines` and asked `question`, a "no"
/// being exit 1 with `cancelled`; anything else is shown them and refused with exit 2 (`why`,
/// then "pass --yes").
fn ask(
    global: &GlobalArgs,
    ctx: &mut Context,
    lines: &[String],
    why: &str,
    question: &str,
    cancelled: &str,
) -> Result<(), CliError> {
    if global.json || !ctx.stdin_is_terminal || !ctx.stdout_is_terminal {
        if !global.json {
            output::text(ctx, lines)?;
        }
        return Err(CliError::usage(format!("{why}: pass --yes to go ahead")));
    }
    output::text(ctx, lines)?;
    if !confirm(ctx, question)? {
        return Err(CliError::new(Exit::Failure, code::CANCELLED, cancelled));
    }
    Ok(())
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
