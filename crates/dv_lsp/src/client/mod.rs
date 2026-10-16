//! The LSP client: one language server, run where the repository lives through
//! `Backend::spawn_stdio` (docs/PLAN.md §18.3, §18.4).
//!
//! [`LspClient::start`] starts the server, says `initialize` (offering UTF-8 and UTF-16
//! positions) and `initialized`, and returns once the server is ready. Its stdout reaches the
//! client on the process's own thread ([`dispatch`]): answers go to the request waiting for
//! them, the server's own requests are answered there (`workspace/configuration` from the spec's
//! settings, `client/registerCapability` recorded, `window/workDoneProgress/create` and
//! `$/progress` kept as [`Progress`]), and the client's [`ClientEvent`]s report its status,
//! progress and stderr. Everything the client writes goes through one writer thread, so neither
//! a caller nor the process's thread ever waits on the server's stdin.
//!
//! Requests ([`LspClient::hover`], [`LspClient::definition`], [`LspClient::references`]) return
//! a [`Pending`] whose [`Pending::wait`] blocks until the answer or the request's timeout (hover
//! 5 s, definition 10 s, references 30 s, PLAN §18.5); dropping it unanswered cancels it. A
//! request is about an open document ([`LspClient::open_document`]), one version of a path at a
//! time (PLAN §18.2). Positions are UTF-8 byte columns both ways; a server that counts UTF-16
//! is converted for with the lines' text.
//!
//! [`LspClient::stop`] says `shutdown` and `exit` and kills the server after a grace period;
//! dropping the client kills it at once.

mod answers;
mod dispatch;
mod pending;
mod watchers;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dv_core::backend::{Backend, BackendError, ProcessEvent, ProcessHandle, ProcessSink};
use lsp_types::Hover;
use parking_lot::{Condvar, Mutex};
use serde_json::{Value, json};

pub use answers::{HoverInfo, HoverPart, SymbolLocation};
pub use pending::{Canceller, Pending, RequestError};
pub use watchers::{CHANGE, CREATE, DELETE};

use crate::framing;
use crate::paths::{UriTarget, WorkspacePaths};
use crate::resolve::ServerCommand;
use crate::text::{LineIndex, PositionEncoding, TextPosition, TextRange, line_of};
use pending::Reply;
use watchers::Watcher;

/// How many of the server's last stderr lines the client keeps (the status bar's failure
/// details, PLAN §18.5).
pub const STDERR_KEPT: usize = 50;

/// How long each step may take (PLAN §18.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// `initialize`'s answer.
    pub initialize: Duration,
    pub hover: Duration,
    pub definition: Duration,
    pub references: Duration,
    /// How long [`LspClient::stop`] waits for `shutdown`'s answer, then for the exit, before it
    /// kills the server.
    pub stop_grace: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            initialize: Duration::from_secs(30),
            hover: Duration::from_secs(5),
            definition: Duration::from_secs(10),
            references: Duration::from_secs(30),
            stop_grace: Duration::from_secs(2),
        }
    }
}

/// What `initialize` and `workspace/configuration` tell a server, from its spec (PLAN §18.4).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerConfig {
    /// The directory the server serves, worktree-relative (`""` is the worktree root).
    pub root: String,
    pub initialization_options: Value,
    /// What `workspace/configuration` answers from, by section (`typescript.preferences` is
    /// `settings["typescript"]["preferences"]`).
    pub settings: Value,
}

/// Where a server is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerStatus {
    /// Started; `initialize` not yet answered.
    Starting,
    Ready,
    /// The process ended by itself, with its exit code (`None` for a signal or a lost
    /// connection): a crash, which the owner restarts (PLAN §18.5).
    Exited(Option<i32>),
    /// The process ended after [`LspClient::stop`] asked it to, or the client was dropped:
    /// nothing to restart. Its exit code says little (TypeScript's `tsc` exits 1 even after a
    /// proper `shutdown` and `exit`).
    Stopped(Option<i32>),
}

impl ServerStatus {
    /// Whether the process has ended, either way.
    pub fn has_ended(self) -> bool {
        matches!(self, ServerStatus::Exited(_) | ServerStatus::Stopped(_))
    }
}

/// Work a server reports (`$/progress` with a `begin`, PLAN §18.5's "loading").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub token: String,
    pub title: String,
    pub message: Option<String>,
    pub percentage: Option<u32>,
}

/// What a client reports as it happens, on the server process's thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    Status(ServerStatus),
    /// The work in progress now (empty when it has all ended).
    Progress(Vec<Progress>),
    Stderr(String),
}

/// Receives a client's [`ClientEvent`]s. It should return quickly; it may call the client.
pub type EventSink = Arc<dyn Fn(ClientEvent) + Send + Sync>;

/// Why a server didn't become ready.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error(transparent)]
    Spawn(#[from] BackendError),
    #[error("the language server ended while starting ({})", exit_label(*.code))]
    Exited {
        code: Option<i32>,
        /// Its last stderr lines.
        stderr: Vec<String>,
    },
    #[error("the language server didn't answer initialize within {} s", .0.as_secs())]
    Timeout(Duration),
    #[error("the language server refused initialize: {0}")]
    Refused(String),
}

fn exit_label(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("exit code {code}"),
        None => "killed".to_string(),
    }
}

/// A running language server (module docs).
pub struct LspClient {
    shared: Arc<Shared>,
    handle: Arc<ProcessHandle>,
    timeouts: Timeouts,
}

/// What the client, its requests and the process's thread share.
pub(crate) struct Shared {
    pub(crate) state: Mutex<State>,
    /// Signalled when the status changes (to `Ready`, or to its end).
    status_changed: Condvar,
    events: EventSink,
    /// The writer thread's queue; `None` once the client is dropped.
    outgoing: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    pub(crate) paths: WorkspacePaths,
    config: ServerConfig,
    /// Reads a worktree file's text, for converting a UTF-16 server's positions in files that
    /// aren't open.
    files: Option<Arc<dyn Backend>>,
}

pub(crate) struct State {
    pub(crate) status: ServerStatus,
    /// [`LspClient::stop`] was called (or the client dropped): the end is a stop, not a crash.
    stop_requested: bool,
    next_id: i64,
    pub(crate) pending: HashMap<i64, mpsc::Sender<Reply>>,
    pub(crate) encoding: PositionEncoding,
    capabilities: Value,
    /// Open documents by worktree path.
    documents: HashMap<String, Document>,
    next_version: i32,
    watchers: Vec<Watcher>,
    /// Every registration's id and method.
    registrations: Vec<(String, String)>,
    progress: Vec<Progress>,
    stderr: VecDeque<String>,
}

struct Document {
    language_id: String,
    text: Arc<str>,
}

impl LspClient {
    /// Starts `command` through `backend` and initializes it; blocks until the server is ready
    /// (call it off the UI thread).
    pub fn start(
        backend: Arc<dyn Backend>,
        command: &ServerCommand,
        config: ServerConfig,
        events: EventSink,
        timeouts: Timeouts,
    ) -> Result<Self, StartError> {
        let spec = command.process_spec();
        let paths = WorkspacePaths::new(&backend.repo().toplevel);
        let spawner = backend.clone();
        Self::start_with(
            move |sink| spawner.spawn_stdio(&spec, sink),
            paths,
            Some(backend),
            config,
            events,
            timeouts,
        )
    }

    /// [`LspClient::start`] over any process: `spawn` starts it with the given sink. `files`
    /// reads worktree files for a UTF-16 server's positions (`None`: columns in files that
    /// aren't open stay as the server gave them).
    pub fn start_with(
        spawn: impl FnOnce(ProcessSink) -> Result<ProcessHandle, BackendError>,
        paths: WorkspacePaths,
        files: Option<Arc<dyn Backend>>,
        config: ServerConfig,
        events: EventSink,
        timeouts: Timeouts,
    ) -> Result<Self, StartError> {
        let (sender, receiver) = mpsc::channel::<Vec<u8>>();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                status: ServerStatus::Starting,
                stop_requested: false,
                next_id: 1,
                pending: HashMap::new(),
                encoding: PositionEncoding::Utf16,
                capabilities: Value::Null,
                documents: HashMap::new(),
                next_version: 1,
                watchers: Vec::new(),
                registrations: Vec::new(),
                progress: Vec::new(),
                stderr: VecDeque::new(),
            }),
            status_changed: Condvar::new(),
            events,
            outgoing: Mutex::new(Some(sender)),
            paths,
            config,
            files,
        });
        let handle = Arc::new(spawn(dispatch::sink(shared.clone()))?);
        // The writer is detached: a write blocked on a backend that stopped answering must
        // never hold up whoever drops the client. It ends once the queue closes (the drop) or a
        // write fails (the process ended).
        {
            let writing = handle.clone();
            std::thread::Builder::new()
                .name("dv-lsp-writer".into())
                .spawn(move || {
                    for frame in receiver {
                        if writing.write(&frame).is_err() {
                            // The process ended; its exit reaches the sink.
                            break;
                        }
                    }
                })
                .map_err(|err| {
                    handle.kill();
                    StartError::Spawn(BackendError::Io {
                        op: "starting a thread for".into(),
                        path: "the language server".into(),
                        source: err,
                    })
                })?;
        }
        let client = Self {
            shared,
            handle,
            timeouts,
        };
        client.initialize()?;
        Ok(client)
    }

    fn initialize(&self) -> Result<(), StartError> {
        let shared = &self.shared;
        let root_uri = shared.paths.uri_text(&shared.config.root);
        let name = root_name(&shared.paths.absolute(&shared.config.root));
        let params = json!({
            // The server runs where the repo lives, maybe in a WSL distro, where dv's own
            // process id means nothing; a server that watches its parent would end at once.
            "processId": null,
            "clientInfo": {"name": "dv", "version": env!("CARGO_PKG_VERSION")},
            "rootUri": root_uri,
            "workspaceFolders": [{"uri": root_uri, "name": name}],
            "initializationOptions": shared.config.initialization_options,
            "capabilities": {
                "general": {"positionEncodings": ["utf-8", "utf-16"]},
                "textDocument": {
                    "synchronization": {"dynamicRegistration": false},
                    "hover": {"contentFormat": ["markdown", "plaintext"]},
                    "definition": {"linkSupport": true},
                    "references": {}
                },
                "workspace": {
                    "configuration": true,
                    "workspaceFolders": true,
                    "didChangeWatchedFiles": {
                        "dynamicRegistration": true,
                        "relativePatternSupport": true
                    }
                },
                "window": {"workDoneProgress": true}
            },
        });
        let pending = self.request(
            "initialize",
            params,
            self.timeouts.initialize,
            |value, _| Ok(value),
        );
        let result = match pending.wait() {
            Ok(result) => result,
            Err(RequestError::Timeout { after, .. }) => return Err(StartError::Timeout(after)),
            Err(RequestError::Exited) => {
                let state = self.wait_for_exit(Duration::from_secs(1));
                return Err(StartError::Exited {
                    code: state.0,
                    stderr: state.1,
                });
            }
            Err(err) => return Err(StartError::Refused(err.to_string())),
        };
        let capabilities = result.get("capabilities").cloned().unwrap_or(Value::Null);
        let encoding = PositionEncoding::from_capability(
            capabilities.get("positionEncoding").and_then(Value::as_str),
        );
        {
            let mut state = shared.state.lock();
            state.encoding = encoding;
            state.capabilities = capabilities;
        }
        shared.notify("initialized", json!({}));
        if !shared.config.settings.is_null() {
            shared.notify(
                "workspace/didChangeConfiguration",
                json!({ "settings": shared.config.settings }),
            );
        }
        shared.set_status(ServerStatus::Ready);
        Ok(())
    }

    /// Waits up to `limit` for the process to end; its exit code and last stderr lines.
    fn wait_for_exit(&self, limit: Duration) -> (Option<i32>, Vec<String>) {
        let deadline = Instant::now() + limit;
        let mut state = self.shared.state.lock();
        while !state.status.has_ended() {
            if self
                .shared
                .status_changed
                .wait_until(&mut state, deadline)
                .timed_out()
            {
                break;
            }
        }
        let code = match state.status {
            ServerStatus::Exited(code) | ServerStatus::Stopped(code) => code,
            _ => None,
        };
        (code, state.stderr.iter().cloned().collect())
    }

    /// The server's process id where it runs.
    pub fn pid(&self) -> u32 {
        self.handle.pid()
    }

    pub fn status(&self) -> ServerStatus {
        self.shared.state.lock().status
    }

    /// The position encoding the server chose.
    pub fn encoding(&self) -> PositionEncoding {
        self.shared.state.lock().encoding
    }

    /// The server's `capabilities` from `initialize`.
    pub fn capabilities(&self) -> Value {
        self.shared.state.lock().capabilities.clone()
    }

    /// The work in progress now.
    pub fn progress(&self) -> Vec<Progress> {
        self.shared.state.lock().progress.clone()
    }

    /// The server's last stderr lines, oldest first.
    pub fn stderr_tail(&self) -> Vec<String> {
        self.shared.state.lock().stderr.iter().cloned().collect()
    }

    /// Every registration the server made (`client/registerCapability`): id and method.
    pub fn registrations(&self) -> Vec<(String, String)> {
        self.shared.state.lock().registrations.clone()
    }

    /// Opens `rel` with `text` (`didOpen`). Already open with the same text, nothing happens;
    /// open with other text (the other side of a review, PLAN §18.2, or the file changed on
    /// disk), the old version is closed first: one version of a path is open at a time.
    pub fn open_document(
        &self,
        rel: &str,
        language_id: &str,
        text: impl Into<Arc<str>>,
    ) -> Result<(), RequestError> {
        let text = text.into();
        let (close, version) = {
            let mut state = self.shared.state.lock();
            if state.status.has_ended() {
                return Err(RequestError::Exited);
            }
            let close = match state.documents.get(rel) {
                Some(open) if open.text == text && open.language_id == language_id => {
                    return Ok(());
                }
                Some(_) => true,
                None => false,
            };
            let version = state.next_version;
            state.next_version += 1;
            state.documents.insert(
                rel.to_string(),
                Document {
                    language_id: language_id.to_string(),
                    text: text.clone(),
                },
            );
            (close, version)
        };
        let uri = self.shared.paths.uri_text(rel);
        if close {
            self.shared.notify(
                "textDocument/didClose",
                json!({"textDocument": {"uri": uri}}),
            );
        }
        self.shared.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": uri,
                "languageId": language_id,
                "version": version,
                "text": &*text,
            }}),
        );
        Ok(())
    }

    /// Closes `rel` (`didClose`), if it is open.
    pub fn close_document(&self, rel: &str) {
        if self.shared.state.lock().documents.remove(rel).is_some() {
            self.shared.notify(
                "textDocument/didClose",
                json!({"textDocument": {"uri": self.shared.paths.uri_text(rel)}}),
            );
        }
    }

    /// The text `rel` is open with.
    pub fn document_text(&self, rel: &str) -> Option<Arc<str>> {
        self.shared
            .state
            .lock()
            .documents
            .get(rel)
            .map(|doc| doc.text.clone())
    }

    /// `textDocument/hover` at `at` in the open document `rel`.
    pub fn hover(&self, rel: &str, at: TextPosition) -> Pending<Option<HoverInfo>> {
        let rel_owned = rel.to_string();
        self.position_request(
            "textDocument/hover",
            rel,
            at,
            json!({}),
            self.timeouts.hover,
            {
                move |value, shared| {
                    let Some(hover) = parse::<Option<Hover>>("textDocument/hover", value)? else {
                        return Ok(None);
                    };
                    let parts = answers::hover_parts(hover.contents);
                    if parts.is_empty() {
                        return Ok(None);
                    }
                    let range = hover.range.map(|range| {
                        let target = UriTarget::Worktree(rel_owned.clone());
                        shared.convert_ranges(vec![(target, range)]).remove(0).range
                    });
                    Ok(Some(HoverInfo { parts, range }))
                }
            },
        )
    }

    /// `textDocument/definition` at `at` in the open document `rel`.
    pub fn definition(&self, rel: &str, at: TextPosition) -> Pending<Vec<SymbolLocation>> {
        self.position_request(
            "textDocument/definition",
            rel,
            at,
            json!({}),
            self.timeouts.definition,
            |value, shared| {
                Ok(shared.locations(answers::location_entries("textDocument/definition", value)))
            },
        )
    }

    /// `textDocument/references` at `at` in the open document `rel`, the declaration included
    /// when `include_declaration`.
    pub fn references(
        &self,
        rel: &str,
        at: TextPosition,
        include_declaration: bool,
    ) -> Pending<Vec<SymbolLocation>> {
        self.position_request(
            "textDocument/references",
            rel,
            at,
            json!({"context": {"includeDeclaration": include_declaration}}),
            self.timeouts.references,
            |value, shared| {
                Ok(shared.locations(answers::location_entries("textDocument/references", value)))
            },
        )
    }

    /// Tells the server about changed worktree files its watchers ask for
    /// (`workspace/didChangeWatchedFiles`): each a worktree path and [`CREATE`], [`CHANGE`] or
    /// [`DELETE`]. Returns how many it was told about.
    pub fn files_changed(&self, changes: &[(String, u8)]) -> usize {
        let paths = &self.shared.paths;
        let events: Vec<Value> = {
            let state = self.shared.state.lock();
            changes
                .iter()
                .filter(|(rel, kind)| {
                    let absolute = paths.absolute(rel).replace('\\', "/");
                    state
                        .watchers
                        .iter()
                        .any(|watcher| watcher.matches(&absolute, *kind, paths.style()))
                })
                .map(|(rel, kind)| {
                    let kind = match *kind {
                        CREATE => 1,
                        DELETE => 3,
                        _ => 2,
                    };
                    json!({"uri": paths.uri_text(rel), "type": kind})
                })
                .collect()
        };
        if !events.is_empty() {
            self.shared.notify(
                "workspace/didChangeWatchedFiles",
                json!({ "changes": events }),
            );
        }
        events.len()
    }

    /// Those of `rels` (worktree paths) a watcher the server registered asks about, for any
    /// kind of change: the paths worth telling it about ([`LspClient::files_changed`]).
    pub fn watched(&self, rels: &[String]) -> Vec<String> {
        let paths = &self.shared.paths;
        let state = self.shared.state.lock();
        rels.iter()
            .filter(|rel| {
                let absolute = paths.absolute(rel).replace('\\', "/");
                state.watchers.iter().any(|watcher| {
                    watcher.matches(&absolute, CREATE | CHANGE | DELETE, paths.style())
                })
            })
            .cloned()
            .collect()
    }

    /// Any request, its result as the server sent it.
    pub fn request_raw(&self, method: &str, params: Value, timeout: Duration) -> Pending<Value> {
        self.request(method, params, timeout, |value, _| Ok(value))
    }

    /// Any notification.
    pub fn notify(&self, method: &str, params: Value) {
        self.shared.notify(method, params);
    }

    /// Says `shutdown` then `exit`, waits for the server to end, and kills it if it hasn't
    /// within the grace period (twice [`Timeouts::stop_grace`] at most). Blocks.
    ///
    /// Returns how it ended: [`ServerStatus::Stopped`] with its exit code once it has, the
    /// events having said so too (a server killed after the grace period reads
    /// `Stopped(None)`, or the kill's own code on Windows).
    pub fn stop(self) -> ServerStatus {
        self.shared.state.lock().stop_requested = true;
        if self.status().has_ended() {
            return self.status();
        }
        let grace = self.timeouts.stop_grace;
        let _ = self.request_raw("shutdown", Value::Null, grace).wait();
        self.shared.notify("exit", Value::Null);
        self.wait_for_exit(grace);
        if !self.status().has_ended() {
            // The sink still delivers until the client is dropped, so the kill's exit is seen.
            self.handle.kill();
            self.wait_for_exit(grace);
        }
        self.status()
    }

    /// A request about a position in the open document `rel`.
    fn position_request<T: 'static>(
        &self,
        method: &str,
        rel: &str,
        at: TextPosition,
        mut params: Value,
        timeout: Duration,
        convert: impl FnOnce(Value, &Shared) -> Result<T, RequestError> + Send + 'static,
    ) -> Pending<T> {
        let position = {
            let state = self.shared.state.lock();
            let Some(doc) = state.documents.get(rel) else {
                return Pending::failed(RequestError::NotOpen(rel.to_string()));
            };
            let line = line_of(&doc.text, at.line).unwrap_or("");
            json!({
                "line": at.line,
                "character": state.encoding.column_to_server(line, at.column),
            })
        };
        if let Value::Object(map) = &mut params {
            map.insert(
                "textDocument".into(),
                json!({"uri": self.shared.paths.uri_text(rel)}),
            );
            map.insert("position".into(), position);
        }
        self.request(method, params, timeout, convert)
    }

    fn request<T: 'static>(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        convert: impl FnOnce(Value, &Shared) -> Result<T, RequestError> + Send + 'static,
    ) -> Pending<T> {
        let (sender, receiver) = mpsc::channel();
        let id = {
            let mut state = self.shared.state.lock();
            if state.status.has_ended() {
                return Pending::failed(RequestError::Exited);
            }
            let id = state.next_id;
            state.next_id += 1;
            state.pending.insert(id, sender);
            id
        };
        self.shared.send(&message(Some(id), method, params));
        Pending::sent(
            self.shared.clone(),
            id,
            method,
            receiver,
            timeout,
            Box::new(convert),
        )
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // First, so the kill's own exit, if the sink still hears it, reads as a stop.
        self.shared.state.lock().stop_requested = true;
        self.handle.kill();
        // Closing the queue ends the (detached) writer thread, which holds the other handle.
        self.shared.outgoing.lock().take();
        // The process's own exit may never reach the sink now (the handle stops delivering
        // once it is dropped), so the client ends here: a request waited on by another thread
        // fails at once rather than at its timeout.
        self.shared.set_status(ServerStatus::Stopped(None));
    }
}

impl std::fmt::Debug for LspClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspClient")
            .field("pid", &self.handle.pid())
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl Shared {
    /// Queues one message for the writer thread.
    pub(crate) fn send(&self, message: &Value) {
        let body = match serde_json::to_vec(message) {
            Ok(body) => body,
            Err(err) => {
                log::warn!("a language server message didn't serialise: {err}");
                return;
            }
        };
        if let Some(outgoing) = &*self.outgoing.lock() {
            // A closed queue is a client being dropped: nothing more is wanted.
            let _ = outgoing.send(framing::encode(&body));
        }
    }

    pub(crate) fn notify(&self, method: &str, params: Value) {
        self.send(&message(None, method, params));
    }

    pub(crate) fn emit(&self, event: ClientEvent) {
        (self.events)(event);
    }

    pub(crate) fn set_status(&self, status: ServerStatus) {
        let status = {
            let mut state = self.state.lock();
            if state.status.has_ended() {
                return;
            }
            // An end after a stop was asked for is a stop, whatever its code.
            let status = match status {
                ServerStatus::Exited(code) if state.stop_requested => ServerStatus::Stopped(code),
                other => other,
            };
            state.status = status;
            if status.has_ended() {
                // Every waiting request fails: its sender goes.
                state.pending.clear();
                state.progress.clear();
            }
            status
        };
        self.status_changed.notify_all();
        self.emit(ClientEvent::Status(status));
    }

    /// The configuration value at `section` (dot-separated) of the spec's settings; `null`
    /// when there is none, and the whole settings for no section.
    pub(crate) fn configuration(&self, section: Option<&str>) -> Value {
        let mut value = &self.config.settings;
        if let Some(section) = section.filter(|section| !section.is_empty()) {
            for key in section.split('.') {
                match value.get(key) {
                    Some(next) => value = next,
                    None => return Value::Null,
                }
            }
        }
        value.clone()
    }

    /// The `(uri, range)` pairs a server answered with, as locations with UTF-8 columns.
    fn locations(&self, raw: Vec<(String, lsp_types::Range)>) -> Vec<SymbolLocation> {
        let targets = raw
            .into_iter()
            .map(|(uri, range)| (self.paths.target(&uri), range))
            .collect();
        self.convert_ranges(targets)
    }

    /// Ranges in the server's encoding as UTF-8 ranges, reading each worktree file's lines
    /// once (an open document's text, else the file through the backend). A file outside the
    /// worktree, or one that can't be read, keeps the server's columns.
    pub(crate) fn convert_ranges(
        &self,
        ranges: Vec<(UriTarget, lsp_types::Range)>,
    ) -> Vec<SymbolLocation> {
        let encoding = self.state.lock().encoding;
        // Each file's text and its line index, built once however many ranges it has.
        let mut texts: HashMap<String, Option<(Arc<str>, LineIndex)>> = HashMap::new();
        ranges
            .into_iter()
            .map(|(target, range)| {
                let text = match (&target, encoding) {
                    (UriTarget::Worktree(rel), PositionEncoding::Utf16) => texts
                        .entry(rel.clone())
                        .or_insert_with(|| {
                            self.text_of(rel).map(|text| {
                                let index = LineIndex::new(&text);
                                (text, index)
                            })
                        })
                        .as_ref(),
                    _ => None,
                };
                let convert = |position: lsp_types::Position| {
                    let column = match text {
                        Some((text, index)) => {
                            let line = index.line(text, position.line).unwrap_or("");
                            encoding.column_from_server(line, position.character)
                        }
                        None => position.character,
                    };
                    TextPosition::new(position.line, column)
                };
                SymbolLocation {
                    range: TextRange {
                        start: convert(range.start),
                        end: convert(range.end),
                    },
                    target,
                }
            })
            .collect()
    }

    /// A worktree file's text: its open document's, else the file read through the backend.
    fn text_of(&self, rel: &str) -> Option<Arc<str>> {
        if let Some(doc) = self.state.lock().documents.get(rel) {
            return Some(doc.text.clone());
        }
        let bytes = self.files.as_ref()?.worktree_read(rel).ok()??;
        Some(Arc::from(String::from_utf8_lossy(&bytes).as_ref()))
    }

    fn push_stderr(&self, line: String) {
        {
            let mut state = self.state.lock();
            if state.stderr.len() == STDERR_KEPT {
                state.stderr.pop_front();
            }
            state.stderr.push_back(line.clone());
        }
        self.emit(ClientEvent::Stderr(line));
    }
}

/// A request (with an id) or a notification. `params` is left out when it is `null`: the
/// protocol has no `null` params, and a strict server (TypeScript's `tsc`) refuses `shutdown`
/// with them and ignores `exit` with them.
fn message(id: Option<i64>, method: &str, params: Value) -> Value {
    let mut message = json!({"jsonrpc": "2.0", "method": method});
    if let Some(id) = id {
        message["id"] = json!(id);
    }
    if !params.is_null() {
        message["params"] = params;
    }
    message
}

/// A result as `T`, or why it isn't one.
fn parse<T: serde::de::DeserializeOwned>(method: &str, value: Value) -> Result<T, RequestError> {
    serde_json::from_value(value).map_err(|err| RequestError::Invalid {
        method: method.to_string(),
        detail: err.to_string(),
    })
}

/// The last component of a path, for a workspace folder's name.
fn root_name(absolute: &str) -> String {
    absolute
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(absolute)
        .to_string()
}

/// What the process's thread does with each event ([`dispatch::sink`]).
fn on_process_event(shared: &Shared, decoder: &mut framing::Decoder, event: ProcessEvent) {
    match event {
        ProcessEvent::Stdout(chunk) => {
            decoder.push(&chunk);
            while let Some(message) = decoder.next_message() {
                match message {
                    Ok(body) => dispatch::message(shared, &body),
                    Err(err) => log::warn!("a language server's output is broken: {err}"),
                }
            }
        }
        ProcessEvent::Stderr(line) => shared.push_stderr(line),
        ProcessEvent::Exit(code) => shared.set_status(ServerStatus::Exited(code)),
    }
}
