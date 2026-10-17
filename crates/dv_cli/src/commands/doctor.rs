//! `dv doctor [--json]` (docs/PLAN.md §12): every environment's checks (`crate::checks`): this
//! machine's and, on Windows, each WSL distro's, a running one's read by its own Linux dv and a
//! stopped one listed as stopped, never booted. Exit 0 when no check needs fixing, else 1;
//! nothing is changed.
//!
//! The text is one block per environment: its name (`Ubuntu (WSL)`, `Debian (WSL, stopped)`),
//! then one line per check, `state  title  detail`, and under a check that needs fixing the
//! command that fixes it, `  fix: dv setup agents`.

use crate::args::GlobalArgs;
use crate::checks::{self, Environment, Kind, Seams};
use crate::context::Context;
use crate::dto::DoctorDoc;
use crate::error::{CliError, Exit};
use crate::output;

pub(crate) fn doctor(global: &GlobalArgs, ctx: &mut Context) -> Result<Exit, CliError> {
    let seams = Seams::from_context(ctx);
    let environments = checks::environments(&seams);
    let ok = !environments.iter().any(Environment::needs_fixing);
    if global.json {
        output::json(ctx, &DoctorDoc { ok, environments })?;
    } else {
        output::text(ctx, &lines(&environments))?;
    }
    Ok(if ok { Exit::Ok } else { Exit::Failure })
}

/// The text of `environments` (module docs).
pub(crate) fn lines(environments: &[Environment]) -> Vec<String> {
    let mut lines = Vec::new();
    for (index, env) in environments.iter().enumerate() {
        if index > 0 {
            lines.push(String::new());
        }
        lines.push(heading(env));
        if let Some(note) = &env.note {
            lines.push(format!("  {note}"));
        }
        for check in &env.checks {
            lines.push(format!(
                "  {}  {}  {}",
                state_word(check.state),
                check.title,
                check.detail
            ));
            if check.state.needs_fixing()
                && let Some(command) = check.fix.as_ref().and_then(|fix| fix.command.as_ref())
            {
                lines.push(format!("    fix: {command}"));
            }
        }
    }
    lines
}

fn heading(env: &Environment) -> String {
    match (env.kind, env.running) {
        (Kind::Wsl, true) => format!("{} (WSL)", env.name),
        (Kind::Wsl, false) => format!("{} (WSL, stopped)", env.name),
        _ => env.name.clone(),
    }
}

/// A state as the text names it.
fn state_word(state: checks::State) -> &'static str {
    match state {
        checks::State::Ok => "ok",
        checks::State::Missing => "missing",
        checks::State::OutOfDate => "out of date",
        checks::State::Failed => "failed",
        checks::State::Skipped => "skipped",
    }
}
