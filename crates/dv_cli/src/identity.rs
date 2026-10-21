//! Who a command acts as (docs/PLAN.md §2.5, §10): an agent named by `--author <name>`, else
//! by `$DV_AUTHOR`, else `claude-code` when `CLAUDECODE` is set (Claude Code exports it to the
//! commands it runs); otherwise the reviewer, by the repository's `user.name`, as the GUI
//! writes (`dv_core::review::reviewer_identity`). What each may do is `review::ops`'s rule.

use dv_core::backend::Backend;
use dv_core::review::{Author, reviewer_identity};

use crate::args::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;

/// The agent name Claude Code's commands act as by default.
pub const CLAUDE_CODE: &str = "claude-code";

/// The agent the flags and the environment name, if any: `--author`, `$DV_AUTHOR`, then
/// `CLAUDECODE`. An empty `--author` is a usage error; an empty `$DV_AUTHOR` counts as unset.
pub(crate) fn agent(global: &GlobalArgs, ctx: &Context) -> Result<Option<Author>, CliError> {
    if let Some(name) = &global.author {
        return agent_named(name, "--author").map(Some);
    }
    if let Some(name) = ctx.env.non_empty("DV_AUTHOR") {
        return agent_named(&name, "$DV_AUTHOR").map(Some);
    }
    if ctx.env.non_empty("CLAUDECODE").is_some() {
        return Ok(Some(Author::agent(CLAUDE_CODE)));
    }
    Ok(None)
}

/// Who the command acts as: [`agent`], else the reviewer (one `git config` call).
pub(crate) fn author(
    global: &GlobalArgs,
    ctx: &Context,
    backend: &dyn Backend,
) -> Result<Author, CliError> {
    Ok(match agent(global, ctx)? {
        Some(agent) => agent,
        None => reviewer_identity(backend),
    })
}

/// The agent a command that only agents run acts as (`command`: `dv review done`, `dv comment
/// annotate`); without an agent identity it is a usage error (exit 2) that says how to give
/// one: "`command` is an agent's `role`: pass --author …; `otherwise`", what the reviewer runs
/// instead.
pub(crate) fn require_agent(
    global: &GlobalArgs,
    ctx: &Context,
    command: &str,
    role: &str,
    otherwise: &str,
) -> Result<Author, CliError> {
    agent(global, ctx)?.ok_or_else(|| {
        CliError::usage(format!(
            "`{command}` is an agent's {role}: pass --author <agent name> (claude-code, codex, \
             …); {otherwise}"
        ))
    })
}

fn agent_named(name: &str, source: &str) -> Result<Author, CliError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CliError::usage(format!("{source} needs an agent name")));
    }
    if name.chars().any(char::is_control) {
        return Err(CliError::usage(format!(
            "{source}: an agent name is one line of text"
        )));
    }
    Ok(Author::agent(name))
}
