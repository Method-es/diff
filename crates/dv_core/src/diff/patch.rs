//! The parser for the unified diff `git diff` prints (docs/PLAN.md §6 step 2).
//!
//! Hunk bodies are read by the line counts in their `@@` headers, never by what a line looks like:
//! a reviewed file may itself contain diff text (a `.patch` file), and a removed line `-- a/x`
//! prints as `--- a/x`, exactly like a file header. Names in headers follow git's quoting: a name
//! with a `"`, a `\` or a control character is C-quoted (`"a/x\ty"`; `core.quotepath=off` leaves
//! other bytes alone), and a `---`/`+++` name containing a space gets a trailing tab.

use std::borrow::Cow;

use super::{DiffLine, LineKind};

/// One file's section of a patch: everything from its `diff --git` line to the next one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PatchFile {
    /// The names on the `diff --git a/… b/…` line, when they could be told apart.
    header: Option<(String, String)>,
    /// The `---` name; `Some(None)` for `/dev/null`.
    minus: Option<Option<String>>,
    /// The `+++` name; `Some(None)` for `/dev/null`.
    plus: Option<Option<String>>,
    rename_from: Option<String>,
    rename_to: Option<String>,
    copy_from: Option<String>,
    copy_to: Option<String>,
    /// `Binary files … differ` (or a binary patch) instead of hunks.
    pub binary: bool,
    /// A combined diff (`diff --cc`) of an unmerged file; its hunks aren't parsed.
    pub combined: bool,
    /// A hunk has been read: only further hunks may follow.
    seen_hunk: bool,
    pub lines: Vec<DiffLine>,
    pub old_eof_newline: bool,
    pub new_eof_newline: bool,
    /// A content line ended `\r\n`; the `\r` is stripped from the line's text.
    pub crlf: bool,
}

impl PatchFile {
    fn new(header: Option<(String, String)>) -> Self {
        Self {
            header,
            minus: None,
            plus: None,
            rename_from: None,
            rename_to: None,
            copy_from: None,
            copy_to: None,
            binary: false,
            combined: false,
            seen_hunk: false,
            lines: Vec::new(),
            old_eof_newline: true,
            new_eof_newline: true,
            crlf: false,
        }
    }

    /// The file's path on the new side, or for a deleted file the path it had.
    pub fn new_name(&self) -> Option<&str> {
        self.rename_to
            .as_deref()
            .or(self.copy_to.as_deref())
            .or(self.plus.as_ref().and_then(|name| name.as_deref()))
            .or(self.header.as_ref().map(|(_, b)| b.as_str()))
    }

    /// The file's path on the old side, or for an added file the path it has.
    #[cfg(test)]
    pub fn old_name(&self) -> Option<&str> {
        self.rename_from
            .as_deref()
            .or(self.copy_from.as_deref())
            .or(self.minus.as_ref().and_then(|name| name.as_deref()))
            .or(self.header.as_ref().map(|(a, _)| a.as_str()))
    }

    /// Records an extended header line (`rename from …`, `--- a/…`, `Binary files …`); lines
    /// dv doesn't need (`index …`, modes, similarity) are skipped.
    fn header_line(&mut self, line: &[u8]) {
        let text = |rest: &[u8]| unquote(rest).map(|name| lossy(&name).into_owned());
        if let Some(rest) = line.strip_prefix(b"--- ") {
            self.minus = side_name(rest, b"a/");
        } else if let Some(rest) = line.strip_prefix(b"+++ ") {
            self.plus = side_name(rest, b"b/");
        } else if let Some(rest) = line.strip_prefix(b"rename from ") {
            self.rename_from = text(rest);
        } else if let Some(rest) = line.strip_prefix(b"rename to ") {
            self.rename_to = text(rest);
        } else if let Some(rest) = line.strip_prefix(b"copy from ") {
            self.copy_from = text(rest);
        } else if let Some(rest) = line.strip_prefix(b"copy to ") {
            self.copy_to = text(rest);
        } else if (line.starts_with(b"Binary files ") && line.ends_with(b" differ"))
            || line == b"GIT binary patch"
        {
            self.binary = true;
        }
    }
}

/// Parses `git diff` output (`--src-prefix=a/ --dst-prefix=b/`) into its file sections.
pub(crate) fn parse(output: &[u8]) -> Result<Vec<PatchFile>, String> {
    if output.is_empty() {
        return Ok(Vec::new());
    }
    let mut lines: Vec<&[u8]> = output.split(|&b| b == b'\n').collect();
    if output.ends_with(b"\n") {
        // The empty piece after the final newline.
        lines.pop();
    }
    let mut files: Vec<PatchFile> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(rest) = line.strip_prefix(b"diff --git ") {
            files.push(PatchFile::new(header_names(rest)));
            i += 1;
        } else if let Some(rest) = line
            .strip_prefix(b"diff --cc ")
            .or_else(|| line.strip_prefix(b"diff --combined "))
        {
            let name = unquote(rest).map(|name| lossy(&name).into_owned());
            let mut file = PatchFile::new(name.map(|name| (name.clone(), name)));
            file.combined = true;
            files.push(file);
            // Every body line of a combined diff starts with its two-column prefix, so the next
            // line starting `diff ` is the next section's header.
            i += 1;
            while i < lines.len() && !lines[i].starts_with(b"diff ") {
                i += 1;
            }
        } else if let Some(file) = files.last_mut() {
            if line.starts_with(b"@@ ") {
                file.seen_hunk = true;
                i = parse_hunk(&lines, i, file)?;
            } else if file.seen_hunk {
                return Err(format!("unexpected line after a hunk: {:?}", lossy(line)));
            } else {
                file.header_line(line);
                i += 1;
            }
        } else {
            // Anything before the first header (a warning git printed to stdout) isn't a diff.
            i += 1;
        }
    }
    Ok(files)
}

/// Parses the hunk whose `@@` header is `lines[start]` into `file`, returning the index of the
/// first line after it.
fn parse_hunk(lines: &[&[u8]], start: usize, file: &mut PatchFile) -> Result<usize, String> {
    let header = lines[start];
    let (old_start, mut old_left, new_start, mut new_left) =
        hunk_header(header).ok_or_else(|| format!("malformed hunk header {:?}", lossy(header)))?;
    // A side with no lines in the hunk names the line before the hunk (`-0,0`, `+5,0`).
    let mut old_no = if old_left == 0 {
        old_start.saturating_add(1)
    } else {
        old_start
    };
    let mut new_no = if new_left == 0 {
        new_start.saturating_add(1)
    } else {
        new_start
    };
    let mut last_kind = None;
    let mut i = start + 1;
    while let Some(&line) = lines.get(i) {
        if line.first() == Some(&b'\\') {
            // `\ No newline at end of file`: the line before it is its side's last and has no
            // newline.
            match last_kind {
                Some(LineKind::Removed) => file.old_eof_newline = false,
                Some(LineKind::Added) => file.new_eof_newline = false,
                Some(LineKind::Context) => {
                    file.old_eof_newline = false;
                    file.new_eof_newline = false;
                }
                None => return Err("a no-newline marker before any line".to_string()),
            }
            i += 1;
            continue;
        }
        if old_left == 0 && new_left == 0 {
            break;
        }
        let (kind, body) = match line.split_first() {
            Some((b' ', body)) => (LineKind::Context, body),
            Some((b'-', body)) => (LineKind::Removed, body),
            Some((b'+', body)) => (LineKind::Added, body),
            // `diff.suppressBlankEmpty` prints an empty context line without its space.
            None => (LineKind::Context, line),
            Some(_) => {
                return Err(format!("unexpected line in a hunk: {:?}", lossy(line)));
            }
        };
        let (takes_old, takes_new) = match kind {
            LineKind::Context => (true, true),
            LineKind::Removed => (true, false),
            LineKind::Added => (false, true),
        };
        if (takes_old && old_left == 0) || (takes_new && new_left == 0) {
            return Err(format!(
                "hunk {:?} has more lines than its header counts",
                lossy(header)
            ));
        }
        let body = match body.strip_suffix(b"\r") {
            Some(stripped) => {
                file.crlf = true;
                stripped
            }
            None => body,
        };
        let numbered = |takes: bool, left: &mut u32, no: &mut u32| {
            takes.then(|| {
                *left -= 1;
                let current = *no;
                *no = no.saturating_add(1);
                current
            })
        };
        let old = numbered(takes_old, &mut old_left, &mut old_no);
        let new = numbered(takes_new, &mut new_left, &mut new_no);
        file.lines.push(DiffLine {
            kind,
            old_no: old,
            new_no: new,
            text: lossy(body).into_owned(),
            words: Vec::new(),
        });
        last_kind = Some(kind);
        i += 1;
    }
    if old_left != 0 || new_left != 0 {
        return Err(format!(
            "hunk {:?} ends before its header's line counts",
            lossy(header)
        ));
    }
    Ok(i)
}

/// `@@ -<old_start>[,<old_count>] +<new_start>[,<new_count>] @@[ section]` → the four numbers
/// (a count left out is 1).
fn hunk_header(line: &[u8]) -> Option<(u32, u32, u32, u32)> {
    let rest = line.strip_prefix(b"@@ -")?;
    // The section heading after the ranges is a line of the file, in whatever encoding it has.
    let end = rest.windows(3).position(|window| window == b" @@")?;
    let ranges = std::str::from_utf8(&rest[..end]).ok()?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |range: &str| -> Option<(u32, u32)> {
        match range.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((range.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = range(old)?;
    let (new_start, new_count) = range(new)?;
    Some((old_start, old_count, new_start, new_count))
}

/// The two names on a `diff --git` line, without their `a/` and `b/` prefixes; `None` when they
/// can't be told apart (an unquoted rename whose names contain spaces — its `rename from`/`rename
/// to` lines name it instead).
fn header_names(rest: &[u8]) -> Option<(String, String)> {
    let finish = |a: &[u8], b: &[u8]| {
        Some((
            lossy(a.strip_prefix(b"a/")?).into_owned(),
            lossy(b.strip_prefix(b"b/")?).into_owned(),
        ))
    };
    if rest.first() == Some(&b'"') {
        let (a, used) = unquote_prefix(rest)?;
        let b = unquote(rest[used..].strip_prefix(b" ")?)?;
        return finish(&a, &b);
    }
    if rest.last() == Some(&b'"') {
        // An unquoted name never contains `"` (git would have quoted it), so the first ` "`
        // starts the quoted second name.
        let at = rest.windows(2).position(|pair| pair == b" \"")?;
        let b = unquote(&rest[at + 1..])?;
        return finish(&rest[..at], &b);
    }
    // Unquoted and not a rename: the same path twice, `a/<path> b/<path>`.
    if rest.len() % 2 == 1 {
        let mid = rest.len() / 2;
        let (a, b) = (&rest[..mid], &rest[mid + 1..]);
        if rest[mid] == b' ' && a.get(2..) == b.get(2..) {
            return finish(a, b);
        }
    }
    None
}

/// A `---`/`+++` name: `Some(None)` for `/dev/null`, else the name without `prefix`.
fn side_name(rest: &[u8], prefix: &[u8]) -> Option<Option<String>> {
    // git ends the name with a tab when it contains a space, so patch tools don't read the rest
    // of the line as a timestamp; a name that itself ends with a tab would be quoted.
    let rest = rest.strip_suffix(b"\t").unwrap_or(rest);
    if rest == b"/dev/null" {
        return Some(None);
    }
    let name = unquote(rest)?;
    Some(Some(lossy(name.strip_prefix(prefix)?).into_owned()))
}

/// A name as git prints it: C-quoted (`"…"`) or verbatim.
pub(crate) fn unquote(text: &[u8]) -> Option<Vec<u8>> {
    if text.first() != Some(&b'"') {
        return Some(text.to_vec());
    }
    let (name, used) = unquote_prefix(text)?;
    (used == text.len()).then_some(name)
}

/// Decodes the C-quoted string `text` starts with (git's `quote_c_style`: `\a \b \t \n \v \f \r
/// \" \\` and three-digit octal escapes), returning it and the number of bytes it took.
fn unquote_prefix(text: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut out = Vec::new();
    let mut i = 1;
    loop {
        match *text.get(i)? {
            b'"' => return Some((out, i + 1)),
            b'\\' => {
                let escape = *text.get(i + 1)?;
                i += 2;
                out.push(match escape {
                    b'a' => 0x07,
                    b'b' => 0x08,
                    b't' => b'\t',
                    b'n' => b'\n',
                    b'v' => 0x0b,
                    b'f' => 0x0c,
                    b'r' => b'\r',
                    b'"' => b'"',
                    b'\\' => b'\\',
                    b'0'..=b'3' => {
                        let digits = text.get(i - 1..i + 2)?;
                        let value = digits.iter().try_fold(0u8, |value, &digit| {
                            (b'0'..=b'7')
                                .contains(&digit)
                                .then(|| value.wrapping_mul(8) + (digit - b'0'))
                        })?;
                        i += 2;
                        value
                    }
                    _ => return None,
                });
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
}

fn lossy(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

#[cfg(test)]
mod tests;
