//! Each command's work (docs/PLAN.md §10.1): [`run`] dispatches.
//!
//! - Reading: [`status`], [`review`] (`list`, `show`) and [`comment`] (`list`, `show`).
//! - Writing: [`review_write`] (`new`, `done`, `submit`, `close`, `delete`) and
//!   [`comment_write`] (`add`, `reply`, `address`, `resolve`, `reopen`).
//! - Waiting for the reviewer: [`wait`]; installing the agent skill: [`setup`]; installing dv in
//!   a WSL distro: [`setup_wsl`].
//! - Serving the app's remote backend over stdio: [`backend`].
//!
//! Commands of later versions are recognised and refused with what to do instead ([`later`]).
//! On Windows, a command for a repository inside a WSL distro runs there instead
//! (`crate::reexec`, PLAN §10.4).

mod backend;
mod comment;
pub(crate) mod comment_write;
mod format;
mod later;
mod review;
pub(crate) mod review_write;
pub(crate) mod setup;
mod setup_wsl;
mod status;
pub(crate) mod wait;

use dv_core::review::ThreadState;

use crate::args::{Command, CommentCommand, GlobalArgs, ReviewCommand, SetupCommand};
use crate::context::Context;
use crate::error::{CliError, Exit};
use crate::reexec;

/// Runs `command`, printing its output; an error is returned for the caller to report. A
/// command for a WSL repository runs inside its distro when this is Windows (PLAN §10.4).
pub(crate) fn run(
    global: &GlobalArgs,
    command: Command,
    ctx: &mut Context,
) -> Result<Exit, CliError> {
    if let Some((distro, path)) = reexec::wsl_repository(global, &command, ctx)? {
        return reexec::run(&distro, &path, ctx);
    }
    match command {
        Command::Status => status::status(global, ctx),
        Command::Review(command) => match command {
            ReviewCommand::List(args) => review::list(global, &args, ctx),
            ReviewCommand::Show(args) => review::show(global, args.id.as_deref(), ctx),
            ReviewCommand::New(args) if args.pr.is_some() => Err(later::github_prs()),
            ReviewCommand::New(args) => review_write::new(global, &args, ctx),
            ReviewCommand::Wait(args) => wait::wait(global, &args, ctx),
            ReviewCommand::Done(args) => review_write::done(global, &args, ctx),
            ReviewCommand::Submit(args) => review_write::submit(global, &args, ctx),
            ReviewCommand::Close(args) => review_write::close(global, &args.id, ctx),
            ReviewCommand::Delete(args) => review_write::delete(global, &args.id, ctx),
        },
        Command::Comment(command) => match command {
            CommentCommand::List(args) => comment::list(global, &args, ctx),
            CommentCommand::Show(args) => comment::show(global, &args.id, ctx),
            CommentCommand::Add(args) => comment_write::add(global, &args, ctx),
            CommentCommand::Reply(args) => comment_write::reply(global, &args, ctx),
            CommentCommand::Address(args) => comment_write::set_state(
                global,
                &args.thread.id,
                ThreadState::Addressed,
                Some(&args.note),
                ctx,
            ),
            CommentCommand::Resolve(args) => {
                comment_write::set_state(global, &args.id, ThreadState::Resolved, None, ctx)
            }
            CommentCommand::Reopen(args) => {
                comment_write::set_state(global, &args.id, ThreadState::Open, None, ctx)
            }
        },
        Command::Setup(args) => match &args.what {
            Some(SetupCommand::Agents(agents)) => setup::agents(global, agents, ctx),
            Some(SetupCommand::Path(_)) => Err(later::setup_path()),
            Some(SetupCommand::Wsl(wsl)) => setup_wsl::wsl(global, wsl, ctx),
            Some(SetupCommand::All(_)) => Err(later::setup_all()),
            None => Err(later::setup_what()),
        },
        Command::Doctor => Err(later::doctor()),
        Command::Backend(args) => backend::backend(&args, ctx),
    }
}
