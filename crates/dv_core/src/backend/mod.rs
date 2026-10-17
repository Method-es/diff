//! The `Backend` interface between dv and one repository worktree (docs/PLAN.md D6, §5.1).
//!
//! Everything dv does to a repo — run git, read worktree files, read and write the review store
//! (`<git-common-dir>/dv/`), watch for changes — goes through this one trait, so the layers above
//! (diff pipeline, review store, UI, CLI) never know where the repo lives. It also runs processes
//! where the repo lives ([`process`]): a WSL repository's language servers run in its distro.
//! [`LocalBackend`] implements it in-process; the same trait is what `dv backend --stdio` serves
//! from inside a WSL distro (PLAN D6), which is why every method is blocking and takes and
//! returns plain data that serialises 1:1.

mod local;
mod paths;
pub mod process;
mod watch;

use std::borrow::Cow;
use std::fmt;
use std::io;

use serde::{Deserialize, Serialize};

use crate::git::ObjectFormat;
use crate::location::Location;

pub use local::{LocalBackend, LocalBackendOptions};
pub(crate) use paths::check_relative;
pub use process::{
    ProcessControl, ProcessEvent, ProcessHandle, ProcessSink, ProcessSpec, STDERR_LINE_MAX,
};

/// One repository worktree's operations (PLAN §5.1). An instance is bound to the worktree it was
/// opened on: git runs in its root, worktree paths are relative to that root, and store paths
/// are relative to `<git-common-dir>/dv/`, which every worktree of a repo shares (PLAN §7).
///
/// Relative paths are `/`-separated and validated before use: no absolute paths, no `..` or `.`
/// components, no backslashes ([`BackendError::InvalidPath`]).
pub trait Backend: Send + Sync {
    /// The repository this backend is bound to.
    fn repo(&self) -> &RepoInfo;

    /// Runs `git <args>` in the worktree root under dv's invocation policy
    /// ([`crate::git::command`]). An exit code outside `opts.ok_codes` is
    /// [`BackendError::Git`].
    fn git(&self, args: &[&str], opts: &GitOpts) -> Result<GitOutput, BackendError>;

    /// A worktree file's bytes as git sees them, `None` when it doesn't exist. A symbolic link is
    /// never followed: its bytes are its target path, which is what git stores for a link (and
    /// what it diffs), whether the target exists or is a directory.
    fn worktree_read(&self, rel: &str) -> Result<Option<Vec<u8>>, BackendError>;

    /// The size in bytes of what [`Backend::worktree_read`] returns, `None` when the file doesn't
    /// exist.
    fn worktree_len(&self, rel: &str) -> Result<Option<u64>, BackendError>;

    /// A worktree file's size, line count and binary sniff ([`TextStats`]) as git sees the file
    /// (a symbolic link is its target path), `None` when it doesn't exist. The file list counts
    /// untracked files' lines with this (PLAN §6 step 1), so an implementation reads the file
    /// where it lives and returns only the numbers. The default reads the whole file through
    /// [`Backend::worktree_read`] and can't tell a link from a file; [`LocalBackend`] streams the
    /// file, stops early on a binary one and reports links.
    fn worktree_text_stats(&self, rel: &str) -> Result<Option<TextStats>, BackendError> {
        Ok(self.worktree_read(rel)?.map(|bytes| TextStats::of(&bytes)))
    }

    /// What is at `path`, an absolute, normalised path on the backend's machine, which may be
    /// outside the worktree: a file (its bytes too when it is at most `read` bytes, `read` itself
    /// at most [`PROBE_READ_MAX`]; `0` only looks), a directory, a symbolic link (never followed:
    /// its target as written), or nothing. Read-only.
    /// The language servers' resolvers read with it (PLAN §18.4): Node resolves packages above
    /// the worktree root (a linked worktree inside the main checkout uses the checkout's
    /// `node_modules`) and through links that leave it (pnpm's global store, `npm link`). A
    /// relative or unnormalised path is [`BackendError::InvalidPath`]. The default refuses every
    /// path; [`LocalBackend`] and the remote backend implement it.
    fn probe_path(&self, path: &str, read: u64) -> Result<PathProbe, BackendError> {
        let _ = read;
        Err(BackendError::InvalidPath {
            path: path.to_string(),
            reason: "this backend reads only inside its worktree".into(),
        })
    }

    /// Appends `line` and a `\n` to a store file in one write, creating the file and its parent
    /// directories as needed. Concurrent appends from any number of threads and processes never
    /// interleave (PLAN D5). `line` may not contain `\n`.
    fn store_append(&self, rel: &str, line: &[u8]) -> Result<(), BackendError>;

    /// A store file's bytes, `None` when it doesn't exist.
    fn store_read(&self, rel: &str) -> Result<Option<Vec<u8>>, BackendError>;

    /// The names of the files directly in a store directory (`""` is the store root), sorted;
    /// empty when the directory doesn't exist.
    fn store_list(&self, dir: &str) -> Result<Vec<String>, BackendError>;

    /// Replaces a store file's contents atomically: readers see the old bytes or the new ones,
    /// never a mix. Creates parent directories as needed.
    fn store_write_atomic(&self, rel: &str, bytes: &[u8]) -> Result<(), BackendError>;

    /// Removes a store file; `false` when there was none.
    fn store_remove(&self, rel: &str) -> Result<bool, BackendError>;

    /// Copies this worktree's index file (`<git-dir>/index`) byte for byte to the store file
    /// `rel`, keeping its modification time, and creates `rel`'s parent directories either way.
    /// `false` when the worktree has no index yet (a repo where nothing was ever staged); git
    /// then starts an empty index at `rel` when a call names it ([`GitOpts::index_file`]).
    ///
    /// A round snapshot works on such a copy so the user's index is never written (PLAN §2.3,
    /// §2.4). The modification time matters: git trusts an entry's cached stat data only when
    /// the file was modified before the index was written, so a copy dated now would let a file
    /// changed in the same instant as the last index write pass as unchanged.
    fn copy_index_to_store(&self, rel: &str) -> Result<bool, BackendError>;

    /// Starts delivering change notifications to `sink` on a background thread (PLAN §5.3) until
    /// the returned handle is dropped. A watch that includes the store, whose worktree or git
    /// half the OS refuses, still watches the store; [`WatchHandle::worktree_error`] says so.
    fn watch(&self, spec: WatchSpec, sink: WatchSink) -> Result<WatchHandle, BackendError>;

    /// Starts `spec` where the repository lives, with piped stdio and never through a shell
    /// (PLAN §18.3): what it prints reaches `sink` on a thread of the process's own until the
    /// returned handle, which writes its stdin and kills it, is dropped ([`process`]). A
    /// program that isn't an absolute path, or a working directory that isn't a directory in
    /// the worktree, is refused before anything starts.
    fn spawn_stdio(
        &self,
        spec: &ProcessSpec,
        sink: ProcessSink,
    ) -> Result<ProcessHandle, BackendError>;
}

/// What a backend knows about its repository, resolved once when it opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoInfo {
    /// The worktree root's location.
    pub location: Location,
    /// The worktree root, absolute, spelled the way the backend's OS spells paths.
    pub toplevel: String,
    /// This worktree's git dir: `<root>/.git`, or `<main>/.git/worktrees/<name>` for a linked
    /// worktree.
    pub git_dir: String,
    /// The git dir every worktree of the repo shares (`git rev-parse --git-common-dir`); the
    /// review store lives in its `dv/` (PLAN §7).
    pub common_dir: String,
    pub object_format: ObjectFormat,
    pub git_version: GitVersion,
}

impl RepoInfo {
    /// Whether this is a linked worktree (`git worktree add`), whose git dir differs from the
    /// common dir.
    pub fn is_linked_worktree(&self) -> bool {
        self.git_dir != self.common_dir
    }
}

/// The version of the git a backend runs, from `git version`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    /// The full version as git prints it (`2.53.0.windows.1`).
    pub text: String,
}

impl GitVersion {
    /// Parses `git version` output (`git version 2.53.0.windows.1`,
    /// `git version 2.39.3 (Apple Git-146)`).
    pub fn parse(output: &str) -> Option<Self> {
        let text = output.trim().strip_prefix("git version ")?.trim();
        let mut numbers = text
            .split(|c: char| !c.is_ascii_digit())
            .map(|part| part.parse::<u32>().ok());
        let major = numbers.next()??;
        let minor = numbers.next()??;
        let patch = numbers.next().flatten().unwrap_or(0);
        Some(Self {
            major,
            minor,
            patch,
            text: text.to_string(),
        })
    }

    /// Whether this version is `major.minor` or newer.
    pub fn at_least(&self, major: u32, minor: u32) -> bool {
        (self.major, self.minor) >= (major, minor)
    }
}

/// The most bytes of a file [`Backend::probe_path`] returns, whatever it is asked for: a
/// library's declarations the file view shows outside the repository (TypeScript's
/// `lib.dom.d.ts` is about 2 MB).
pub const PROBE_READ_MAX: u64 = 4 * 1024 * 1024;

/// What a resolver reads of a small file with [`Backend::probe_path`]: a `package.json`, a
/// shim, `/etc/passwd`, never an executable. Its other probes only look (`read` 0).
pub const PROBE_READ_SMALL: u64 = 1024 * 1024;

/// What [`Backend::probe_path`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathProbe {
    Missing,
    /// A file of `len` bytes; `contents` when it is at most what the probe asked to read.
    File {
        len: u64,
        contents: Option<Vec<u8>>,
    },
    Dir,
    /// A symbolic link (or a Windows junction): its target as the OS reads it, in the
    /// machine's spelling, relative or absolute.
    Link {
        target: String,
    },
}

/// What [`Backend::worktree_text_stats`] reports about a file, counted the way git counts for
/// `--numstat` (PLAN §6 step 1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextStats {
    /// The size in bytes.
    pub len: u64,
    /// Lines: one per `\n`, plus a final line without one. `0` for a binary file, whose lines
    /// aren't counted.
    pub lines: u64,
    /// A NUL byte in the first [`TextStats::SNIFF_LEN`] bytes: git's own binary heuristic
    /// (`buffer_is_binary`).
    pub binary: bool,
    /// The path is a symbolic link; the counts are of its target path, which git stores (and
    /// diffs) as the link's content.
    pub symlink: bool,
}

impl TextStats {
    /// How far into a file git looks for a NUL byte when guessing whether it is binary.
    pub const SNIFF_LEN: u64 = 8000;

    /// The stats of a whole file's bytes.
    pub fn of(bytes: &[u8]) -> Self {
        let mut counter = TextStatsCounter::default();
        counter.feed(bytes);
        counter.finish()
    }
}

/// Builds [`TextStats`] from a file read in chunks, so a backend never holds a large file in
/// memory to count it.
#[derive(Debug, Clone, Default)]
pub struct TextStatsCounter {
    stats: TextStats,
    /// The last byte seen, to count a final line that has no `\n`.
    last: Option<u8>,
}

impl TextStatsCounter {
    /// Accounts for the next chunk of the file.
    pub fn feed(&mut self, chunk: &[u8]) {
        let Some(&last) = chunk.last() else {
            return;
        };
        let stats = &mut self.stats;
        if stats.len < TextStats::SNIFF_LEN {
            let sniff = usize::try_from(TextStats::SNIFF_LEN - stats.len).unwrap_or(usize::MAX);
            if chunk[..sniff.min(chunk.len())].contains(&0) {
                stats.binary = true;
            }
        }
        stats.len += chunk.len() as u64;
        if !stats.binary {
            stats.lines += chunk.iter().filter(|&&b| b == b'\n').count() as u64;
        }
        self.last = Some(last);
    }

    /// Whether the file has turned out binary: its lines no longer matter, so a caller may
    /// stop reading and set the length itself ([`TextStatsCounter::finish_with_len`]).
    pub fn is_binary(&self) -> bool {
        self.stats.binary
    }

    /// The stats of everything fed.
    pub fn finish(self) -> TextStats {
        let mut stats = self.stats;
        if stats.binary {
            stats.lines = 0;
        } else if self.last.is_some_and(|b| b != b'\n') {
            stats.lines += 1;
        }
        stats
    }

    /// The stats of a file of `len` bytes whose reading stopped early (a binary file).
    pub fn finish_with_len(self, len: u64) -> TextStats {
        TextStats {
            len,
            ..self.finish()
        }
    }
}

/// Per-call options for [`Backend::git`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitOpts {
    /// Bytes written to git's stdin, which is closed afterwards; `None` gives git a null stdin.
    pub stdin: Option<Vec<u8>>,
    /// Fails with [`BackendError::Timeout`] after this long, killing git if it is still running
    /// (and no longer waiting for a process git started that holds its output open); `None`
    /// waits forever.
    pub timeout_ms: Option<u64>,
    /// Exit codes that count as success (`[0]` by default; e.g. `[0, 1]` for `git diff
    /// --no-index`, which exits 1 when the files differ).
    pub ok_codes: Vec<i32>,
    /// A store file (a store-relative path, validated like every store path) that this one
    /// call uses as its index: the backend resolves it under `<git-common-dir>/dv/` and exports
    /// it as `GIT_INDEX_FILE`, after the git policy has removed any inherited one. Round
    /// snapshots stage into a copy of the index this way ([`Backend::copy_index_to_store`],
    /// PLAN §2.3); `None` is the worktree's own index.
    #[serde(default)]
    pub index_file: Option<String>,
}

impl Default for GitOpts {
    fn default() -> Self {
        Self {
            stdin: None,
            timeout_ms: None,
            ok_codes: vec![0],
            index_file: None,
        }
    }
}

impl GitOpts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_stdin(mut self, stdin: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(stdin.into());
        self
    }

    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout_ms = Some(u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX));
        self
    }

    pub fn with_ok_codes(mut self, codes: &[i32]) -> Self {
        self.ok_codes = codes.to_vec();
        self
    }

    pub fn with_index_file(mut self, rel: impl Into<String>) -> Self {
        self.index_file = Some(rel.into());
        self
    }
}

/// What a successful [`Backend::git`] call printed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitOutput {
    /// The exit code, one of the call's `ok_codes`.
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl GitOutput {
    /// stdout as UTF-8 (git prints paths as UTF-8 with `core.quotepath=off`), or `None` when it
    /// isn't.
    pub fn stdout_utf8(&self) -> Option<&str> {
        std::str::from_utf8(&self.stdout).ok()
    }
}

/// Which parts of the repo a [`Backend::watch`] reports on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchSpec {
    /// The worktree, minus ignored paths and the git dir.
    pub worktree: bool,
    /// The review store, `<git-common-dir>/dv/`.
    pub store: bool,
    /// The git state a target resolves against: this worktree's `HEAD` and index, and the
    /// repo's refs (`refs/**` except `refs/dv/**`, `packed-refs`), reported as
    /// [`WatchEvent::git_changed`] (PLAN §5.3). A `working` target's old side moves when the
    /// agent commits without touching a worktree file, and a `staged` target lives in the index.
    #[serde(default)]
    pub git: bool,
}

impl WatchSpec {
    pub const ALL: Self = Self {
        worktree: true,
        store: true,
        git: true,
    };
    pub const WORKTREE: Self = Self {
        worktree: true,
        store: false,
        git: false,
    };
    pub const STORE: Self = Self {
        worktree: false,
        store: true,
        git: false,
    };
    pub const GIT: Self = Self {
        worktree: false,
        store: false,
        git: true,
    };
}

/// One coalesced batch of changes (PLAN §5.3): the paths touched since the previous batch, each
/// once, sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchEvent {
    /// Worktree paths, relative to the worktree root, `/`-separated.
    pub worktree_paths: Vec<String>,
    /// Store paths, relative to the store root (the same paths the `store_*` methods take).
    pub store_paths: Vec<String>,
    /// The git state changed ([`WatchSpec::git`]): `HEAD` or the index was replaced, or a ref
    /// outside `refs/dv/` moved. Only final names count; a `*.lock` that comes and goes doesn't.
    #[serde(default)]
    pub git_changed: bool,
    /// The OS reported that events may have been lost (a watcher buffer overflow, a watch
    /// error): consumers refresh everything instead of trusting the path lists.
    pub rescan: bool,
}

/// Receives [`WatchEvent`]s on the watch's background thread. It should return quickly (hand the
/// event to a channel); the next batch waits for it.
pub type WatchSink = Box<dyn FnMut(WatchEvent) + Send + 'static>;

/// Keeps a watch running. Dropping it stops the watch: once `drop` returns, the sink is never
/// called again and has itself been dropped (except when the handle is dropped from inside the
/// sink, where the watch stops after the sink returns).
pub struct WatchHandle {
    _guard: Box<dyn Send + Sync>,
    worktree_error: Option<String>,
}

impl WatchHandle {
    /// A handle whose drop runs `guard`'s drop, which is what stops the watch.
    pub fn new(guard: impl Send + Sync + 'static) -> Self {
        Self {
            _guard: Box::new(guard),
            worktree_error: None,
        }
    }

    /// Marks a watch that asked for the store and the worktree or git state but runs for the
    /// store alone, because the OS couldn't watch the rest (`message` says why).
    pub fn with_worktree_error(mut self, message: String) -> Self {
        self.worktree_error = Some(message);
        self
    }

    /// Why neither the worktree nor the git state is watched, when a watch that also asked for
    /// the store could only watch the store (on Linux, typically inotify's per-user watch
    /// limit, PLAN §5.3). No worktree paths and no `git_changed` arrive then; the caller
    /// refreshes those some other way.
    pub fn worktree_error(&self) -> Option<&str> {
        self.worktree_error.as_deref()
    }
}

impl fmt::Debug for WatchHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatchHandle")
            .field("worktree_error", &self.worktree_error)
            .finish_non_exhaustive()
    }
}

/// Why a backend operation failed.
///
/// `Display` is a one-line summary fit for a banner headline; the failed command and git's
/// stderr are in the fields and in [`BackendError::detail`], for a disclosure or "Copy
/// diagnostics" (DESIGN §4.4).
///
/// Every field is plain data (an [`io::Error`] aside, which is its kind and message), so the
/// remote backend rebuilds the same variant on the client's side of the wire, reading the same
/// (`dv_backend::proto::WireError`, PLAN §5.1).
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// The location's backend can't run here, or can't be reached: a WSL location without the
    /// in-distro backend, a connection to it that failed for good.
    #[error("{location} can't be opened: {reason}")]
    Unavailable { location: String, reason: String },
    /// The connection to `dv backend --stdio` died while the call was in flight, or is being
    /// re-established (PLAN §5.2). `location` names where that dv runs (the distro).
    #[error("lost the connection to dv in {location}: {reason}")]
    Disconnected { location: String, reason: String },
    #[error("no such directory: {path}")]
    PathNotFound { path: String },
    #[error("not a git repository: {path}")]
    NotARepository { path: String },
    #[error("{path} is a bare repository; dv reviews a working tree")]
    BareRepository { path: String },
    #[error("{path} is inside a git directory, not a working tree")]
    InsideGitDir { path: String },
    /// git refuses a repository owned by another user until it is listed in `safe.directory`.
    #[error("git refuses to open {path}: it is owned by another user (git config safe.directory)")]
    UnsafeRepository { path: String },
    #[error("couldn't start git: {source}")]
    Spawn {
        args: Vec<String>,
        #[source]
        source: io::Error,
    },
    #[error("git {} failed ({})", subcommand(.args), exit_label(*.code))]
    Git {
        args: Vec<String>,
        /// `None` when git was killed by a signal.
        code: Option<i32>,
        stderr: String,
    },
    #[error("git {} timed out after {} ms", subcommand(.args), .timeout_ms)]
    Timeout { args: Vec<String>, timeout_ms: u64 },
    #[error("unexpected output from git {}: {detail}", subcommand(.args))]
    UnexpectedOutput { args: Vec<String>, detail: String },
    #[error("invalid path {path:?}: {reason}")]
    InvalidPath {
        path: String,
        reason: Cow<'static, str>,
    },
    #[error("invalid store line: {reason}")]
    InvalidLine { reason: Cow<'static, str> },
    #[error("{op} {path} failed: {source}")]
    Io {
        op: Cow<'static, str>,
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("watching {path} failed: {message}")]
    Watch { path: String, message: String },
    /// A [`ProcessSpec`] the backend won't run: a program that isn't an absolute path, a batch
    /// file on Windows, NUL in an argument (PLAN §18.3).
    #[error("can't run {program}: {reason}")]
    InvalidProcess {
        program: String,
        reason: Cow<'static, str>,
    },
    /// [`Backend::spawn_stdio`] couldn't start the program (no such file, not executable).
    #[error("couldn't start {program}: {source}")]
    StartProcess {
        program: String,
        #[source]
        source: io::Error,
    },
}

impl BackendError {
    /// The diagnostics behind the headline, for errors that have more: the git command line, its
    /// exit code and its stderr.
    pub fn detail(&self) -> Option<String> {
        let command = |args: &[String]| format!("$ git {}", args.join(" "));
        match self {
            BackendError::Spawn { args, source } => Some(format!("{}\n{source}", command(args))),
            BackendError::Git { args, code, stderr } => Some(format!(
                "{}\n{}\n{}",
                command(args),
                exit_label(*code),
                stderr.trim_end()
            )),
            BackendError::Timeout { args, .. } => Some(command(args)),
            BackendError::UnexpectedOutput { args, detail } => {
                Some(format!("{}\n{detail}", command(args)))
            }
            _ => None,
        }
    }

    pub(crate) fn io(op: &'static str, path: &std::path::Path, source: io::Error) -> Self {
        BackendError::Io {
            op: op.into(),
            path: path.display().to_string(),
            source,
        }
    }
}

/// The git subcommand in `args`: the first argument that isn't an option (`git -c x=y diff …` →
/// `diff`).
fn subcommand(args: &[String]) -> &str {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-c" | "-C" => {
                iter.next();
            }
            arg if arg.starts_with('-') => {}
            arg => return arg,
        }
    }
    "command"
}

fn exit_label(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("exit code {code}"),
        None => "killed by a signal".to_string(),
    }
}

#[cfg(test)]
mod tests;
