//! The wire protocol between the app and `dv backend --stdio` (docs/PLAN.md §5.1).
//!
//! JSON, one object per line (`\n`), UTF-8, in both directions over the server's stdin and
//! stdout:
//!
//! - A request is `{"id": <u64>, "op": "<name>", …args}`: a [`Request`], the [`Op`] flattened
//!   into the envelope. The client's first request is `hello`.
//! - A response is `{"id": n, "ok": <value>}` or `{"id": n, "err": <WireError>}`. The server
//!   runs calls concurrently and answers in whatever order they finish; the client matches ids.
//! - A watch batch is `{"watch": <u32>, "event": <WatchEvent>}`, for the watch the `watch` op
//!   started, interleaved with the responses.
//! - A process's output is `{"proc": <u32>, "out": <base64>}` (a chunk of its stdout),
//!   `{"proc": n, "err": <line>}` (a line of its stderr) and, last, `{"proc": n, "exit": <code
//!   or null>}`, for the process the `spawn` op started (PLAN §18.3), interleaved the same way
//!   and in order per process.
//!
//! Every byte field (git's stdin, stdout and stderr, file contents, store lines and bytes) is
//! base64 (the standard alphabet) in the dedicated wire types here ([`base64_bytes`],
//! [`base64_opt`], [`Bytes`]): `dv_core`'s `GitOpts` and `GitOutput` would serialise their
//! bytes as JSON number arrays, so they never cross as they are. The `dv_core` types without
//! bytes (`RepoInfo`, `WatchSpec`, `WatchEvent`, `TextStats`, `ProcessSpec`) do.
//!
//! Errors cross as [`WireError`], which carries every `BackendError` variant with its fields,
//! so the client rebuilds the same variant and its `Display` and `detail()` read the same on
//! both sides; plus `protocol`, the server's answer to a request it can't read (with id 0 when
//! the line has none), after which it keeps serving.

use std::borrow::Cow;
use std::io;

use dv_core::backend::{
    BackendError, GitOpts, GitOutput, PathProbe, ProcessSpec, RepoInfo, WatchEvent, WatchSpec,
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// The protocol's version, which `hello` exchanges. Both ends come from one build (PLAN §5.2),
/// so a mismatch means a stale binary, never something to negotiate. 2 added processes
/// (`spawn`, PLAN §18.3).
pub const PROTO: u32 = 2;

/// One request line: the id the response carries, and the operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub op: Op,
}

/// The operations: `hello`, `open` and `close`, one per `Backend` method on an open repository
/// (`repo` is the handle `open` returned), `unwatch` for a watch `watch` started, and
/// `proc_write` and `proc_kill` for a process `spawn` started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// The client's first request. `build` is the build hash the client expects the server to
    /// be (PLAN §5.2), checked when given. Answered with a [`HelloReply`].
    Hello {
        proto: u32,
        #[serde(default)]
        build: Option<String>,
    },
    /// Opens a `LocalBackend` on `path` with `env` as its environment overlay
    /// (`LocalBackendOptions::env`: `(name, Some(value))` sets, `(name, None)` removes). Answered
    /// with an [`OpenReply`].
    Open {
        path: String,
        #[serde(default)]
        env: Vec<(String, Option<String>)>,
    },
    /// Forgets a repository handle. Watches started through it keep running until unwatched.
    Close { repo: u32 },
    /// `Backend::git`. Answered with a [`WireGitOutput`].
    Git {
        repo: u32,
        args: Vec<String>,
        #[serde(default)]
        opts: WireGitOpts,
    },
    /// `Backend::worktree_read`: `Option<Bytes>`.
    WorktreeRead { repo: u32, rel: String },
    /// `Backend::worktree_len`: `Option<u64>`.
    WorktreeLen { repo: u32, rel: String },
    /// `Backend::worktree_text_stats`: `Option<TextStats>`.
    WorktreeTextStats { repo: u32, rel: String },
    /// `Backend::probe_path`: a [`WirePathProbe`].
    ProbePath { repo: u32, path: String },
    /// `Backend::store_append`: `null`.
    StoreAppend {
        repo: u32,
        rel: String,
        #[serde(with = "base64_bytes")]
        line: Vec<u8>,
    },
    /// `Backend::store_read`: `Option<Bytes>`.
    StoreRead { repo: u32, rel: String },
    /// `Backend::store_list`: the names.
    StoreList { repo: u32, dir: String },
    /// `Backend::store_write_atomic`: `null`.
    StoreWriteAtomic {
        repo: u32,
        rel: String,
        #[serde(with = "base64_bytes")]
        bytes: Vec<u8>,
    },
    /// `Backend::store_remove`: whether there was a file.
    StoreRemove { repo: u32, rel: String },
    /// `Backend::copy_index_to_store`: whether the worktree had an index.
    CopyIndexToStore { repo: u32, rel: String },
    /// `Backend::watch`: a [`WatchReply`], then `{"watch", "event"}` lines until `unwatch`.
    Watch { repo: u32, spec: WatchSpec },
    /// Stops a watch: `null`, also for a watch that already stopped.
    Unwatch { watch: u32 },
    /// `Backend::spawn_stdio`: a [`SpawnReply`], then `{"proc", "out" | "err" | "exit"}` lines
    /// until its exit.
    Spawn { repo: u32, spec: ProcessSpec },
    /// Writes to a process's stdin: `null` once written; an `io` error once it has ended.
    ProcWrite {
        #[serde(rename = "proc")]
        process: u32,
        #[serde(with = "base64_bytes")]
        bytes: Vec<u8>,
    },
    /// Kills a process and everything it started: `null`, also for one that already ended. Its
    /// exit line follows.
    ProcKill {
        #[serde(rename = "proc")]
        process: u32,
    },
}

impl Op {
    /// The operation's wire name, for diagnostics.
    pub fn name(&self) -> &'static str {
        match self {
            Op::Hello { .. } => "hello",
            Op::Open { .. } => "open",
            Op::Close { .. } => "close",
            Op::Git { .. } => "git",
            Op::WorktreeRead { .. } => "worktree_read",
            Op::WorktreeLen { .. } => "worktree_len",
            Op::WorktreeTextStats { .. } => "worktree_text_stats",
            Op::ProbePath { .. } => "probe_path",
            Op::StoreAppend { .. } => "store_append",
            Op::StoreRead { .. } => "store_read",
            Op::StoreList { .. } => "store_list",
            Op::StoreWriteAtomic { .. } => "store_write_atomic",
            Op::StoreRemove { .. } => "store_remove",
            Op::CopyIndexToStore { .. } => "copy_index_to_store",
            Op::Watch { .. } => "watch",
            Op::Unwatch { .. } => "unwatch",
            Op::Spawn { .. } => "spawn",
            Op::ProcWrite { .. } => "proc_write",
            Op::ProcKill { .. } => "proc_kill",
        }
    }
}

/// `hello`'s answer: who the server is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloReply {
    /// The server's [`PROTO`].
    pub proto: u32,
    /// Whether the server's own executable hashes to the `build` the client sent; `None` when
    /// it sent none (PLAN §5.2).
    pub build_ok: Option<bool>,
    pub pid: u32,
    /// `std::env::consts::OS` and `ARCH` of the server.
    pub os: String,
    pub arch: String,
    /// The server's dv version.
    pub version: String,
}

/// `open`'s answer: the handle every later op names the repository by, and what the server's
/// `LocalBackend` resolved (its `location` is the server's local path; the client rewrites it
/// for a WSL distro).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenReply {
    pub repo: u32,
    pub info: RepoInfo,
}

/// `watch`'s answer: the id its batches carry, and why the worktree and the git state aren't
/// watched when only the store could be (`WatchHandle::worktree_error`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchReply {
    pub watch: u32,
    pub worktree_error: Option<String>,
}

/// `probe_path`'s answer: a `PathProbe`, a file's bytes base64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WirePathProbe {
    Missing,
    File {
        len: u64,
        #[serde(default)]
        contents: Option<Bytes>,
    },
    Dir,
    Link {
        target: String,
    },
}

impl From<PathProbe> for WirePathProbe {
    fn from(probe: PathProbe) -> Self {
        match probe {
            PathProbe::Missing => Self::Missing,
            PathProbe::File { len, contents } => Self::File {
                len,
                contents: contents.map(Bytes),
            },
            PathProbe::Dir => Self::Dir,
            PathProbe::Link { target } => Self::Link { target },
        }
    }
}

impl From<WirePathProbe> for PathProbe {
    fn from(probe: WirePathProbe) -> Self {
        match probe {
            WirePathProbe::Missing => Self::Missing,
            WirePathProbe::File { len, contents } => Self::File {
                len,
                contents: contents.map(|bytes| bytes.0),
            },
            WirePathProbe::Dir => Self::Dir,
            WirePathProbe::Link { target } => Self::Link { target },
        }
    }
}

/// `spawn`'s answer: the id its output lines carry, and the process's id where it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnReply {
    #[serde(rename = "proc")]
    pub process: u32,
    pub pid: u32,
}

/// `GitOpts` on the wire: its stdin base64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireGitOpts {
    #[serde(default, with = "base64_opt")]
    pub stdin: Option<Vec<u8>>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default = "ok_codes_default")]
    pub ok_codes: Vec<i32>,
    #[serde(default)]
    pub index_file: Option<String>,
}

fn ok_codes_default() -> Vec<i32> {
    vec![0]
}

impl Default for WireGitOpts {
    fn default() -> Self {
        GitOpts::default().into()
    }
}

impl From<GitOpts> for WireGitOpts {
    fn from(opts: GitOpts) -> Self {
        Self {
            stdin: opts.stdin,
            timeout_ms: opts.timeout_ms,
            ok_codes: opts.ok_codes,
            index_file: opts.index_file,
        }
    }
}

impl From<WireGitOpts> for GitOpts {
    fn from(opts: WireGitOpts) -> Self {
        Self {
            stdin: opts.stdin,
            timeout_ms: opts.timeout_ms,
            ok_codes: opts.ok_codes,
            index_file: opts.index_file,
        }
    }
}

/// `GitOutput` on the wire: its stdout and stderr base64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireGitOutput {
    pub code: i32,
    #[serde(with = "base64_bytes")]
    pub stdout: Vec<u8>,
    #[serde(with = "base64_bytes")]
    pub stderr: Vec<u8>,
}

impl From<GitOutput> for WireGitOutput {
    fn from(output: GitOutput) -> Self {
        Self {
            code: output.code,
            stdout: output.stdout,
            stderr: output.stderr,
        }
    }
}

impl From<WireGitOutput> for GitOutput {
    fn from(output: WireGitOutput) -> Self {
        Self {
            code: output.code,
            stdout: output.stdout,
            stderr: output.stderr,
        }
    }
}

/// Bytes as one base64 string: the value of `worktree_read` and `store_read` (`Option<Bytes>`,
/// `null` for a missing file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Bytes(#[serde(with = "base64_bytes")] pub Vec<u8>);

/// A `BackendError` on the wire, every variant with its fields as strings and numbers (an
/// `io::Error` as its kind and message), so [`WireError::into_backend`] rebuilds the variant
/// the server had; plus [`WireError::Protocol`], a request the server couldn't read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireError {
    Unavailable {
        location: String,
        reason: String,
    },
    Disconnected {
        location: String,
        reason: String,
    },
    PathNotFound {
        path: String,
    },
    NotARepository {
        path: String,
    },
    BareRepository {
        path: String,
    },
    InsideGitDir {
        path: String,
    },
    UnsafeRepository {
        path: String,
    },
    Spawn {
        args: Vec<String>,
        source: WireIoError,
    },
    Git {
        args: Vec<String>,
        code: Option<i32>,
        stderr: String,
    },
    Timeout {
        args: Vec<String>,
        timeout_ms: u64,
    },
    UnexpectedOutput {
        args: Vec<String>,
        detail: String,
    },
    InvalidPath {
        path: String,
        reason: String,
    },
    InvalidLine {
        reason: String,
    },
    Io {
        op: String,
        path: String,
        source: WireIoError,
    },
    Watch {
        path: String,
        message: String,
    },
    InvalidProcess {
        program: String,
        reason: String,
    },
    StartProcess {
        program: String,
        source: WireIoError,
    },
    /// The server couldn't read or run the request as sent (a malformed line, an unknown op, a
    /// repository or watch handle it doesn't have): a bug or a version skew, never the repo.
    Protocol {
        message: String,
    },
}

/// An `io::Error` on the wire: its kind (`ErrorKind`'s name, `NotFound`) and its `Display`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireIoError {
    pub kind: String,
    pub message: String,
}

impl WireError {
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol {
            message: message.into(),
        }
    }

    /// The `BackendError` this stands for. `location` names the server's side (the distro) for
    /// a [`WireError::Protocol`], which becomes [`BackendError::Unavailable`]: the repository
    /// can't be worked on through a server that doesn't understand the request.
    pub fn into_backend(self, location: &str) -> BackendError {
        match self {
            WireError::Unavailable { location, reason } => {
                BackendError::Unavailable { location, reason }
            }
            WireError::Disconnected { location, reason } => {
                BackendError::Disconnected { location, reason }
            }
            WireError::PathNotFound { path } => BackendError::PathNotFound { path },
            WireError::NotARepository { path } => BackendError::NotARepository { path },
            WireError::BareRepository { path } => BackendError::BareRepository { path },
            WireError::InsideGitDir { path } => BackendError::InsideGitDir { path },
            WireError::UnsafeRepository { path } => BackendError::UnsafeRepository { path },
            WireError::Spawn { args, source } => BackendError::Spawn {
                args,
                source: source.into(),
            },
            WireError::Git { args, code, stderr } => BackendError::Git { args, code, stderr },
            WireError::Timeout { args, timeout_ms } => BackendError::Timeout { args, timeout_ms },
            WireError::UnexpectedOutput { args, detail } => {
                BackendError::UnexpectedOutput { args, detail }
            }
            WireError::InvalidPath { path, reason } => BackendError::InvalidPath {
                path,
                reason: Cow::Owned(reason),
            },
            WireError::InvalidLine { reason } => BackendError::InvalidLine {
                reason: Cow::Owned(reason),
            },
            WireError::Io { op, path, source } => BackendError::Io {
                op: Cow::Owned(op),
                path,
                source: source.into(),
            },
            WireError::Watch { path, message } => BackendError::Watch { path, message },
            WireError::InvalidProcess { program, reason } => BackendError::InvalidProcess {
                program,
                reason: Cow::Owned(reason),
            },
            WireError::StartProcess { program, source } => BackendError::StartProcess {
                program,
                source: source.into(),
            },
            WireError::Protocol { message } => BackendError::Unavailable {
                location: location.to_string(),
                reason: format!("dv there couldn't run the request: {message}"),
            },
        }
    }
}

impl From<&BackendError> for WireError {
    fn from(error: &BackendError) -> Self {
        match error {
            BackendError::Unavailable { location, reason } => WireError::Unavailable {
                location: location.clone(),
                reason: reason.clone(),
            },
            BackendError::Disconnected { location, reason } => WireError::Disconnected {
                location: location.clone(),
                reason: reason.clone(),
            },
            BackendError::PathNotFound { path } => WireError::PathNotFound { path: path.clone() },
            BackendError::NotARepository { path } => {
                WireError::NotARepository { path: path.clone() }
            }
            BackendError::BareRepository { path } => {
                WireError::BareRepository { path: path.clone() }
            }
            BackendError::InsideGitDir { path } => WireError::InsideGitDir { path: path.clone() },
            BackendError::UnsafeRepository { path } => {
                WireError::UnsafeRepository { path: path.clone() }
            }
            BackendError::Spawn { args, source } => WireError::Spawn {
                args: args.clone(),
                source: source.into(),
            },
            BackendError::Git { args, code, stderr } => WireError::Git {
                args: args.clone(),
                code: *code,
                stderr: stderr.clone(),
            },
            BackendError::Timeout { args, timeout_ms } => WireError::Timeout {
                args: args.clone(),
                timeout_ms: *timeout_ms,
            },
            BackendError::UnexpectedOutput { args, detail } => WireError::UnexpectedOutput {
                args: args.clone(),
                detail: detail.clone(),
            },
            BackendError::InvalidPath { path, reason } => WireError::InvalidPath {
                path: path.clone(),
                reason: reason.to_string(),
            },
            BackendError::InvalidLine { reason } => WireError::InvalidLine {
                reason: reason.to_string(),
            },
            BackendError::Io { op, path, source } => WireError::Io {
                op: op.to_string(),
                path: path.clone(),
                source: source.into(),
            },
            BackendError::Watch { path, message } => WireError::Watch {
                path: path.clone(),
                message: message.clone(),
            },
            BackendError::InvalidProcess { program, reason } => WireError::InvalidProcess {
                program: program.clone(),
                reason: reason.to_string(),
            },
            BackendError::StartProcess { program, source } => WireError::StartProcess {
                program: program.clone(),
                source: source.into(),
            },
        }
    }
}

impl From<&io::Error> for WireIoError {
    fn from(error: &io::Error) -> Self {
        Self {
            kind: format!("{:?}", error.kind()),
            message: error.to_string(),
        }
    }
}

impl From<WireIoError> for io::Error {
    /// An error of the same kind whose `Display` is the original's (an OS error's code and
    /// all, as its message).
    fn from(error: WireIoError) -> Self {
        io::Error::new(error_kind(&error.kind), error.message)
    }
}

/// The `ErrorKind` named `name` (its `Debug` spelling), `Other` for a kind this build doesn't
/// know.
fn error_kind(name: &str) -> io::ErrorKind {
    use io::ErrorKind::*;
    const KINDS: &[io::ErrorKind] = &[
        NotFound,
        PermissionDenied,
        ConnectionRefused,
        ConnectionReset,
        HostUnreachable,
        NetworkUnreachable,
        ConnectionAborted,
        NotConnected,
        AddrInUse,
        AddrNotAvailable,
        NetworkDown,
        BrokenPipe,
        AlreadyExists,
        WouldBlock,
        NotADirectory,
        IsADirectory,
        DirectoryNotEmpty,
        ReadOnlyFilesystem,
        StaleNetworkFileHandle,
        InvalidInput,
        InvalidData,
        TimedOut,
        WriteZero,
        StorageFull,
        NotSeekable,
        QuotaExceeded,
        FileTooLarge,
        ResourceBusy,
        ExecutableFileBusy,
        Deadlock,
        CrossesDevices,
        TooManyLinks,
        InvalidFilename,
        ArgumentListTooLong,
        Interrupted,
        Unsupported,
        UnexpectedEof,
        OutOfMemory,
        Other,
    ];
    KINDS
        .iter()
        .copied()
        .find(|kind| format!("{kind:?}") == name)
        .unwrap_or(Other)
}

/// A response line as the client reads it: a reply (`id` with `ok` or `err`), a watch batch
/// (`watch` and `event`) or a process's output (`proc` with `out`, `err` or `exit`). `ok`
/// stays unparsed until the waiting call parses it into the type it expects; `null` and an
/// absent `ok` are both `None`. `err` is a [`WireError`] in a reply and a stderr line in a
/// process's output, so it is parsed once the line's kind is known.
#[derive(Debug, Deserialize)]
pub struct Incoming {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub ok: Option<Box<RawValue>>,
    #[serde(default)]
    pub err: Option<Box<RawValue>>,
    #[serde(default)]
    pub watch: Option<u32>,
    #[serde(default)]
    pub event: Option<WatchEvent>,
    #[serde(default, rename = "proc")]
    pub process: Option<u32>,
    #[serde(default, with = "base64_opt")]
    pub out: Option<Vec<u8>>,
    /// `Some(None)` for `"exit": null` (a signal killed it, or it ended with its server),
    /// `None` when the line has no `exit`.
    #[serde(default, deserialize_with = "present")]
    pub exit: Option<Option<i32>>,
}

/// A field that is there, `null` included, as `Some` of its value; with `#[serde(default)]` an
/// absent one is `None`.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// The line answering request `id` with `value` (`\n`-terminated).
pub fn ok_line<T: Serialize + ?Sized>(id: u64, value: &T) -> Vec<u8> {
    #[derive(Serialize)]
    struct Line<'a, T: ?Sized> {
        id: u64,
        ok: &'a T,
    }
    to_line(&Line { id, ok: value }, id)
}

/// The line answering request `id` with `error` (`\n`-terminated).
pub fn err_line(id: u64, error: &WireError) -> Vec<u8> {
    #[derive(Serialize)]
    struct Line<'a> {
        id: u64,
        err: &'a WireError,
    }
    to_line(&Line { id, err: error }, id)
}

/// The line carrying a batch of watch `watch` (`\n`-terminated).
pub fn watch_line(watch: u32, event: &WatchEvent) -> Vec<u8> {
    #[derive(Serialize)]
    struct Line<'a> {
        watch: u32,
        event: &'a WatchEvent,
    }
    to_line(&Line { watch, event }, 0)
}

/// The line carrying a chunk of process `process`'s stdout (`\n`-terminated).
pub fn proc_out_line(process: u32, bytes: &[u8]) -> Vec<u8> {
    #[derive(Serialize)]
    struct Line<'a> {
        #[serde(rename = "proc")]
        process: u32,
        #[serde(with = "base64_bytes")]
        out: &'a [u8],
    }
    to_line(
        &Line {
            process,
            out: bytes,
        },
        0,
    )
}

/// The line carrying a line of process `process`'s stderr (`\n`-terminated).
pub fn proc_err_line(process: u32, line: &str) -> Vec<u8> {
    #[derive(Serialize)]
    struct Line<'a> {
        #[serde(rename = "proc")]
        process: u32,
        err: &'a str,
    }
    to_line(&Line { process, err: line }, 0)
}

/// The line saying process `process` exited with `code`, `null` when a signal killed it
/// (`\n`-terminated).
pub fn proc_exit_line(process: u32, code: Option<i32>) -> Vec<u8> {
    #[derive(Serialize)]
    struct Line {
        #[serde(rename = "proc")]
        process: u32,
        exit: Option<i32>,
    }
    to_line(
        &Line {
            process,
            exit: code,
        },
        0,
    )
}

/// `value` as one `\n`-terminated JSON line. JSON escapes every newline inside strings, so the
/// only one is the terminator. Serialising the protocol's own types can't fail (no maps with
/// non-string keys, no failing `Serialize`); should it, the line is a protocol error for `id`.
fn to_line<T: Serialize>(value: &T, id: u64) -> Vec<u8> {
    let mut line = serde_json::to_vec(value).unwrap_or_else(|error| {
        let error = WireError::protocol(format!("couldn't encode the answer: {error}"));
        serde_json::to_vec(&serde_json::json!({ "id": id, "err": error })).unwrap_or_else(|_| {
            br#"{"id":0,"err":{"kind":"protocol","message":"encoding"}}"#.to_vec()
        })
    });
    line.push(b'\n');
    line
}

/// `#[serde(with = "base64_bytes")]`: `Vec<u8>` as one base64 string (standard alphabet,
/// padded).
pub mod base64_bytes {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize as _, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD
            .decode(text.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

/// `#[serde(with = "base64_opt")]`: `Option<Vec<u8>>` as `null` or one base64 string.
pub mod base64_opt {
    use serde::{Deserialize as _, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        bytes: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => super::base64_bytes::serialize(bytes, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<super::Bytes>::deserialize(deserializer).map(|bytes| bytes.map(|b| b.0))
    }
}

#[cfg(test)]
mod tests;
