//! A worktree's files as a language server names them: absolute paths on the backend's machine
//! and `file:` URIs (docs/PLAN.md §18.3, §18.4).
//!
//! A server runs where the repository lives, so its paths are that machine's: POSIX inside a WSL
//! distro, Windows on this machine. [`WorkspacePaths`] takes the spelling from the backend's
//! `toplevel` and maps a worktree-relative path (the `/`-separated paths every `Backend` method
//! takes) to an absolute path and a URI, and a URI the server answers with back to a worktree
//! path when it is inside the worktree.

use std::str::FromStr as _;

use lsp_types::Uri;

/// How the backend's machine spells paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathStyle {
    /// `/home/kyle/repo`.
    Posix,
    /// `D:\Software\repo`, `\\server\share\repo`.
    Windows,
}

/// A URI dv made that doesn't parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{uri} isn't a valid URI: {reason}")]
pub struct InvalidUri {
    pub uri: String,
    pub reason: String,
}

/// Where a URI a server answered with points.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UriTarget {
    /// A file inside the worktree, by its worktree-relative path.
    Worktree(String),
    /// Anything else: a file outside the worktree (`path`, in the backend's spelling), or a URI
    /// that isn't a file (a server's bundled library, `path` `None`).
    Outside { uri: String, path: Option<String> },
}

/// The paths of one worktree (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePaths {
    /// The worktree root, `/`-separated, without a trailing separator.
    root: String,
    style: PathStyle,
}

impl WorkspacePaths {
    /// The worktree whose root is `toplevel`, spelled as `Backend::repo().toplevel` spells it.
    pub fn new(toplevel: &str) -> Self {
        let style = if toplevel.starts_with('/') {
            PathStyle::Posix
        } else {
            PathStyle::Windows
        };
        let mut root = strip_verbatim(&toplevel.replace('\\', "/"));
        // A trailing separator goes, except the one that makes `/` or `C:/` a root.
        while root.len() > 1 && root.ends_with('/') && !root.ends_with(":/") {
            root.pop();
        }
        Self { root, style }
    }

    pub fn style(&self) -> PathStyle {
        self.style
    }

    /// `rel` (`""` is the root) as an absolute path in the backend's spelling. A path that is
    /// absolute already (a file outside the worktree, which the app shows read-only and asks
    /// about) is that path.
    pub fn absolute(&self, rel: &str) -> String {
        if is_absolute(rel, self.style) {
            return match self.style {
                PathStyle::Posix => rel.to_string(),
                PathStyle::Windows => rel.replace('/', "\\"),
            };
        }
        let joined = if rel.is_empty() {
            self.root.clone()
        } else if self.root.ends_with('/') {
            format!("{}{rel}", self.root)
        } else {
            format!("{}/{rel}", self.root)
        };
        match self.style {
            PathStyle::Posix => joined,
            PathStyle::Windows => joined.replace('/', "\\"),
        }
    }

    /// `rel`'s `file:` URI, parsed. Every byte outside a URI path's characters is
    /// percent-encoded, so it parses unless the root itself can't be a URI (a UNC host with
    /// characters a host can't have), which is an error rather than some other URI.
    pub fn uri(&self, rel: &str) -> Result<Uri, InvalidUri> {
        let text = self.uri_text(rel);
        Uri::from_str(&text).map_err(|err| InvalidUri {
            uri: text,
            reason: err.to_string(),
        })
    }

    /// `rel`'s `file:` URI as text.
    pub fn uri_text(&self, rel: &str) -> String {
        let path = self.absolute(rel).replace('\\', "/");
        match self.style {
            PathStyle::Posix => format!("file://{}", encode_path(&path)),
            PathStyle::Windows => match path.strip_prefix("//") {
                // UNC: `\\server\share\x` is `file://server/share/x`.
                Some(unc) => {
                    let (host, rest) = unc.split_once('/').unwrap_or((unc, ""));
                    format!("file://{host}/{}", encode_path(rest))
                }
                None => format!("file:///{}", encode_path(&path)),
            },
        }
    }

    /// The worktree-relative path of an absolute path in the backend's spelling (or with `/`),
    /// `None` when it is outside the worktree. `""` is the root itself.
    pub fn relative(&self, absolute: &str) -> Option<String> {
        let path = strip_verbatim(&absolute.replace('\\', "/"));
        let rest = match self.style {
            PathStyle::Posix => path.strip_prefix(&self.root)?.to_string(),
            PathStyle::Windows => {
                let head = path.get(..self.root.len())?;
                if !head.eq_ignore_ascii_case(&self.root) {
                    return None;
                }
                path[self.root.len()..].to_string()
            }
        };
        if rest.is_empty() {
            return Some(String::new());
        }
        if self.root.ends_with('/') {
            return Some(rest);
        }
        rest.strip_prefix('/').map(str::to_string)
    }

    /// Where `uri` points: a worktree file, or something outside.
    pub fn target(&self, uri: &str) -> UriTarget {
        let outside = |path: Option<String>| UriTarget::Outside {
            uri: uri.to_string(),
            path,
        };
        let Some(path) = self.path_of_uri(uri) else {
            return outside(None);
        };
        match self.relative(&path) {
            Some(rel) if !rel.is_empty() => UriTarget::Worktree(rel),
            _ => outside(Some(path)),
        }
    }

    /// The absolute path a `file:` URI names, in the backend's spelling; `None` for another
    /// scheme.
    pub fn path_of_uri(&self, uri: &str) -> Option<String> {
        let rest = strip_scheme(uri)?;
        let (host, path) = match rest.strip_prefix("//") {
            Some(after) => match after.find('/') {
                Some(at) => (&after[..at], &after[at..]),
                None => (after, ""),
            },
            None => ("", rest),
        };
        let path = decode(path);
        match self.style {
            PathStyle::Posix => (host.is_empty() || host == "localhost").then_some(path),
            PathStyle::Windows => {
                if !host.is_empty() && host != "localhost" {
                    return Some(format!("\\\\{host}{}", path.replace('/', "\\")));
                }
                // `/D:/x` (or `/d%3A/x`, decoded) is `D:\x`.
                let path = match path.as_bytes() {
                    [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => &path[1..],
                    _ => &path,
                };
                Some(path.replace('/', "\\"))
            }
        }
    }
}

/// `file:` (any case) removed, or `None` for another scheme.
fn strip_scheme(uri: &str) -> Option<&str> {
    let head = uri.get(..5)?;
    head.eq_ignore_ascii_case("file:").then(|| &uri[5..])
}

/// A Windows verbatim prefix (`//?/`, and `/??/` as a junction's target reads) removed; a
/// verbatim UNC path (`//?/UNC/server/share`) becomes `//server/share`.
fn strip_verbatim(path: &str) -> String {
    for prefix in ["//?/", "/??/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            return match rest.strip_prefix("UNC/") {
                Some(unc) => format!("//{unc}"),
                None => rest.to_string(),
            };
        }
    }
    path.to_string()
}

/// The characters a URI path keeps as they are: unreserved, sub-delims, `:`, `@` and `/`.
fn keeps(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'.'
                | b'_'
                | b'~'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'@'
                | b'/'
        )
}

fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &byte in path.as_bytes() {
        if keeps(byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Percent-decoding, leniently: a `%` not followed by two hex digits stays as it is.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = text.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether `path` is absolute in `style`: `/…` on POSIX; a drive (`C:\`, `C:/`) or a UNC path on
/// Windows.
pub fn is_absolute(path: &str, style: PathStyle) -> bool {
    match style {
        PathStyle::Posix => path.starts_with('/'),
        PathStyle::Windows => {
            let bytes = path.as_bytes();
            (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\'))
                || path.starts_with("\\\\")
                || path.starts_with("//")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_paths_and_uris() {
        let paths = WorkspacePaths::new("/home/kyle/code/my app");
        assert_eq!(paths.style(), PathStyle::Posix);
        assert_eq!(paths.absolute(""), "/home/kyle/code/my app");
        assert_eq!(
            paths.absolute("src/a.ts"),
            "/home/kyle/code/my app/src/a.ts"
        );
        assert_eq!(
            paths.uri_text("node_modules/.bun/@typescript+typescript-linux-x64@7.0.2/x#y%.ts"),
            "file:///home/kyle/code/my%20app/node_modules/.bun/@typescript+typescript-linux-x64@7.0.2/x%23y%25.ts"
        );
        assert_eq!(
            paths.uri("src/\u{e9}.ts").unwrap().as_str(),
            "file:///home/kyle/code/my%20app/src/%C3%A9.ts"
        );
        assert_eq!(
            paths.target("file:///home/kyle/code/my%20app/src/%C3%A9.ts"),
            UriTarget::Worktree("src/\u{e9}.ts".into())
        );
        assert_eq!(
            paths.target("file:///home/kyle/code/my%20apple/x.ts"),
            UriTarget::Outside {
                uri: "file:///home/kyle/code/my%20apple/x.ts".into(),
                path: Some("/home/kyle/code/my apple/x.ts".into())
            }
        );
        assert_eq!(
            paths.target("bundled:///libs/lib.dom.d.ts"),
            UriTarget::Outside {
                uri: "bundled:///libs/lib.dom.d.ts".into(),
                path: None
            }
        );
        assert_eq!(
            paths.relative("/home/kyle/code/my app"),
            Some(String::new())
        );
        assert_eq!(paths.relative("/home/kyle/code/my app/x"), Some("x".into()));
        assert_eq!(paths.relative("/home/kyle/code/my"), None);
    }

    #[test]
    fn windows_paths_and_uris() {
        let paths = WorkspacePaths::new(r"D:\Software\diff");
        assert_eq!(paths.style(), PathStyle::Windows);
        assert_eq!(paths.absolute("src/a.ts"), r"D:\Software\diff\src\a.ts");
        assert_eq!(
            paths.uri_text("src/a b.ts"),
            "file:///D:/Software/diff/src/a%20b.ts"
        );
        // VS Code's spelling: a lower-case drive with its colon encoded.
        assert_eq!(
            paths.target("file:///d%3A/software/diff/src/a.ts"),
            UriTarget::Worktree("src/a.ts".into())
        );
        assert_eq!(
            paths.target("file:///C:/Users/x/lib.d.ts"),
            UriTarget::Outside {
                uri: "file:///C:/Users/x/lib.d.ts".into(),
                path: Some(r"C:\Users\x\lib.d.ts".into())
            }
        );
        // A junction's target, as `read_link` gives it with `/`.
        assert_eq!(
            paths.relative("//?/D:/Software/diff/node_modules/.pnpm"),
            Some("node_modules/.pnpm".into())
        );
    }

    #[test]
    fn unc_paths() {
        let paths = WorkspacePaths::new(r"\\wsl.localhost\Ubuntu\tmp\repo");
        assert_eq!(paths.style(), PathStyle::Windows);
        assert_eq!(
            paths.uri_text("a.ts"),
            "file://wsl.localhost/Ubuntu/tmp/repo/a.ts"
        );
        assert_eq!(
            paths.target("file://wsl.localhost/Ubuntu/tmp/repo/a.ts"),
            UriTarget::Worktree("a.ts".into())
        );
        assert_eq!(
            paths.relative("//?/UNC/wsl.localhost/Ubuntu/tmp/repo/b"),
            Some("b".into())
        );
        // A host can't hold a space: an error, not another URI.
        assert!(WorkspacePaths::new(r"\\bad host\share").uri("a").is_err());
    }

    #[test]
    fn decoding_is_lenient() {
        assert_eq!(decode("a%20b%zz%4"), "a b%zz%4");
    }
}
