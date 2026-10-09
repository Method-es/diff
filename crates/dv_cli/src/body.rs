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
        (None, Some(file)) if file == Path::new("-") => {
            let mut bytes = Vec::new();
            ctx.stdin
                .read_to_end(&mut bytes)
                .map_err(|error| io_error(format!("reading the text from stdin: {error}")))?;
            decode(bytes, "stdin")?
        }
        (None, Some(file)) => {
            let path = ctx.cwd.join(file);
            let bytes = std::fs::read(&path)
                .map_err(|error| io_error(format!("reading {}: {error}", path.display())))?;
            decode(bytes, &path.display().to_string())?
        }
        (None, None) => return Ok(None),
    };
    Ok(Some(normalise(&raw)))
}

fn decode(bytes: Vec<u8>, source: &str) -> Result<String, CliError> {
    String::from_utf8(bytes).map_err(|_| io_error(format!("the text from {source} isn't UTF-8")))
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
