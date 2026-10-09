//! A command's text (docs/PLAN.md §10.1): `--body <text>`, or `--body-file <path>` / `-` for
//! stdin, which is safe for any quoting and what the agent skill uses (PLAN §10.5); a note the
//! same way, `--note <text>` or `--note-file <path>` / `-`.
//!
//! The text is Markdown, kept as written except for what a pipe or an editor adds around it: a
//! leading UTF-8 byte order mark (PowerShell writes one), `\r\n` line endings, and trailing
//! whitespace (the heredoc's final newline).

use std::io::Read as _;
use std::path::Path;

use crate::args::{Body, Note, OptionalBody};
use crate::context::Context;
use crate::error::{CliError, Exit, code};

/// The text a required `--body` / `--body-file` gives; empty text is a usage error naming
/// `what` ("comment", "reply").
pub(crate) fn required(body: &Body, what: &str, ctx: &mut Context) -> Result<String, CliError> {
    let text = read(body.body.as_deref(), body.body_file.as_deref(), ctx)?.unwrap_or_default();
    if text.trim().is_empty() {
        return Err(CliError::usage(format!("the {what} is empty")));
    }
    Ok(text)
}

/// The text an optional `--body` / `--body-file` gives; `None` when neither is given or the
/// text is empty.
pub(crate) fn optional(body: &OptionalBody, ctx: &mut Context) -> Result<Option<String>, CliError> {
    Ok(read(body.body.as_deref(), body.body_file.as_deref(), ctx)?
        .filter(|text| !text.trim().is_empty()))
}

fn read(
    body: Option<&str>,
    file: Option<&Path>,
    ctx: &mut Context,
) -> Result<Option<String>, CliError> {
    let raw = match (body, file) {
        (Some(body), _) => body.to_string(),
        (None, Some(file)) => {
            let bytes = bytes(file, ctx)?;
            decode(bytes, &source(file, ctx))?
        }
        (None, None) => return Ok(None),
    };
    Ok(Some(normalise(&raw)))
}

/// What a `--body-file` or `--note-file` holds, as read: stdin for `-`, else the file, relative
/// to the working directory.
pub(crate) fn bytes(file: &Path, ctx: &mut Context) -> Result<Vec<u8>, CliError> {
    if file == Path::new("-") {
        let mut bytes = Vec::new();
        ctx.stdin
            .read_to_end(&mut bytes)
            .map_err(|error| io_error(format!("reading the text from stdin: {error}")))?;
        return Ok(bytes);
    }
    let path = ctx.cwd.join(file);
    std::fs::read(&path).map_err(|error| io_error(format!("reading {}: {error}", path.display())))
}

/// What a `--body-file` or `--note-file` holds, read here for a command that runs inside a WSL
/// distro (PLAN §10.4), whose dv gets it on stdin: a named file is checked for UTF-8 here, so
/// the error names that file, not the distro's stdin; stdin's text passes as read, the distro's
/// dv checking it as its own.
pub(crate) fn forwarded(file: &Path, ctx: &mut Context) -> Result<Vec<u8>, CliError> {
    let bytes = bytes(file, ctx)?;
    if file != Path::new("-") && std::str::from_utf8(&bytes).is_err() {
        return Err(not_utf8(&source(file, ctx)));
    }
    Ok(bytes)
}

/// How errors name where the text came from: stdin, or the file's full path.
fn source(file: &Path, ctx: &Context) -> String {
    if file == Path::new("-") {
        "stdin".to_string()
    } else {
        ctx.cwd.join(file).display().to_string()
    }
}

fn decode(bytes: Vec<u8>, source: &str) -> Result<String, CliError> {
    String::from_utf8(bytes).map_err(|_| not_utf8(source))
}

fn not_utf8(source: &str) -> CliError {
    io_error(format!("the text from {source} isn't UTF-8"))
}

/// The text a `--note` / `--note-file` gives, read and cleaned as a body is; `None` when
/// neither is given or the text is empty.
pub(crate) fn note(note: &Note, ctx: &mut Context) -> Result<Option<String>, CliError> {
    Ok(read(note.note.as_deref(), note.note_file.as_deref(), ctx)?
        .filter(|text| !text.trim().is_empty()))
}

/// Strips what a pipe or an editor adds around the text (module docs).
pub(crate) fn normalise(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.replace("\r\n", "\n").trim_end().to_string()
}

fn io_error(message: String) -> CliError {
    CliError::new(Exit::Failure, code::IO, message)
}
