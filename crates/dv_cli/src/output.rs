//! Printing (docs/PLAN.md §10.3): with `--json` one JSON document on stdout ([`json`]), else
//! short stable lines without colour ([`text`]); errors always on stderr, and with `--json` also
//! as `{"schema": 1, "error": {…}}` on stdout ([`error`]), clap's own parse errors included
//! ([`parse_error`]).

use std::ffi::OsString;
use std::io::Write as _;

use serde::Serialize;

use crate::context::Context;
use crate::dto::{Document, ErrorDoc, ErrorDto};
use crate::error::{CliError, Exit, code};

/// Prints `body` as one compact JSON document, `"schema": 1` first, and a newline.
pub(crate) fn json<T: Serialize>(ctx: &mut Context, body: &T) -> Result<(), CliError> {
    let text = serde_json::to_string(&Document::new(body)).map_err(|error| {
        CliError::new(
            Exit::Failure,
            code::INTERNAL,
            format!("encoding JSON: {error}"),
        )
    })?;
    write_out(ctx, &format!("{text}\n"))
}

/// Prints `lines` on stdout, each ending in a newline.
pub(crate) fn text(ctx: &mut Context, lines: &[String]) -> Result<(), CliError> {
    let mut out = String::new();
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    write_out(ctx, &out)
}

fn write_out(ctx: &mut Context, text: &str) -> Result<(), CliError> {
    ctx.stdout
        .write_all(text.as_bytes())
        .and_then(|()| ctx.stdout.flush())
        .map_err(|error| {
            CliError::new(
                Exit::Failure,
                code::IO,
                format!("writing the output: {error}"),
            )
        })
}

/// Reports `error`: `dv: <message>` on stderr, then its candidates (`id  label  status  round
/// n  worktree`, PLAN §10.2: the worktree tells apart two worktrees' reviews whose labels read
/// the same) and its detail; with `json` also the error document on stdout. Returns its exit
/// code. Printing is best effort: a closed stream can't report anything anyway.
pub fn error(ctx: &mut Context, json: bool, error: &CliError) -> Exit {
    let mut lines = vec![format!("dv: {}", error.message)];
    if let Some(candidates) = &error.candidates {
        lines.extend(candidates.iter().map(|candidate| {
            format!(
                "  {}  {}  {}  round {}  {}",
                candidate.id,
                candidate.label,
                candidate.status,
                candidate.round,
                candidate.worktree
            )
        }));
    }
    if let Some(detail) = &error.detail {
        lines.extend(detail.lines().map(|line| format!("  {line}")));
    }
    let mut text = lines.join("\n");
    text.push('\n');
    let _ = ctx.stderr.write_all(text.as_bytes());
    let _ = ctx.stderr.flush();
    if json {
        let doc = ErrorDoc {
            error: ErrorDto {
                code: error.code,
                message: error.message.clone(),
                detail: error.detail.clone(),
                candidates: error.candidates.clone(),
            },
        };
        let _ = self::json(ctx, &doc);
    }
    error.exit
}

/// Reports a command line clap couldn't parse, given the arguments it was parsing (the program
/// name first). `--help` and `--version` print on stdout and exit 0; a real error prints on
/// stderr and exits 2, and when `--json` is anywhere on the line it is also the JSON error
/// document (`usage`) on stdout, so an agent that asked for JSON always gets JSON.
pub fn parse_error(error: &clap::Error, args: &[OsString], ctx: &mut Context) -> Exit {
    let rendered = error.render().to_string();
    if error.exit_code() == 0 {
        let _ = ctx.stdout.write_all(rendered.as_bytes());
        let _ = ctx.stdout.flush();
        return Exit::Ok;
    }
    let _ = ctx.stderr.write_all(rendered.as_bytes());
    let _ = ctx.stderr.flush();
    if asks_for_json(args) {
        let message = match error.kind() {
            // A command group named alone (`dv --json review`): clap shows the group's help,
            // whose first line is its description, not what is wrong.
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                missing_subcommand(&rendered)
            }
            _ => headline(&rendered),
        };
        let doc = ErrorDoc {
            error: ErrorDto {
                code: code::USAGE,
                message,
                detail: None,
                candidates: None,
            },
        };
        let _ = json(ctx, &doc);
    }
    Exit::Usage
}

/// Whether a line clap couldn't parse asked for JSON: `--json` (or a `--json=…` spelling clap
/// refuses for a flag) anywhere before a bare `--`, after which every argument is a value. A
/// `--` right after `--body` or `--note` is that option's value (they take text that starts
/// with '-', and refuse a lone `--`), not the end of the options.
fn asks_for_json(args: &[OsString]) -> bool {
    let mut previous: Option<&str> = None;
    for arg in args.iter().skip(1) {
        let text = arg.to_str();
        if text == Some("--") && !matches!(previous, Some("--body" | "--note")) {
            return false;
        }
        if text.is_some_and(|text| text == "--json" || text.starts_with("--json=")) {
            return true;
        }
        previous = text;
    }
    false
}

/// What a command group named without one of its commands lacks, in clap's own words for
/// the same mistake made otherwise (`dv review --json`): "'dv review' requires a subcommand but
/// one was not provided". The group is read from the help's usage line (`Usage: dv review
/// [OPTIONS] <COMMAND>`).
fn missing_subcommand(rendered: &str) -> String {
    let usage = rendered
        .lines()
        .find_map(|line| line.trim().strip_prefix("Usage: "))
        .unwrap_or("dv");
    let command: Vec<&str> = usage
        .split_whitespace()
        .take_while(|word| !word.starts_with(['[', '<']))
        .collect();
    format!(
        "'{}' requires a subcommand but one was not provided",
        command.join(" ")
    )
}

/// The first line of clap's message, without its `error: ` prefix. A line that ends in `:`
/// introduces the indented lines under it (the required arguments that are missing), which
/// follow it, so the message alone says what to fix.
fn headline(rendered: &str) -> String {
    let mut lines = rendered.lines().skip_while(|line| line.trim().is_empty());
    let first = lines
        .next()
        .map(str::trim)
        .unwrap_or("invalid command line");
    let first = first.strip_prefix("error: ").unwrap_or(first);
    if !first.ends_with(':') {
        return first.to_string();
    }
    let listed: Vec<&str> = lines
        .take_while(|line| line.starts_with([' ', '\t']) && !line.trim().is_empty())
        .map(str::trim)
        .collect();
    if listed.is_empty() {
        first.to_string()
    } else {
        format!("{first} {}", listed.join(", "))
    }
}
