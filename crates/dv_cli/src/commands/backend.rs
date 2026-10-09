//! `dv backend --stdio` (docs/PLAN.md D6, §5.1): the server the app talks to, serving
//! repositories through `LocalBackend`s over the command's stdin and stdout until stdin ends
//! (`dv_backend::server`). The app starts it: inside a WSL distro, one per distro; on any OS as
//! a loopback for tests. Every build has it.

use std::io;

use crate::args::BackendArgs;
use crate::context::Context;
use crate::error::{CliError, Exit, code};

/// Serves the protocol on the context's stdin and stdout. It returns once stdin ends, whatever
/// requests are still running: the `dv` binary exits right after, which ends them with it.
pub(crate) fn backend(args: &BackendArgs, ctx: &mut Context) -> Result<Exit, CliError> {
    if !args.stdio {
        return Err(CliError::usage(
            "`dv backend` serves the dv app over stdin and stdout; the app starts it as \
             `dv backend --stdio`",
        ));
    }
    let input = std::mem::replace(&mut ctx.stdin, Box::new(io::empty()));
    let output = std::mem::replace(&mut ctx.stdout, Box::new(io::sink()));
    dv_backend::server::serve(input, output).map_err(|error| {
        CliError::new(
            Exit::Failure,
            code::IO,
            format!("reading the backend's requests failed: {error}"),
        )
    })?;
    Ok(Exit::Ok)
}
