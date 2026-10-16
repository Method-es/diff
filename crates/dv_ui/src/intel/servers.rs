//! The language servers (PLAN §18.5): one per worktree, spec and executable, shared by every
//! review open on that worktree, as the app-wide [`LanguageServers`] service.
//!
//! **Finding them.** A review asks for the servers of its files once its file list shows
//! ([`LanguageServers::serve`]), and a request asks for its file's ([`LanguageServers::lookup`]):
//! each `(spec, directory)` is resolved once, in the background, through the worktree's
//! [`Resolver`] (which reads the repository through its backend, inside the distro for a WSL
//! repository), and routed to the server its command names (started then, if it isn't
//! running), to the reason it is missing, or to nothing when the spec doesn't apply there. A
//! remote repository's platform is its server's (`hello`'s `os` and `arch`), so the resolver
//! waits for the connection to answer once.
//!
//! **Their life.** A server starts in the background (`LspClient::start` blocks until it is
//! ready) and is `Ready` once it answered `initialize`. One that ends by itself
//! (`ServerStatus::Exited`) restarts after 1, 2 and 5 s ([`Restarts`]), its command resolved
//! afresh first (`Resolver::resolve_fresh`, an install meanwhile is seen), then shows as failed
//! until **Retry**; one whose connection is down waits for it to come back instead (a remote
//! process ends with its connection, PLAN §18.3). A stop dv asked for (`ServerStatus::Stopped`)
//! is never restarted. A worktree's servers stop [`LINGER`] after its last review closes.
//!
//! **Nothing blocks the UI thread.** Starting, stopping and dropping a client all wait on the
//! server (and, for a WSL repository, on the connection to its distro), so each runs off the UI
//! thread: a client is held in a [`ClientSlot`], whose drop stops it on a thread of its own.
//!
//! **Changed files.** The review's watch reports the worktree paths that changed
//! ([`LanguageServers::files_changed`]): the resolver forgets what an install can change
//! (`Resolver::files_changed`), and every ready server is told about the files its watchers
//! asked for (`workspace/didChangeWatchedFiles`), each a change or, when the file is gone, a
//! deletion, worked out in the background.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dv_backend::{Connection, ConnectionState};
use dv_core::{Backend, Location};
use dv_lsp::client::{CHANGE, DELETE};
use dv_lsp::paths::PathStyle;
use dv_lsp::resolve::HostEnv;
use dv_lsp::{
    ClientEvent, LanguageServerSpec, LspClient, LspSettings, Missing, Platform, Progress,
    Resolution, Resolver, Restarts, ServerCommand, ServerStatus, StartError, Timeouts, languages,
    resolve::parent, server_config,
};
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{AppContext as _, Context, EventEmitter, Task};
use parking_lot::Mutex;

/// How long a worktree's servers keep running after its last review closed (PLAN §18.5).
pub const LINGER: Duration = Duration::from_secs(5 * 60);

/// How many of a server's last stderr lines its failure details show.
pub const STDERR_SHOWN: usize = 20;

/// Where a server is in its life, as the status bar shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerState {
    /// Started; not ready yet.
    Starting,
    Ready,
    /// It ended by itself and starts again shortly.
    Restarting,
    /// It ended with its connection, and starts again once that is back.
    Waiting,
    /// It ended, or couldn't start, too often: it stays down until **Retry**.
    Failed(String),
}

impl ServerState {
    /// `"starting"`, `"ready"`, `"restarting"`, `"waiting"` or `"failed"`, as automation state
    /// spells it.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Restarting => "restarting",
            Self::Waiting => "waiting",
            Self::Failed(_) => "failed",
        }
    }
}

/// What the status bar and automation know of one server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerInfo {
    pub id: u64,
    pub spec_id: &'static str,
    pub name: &'static str,
    pub state: ServerState,
    /// The directory it serves, worktree-relative (`""` is the root).
    pub root: String,
    pub command_line: String,
    /// Where its command came from: `DV_LSP_TYPESCRIPT`, "the project (7.0.2)", `settings.json`.
    pub source: String,
    pub progress: Vec<Progress>,
    /// Its last stderr lines, oldest first.
    pub stderr: Vec<String>,
    /// How many times it has started.
    pub starts: u32,
}

/// What a request finds for a file ([`LanguageServers::lookup`]).
#[derive(Clone)]
pub enum Lookup {
    /// A ready server, and the language id it opens the file with.
    Ready {
        client: Arc<LspClient>,
        language_id: &'static str,
        server: u64,
    },
    /// Its server is being found or is starting.
    Pending,
    /// No server serves it: no spec does, none applies, or its server is missing or failed.
    Unavailable,
}

/// What the pool tells its observers besides `notify`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServersEvent {
    /// A server of the worktree became ready.
    Ready(Location),
}

/// A client whose drop stops it off the UI thread (module docs).
struct ClientSlot(Option<Arc<LspClient>>);

impl ClientSlot {
    fn client(&self) -> Option<&Arc<LspClient>> {
        self.0.as_ref()
    }
}

impl Drop for ClientSlot {
    fn drop(&mut self) {
        if let Some(client) = self.0.take() {
            stop_off_thread(client);
        }
    }
}

/// Stops `client` on a thread of its own: `shutdown` and `exit`, waiting up to its grace
/// period, when this is its last holder; else the holder still using it (a request in flight,
/// on a background thread) drops it, which kills the server.
pub fn stop_off_thread(client: Arc<LspClient>) {
    let spawned = std::thread::Builder::new()
        .name("dv-lsp-stop".into())
        .spawn(move || {
            if let Ok(client) = Arc::try_unwrap(client) {
                let ended = client.stop();
                log::info!("a language server stopped: {ended:?}");
            }
        });
    if let Err(error) = spawned {
        // The closure, the client with it, was dropped here: a kill, which doesn't wait.
        log::warn!("stopping a language server in the background failed: {error}");
    }
}

/// One server (module docs).
struct Server {
    spec: &'static LanguageServerSpec,
    command: ServerCommand,
    /// A file it serves: what its command is resolved for again before a restart.
    sample: String,
    state: ServerState,
    client: Option<ClientSlot>,
    /// Bumped by every start: events and results of an earlier one are dropped.
    generation: u64,
    progress: Vec<Progress>,
    stderr: VecDeque<String>,
    restarts: Restarts,
    started_at: Instant,
    starts: u32,
    /// The start, or the wait before a restart.
    task: Option<Task<()>>,
}

/// Where a `(spec, directory)` of a worktree is served.
#[derive(Clone, Debug)]
enum Route {
    /// Queued or being resolved.
    Pending,
    Server(u64),
    Missing(Missing),
    /// The spec doesn't apply there (no root marker above it).
    NotApplicable,
}

/// One worktree's servers.
struct Worktree {
    backend: Arc<dyn Backend>,
    connection: Option<Arc<Connection>>,
    resolver: Option<Arc<Resolver>>,
    /// Finding a remote repository's platform (its server's `hello`).
    platform_task: Option<Task<()>>,
    /// The reviews using it.
    users: usize,
    /// Stops it [`LINGER`] after its last review closed.
    linger: Option<Task<()>>,
    servers: BTreeMap<u64, Server>,
    routes: HashMap<(&'static str, String), Route>,
    /// `(spec, file)` waiting to be resolved, in order.
    queue: VecDeque<(&'static LanguageServerSpec, String)>,
    resolving: Option<Task<()>>,
    /// Every `(spec, directory)` ever asked for, with a file of it: asked again when the
    /// resolver forgets (an install), so a missing server still shows as missing and a server
    /// whose command changed is replaced.
    known: HashMap<(&'static str, String), (&'static LanguageServerSpec, String)>,
    /// Bumped by every forget: a batch resolved before one is dropped and asked again.
    resolve_generation: u64,
    /// Telling servers about changed files, in the background.
    announcing: usize,
    /// Whether the connection was last heard down.
    down: bool,
    /// When missing servers were last looked for again, and how many times.
    rechecked: Option<Instant>,
    rechecks: u32,
    _connection: Option<dv_backend::Subscription>,
}

/// What a client and a connection report, from their own threads.
enum Event {
    Client {
        worktree: Location,
        server: u64,
        generation: u64,
        event: ClientEvent,
    },
    Connection {
        worktree: Location,
        up: bool,
    },
}

/// How a server starts: `LspClient::start` (blocking; called on a background thread). Tests
/// put an in-process server in its place ([`LanguageServers::set_starter`]).
pub type Starter = Arc<
    dyn Fn(
            Arc<dyn Backend>,
            &ServerCommand,
            dv_lsp::ServerConfig,
            dv_lsp::EventSink,
        ) -> Result<LspClient, StartError>
        + Send
        + Sync,
>;

/// Where the resolvers read `DV_LSP_<ID>` from: this process's environment, or a test's.
pub type EnvReader = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Each remote server's machine (its user's `PATH`, read from their shell), by connection and
/// server process: read once per connection, and again when it comes back as another server.
/// Each entry has a lock of its own, held while it is read, so two reviews on one distro read
/// it once and a slow distro holds up no other.
type Hosts = Arc<Mutex<HashMap<(usize, u32), Arc<Mutex<Option<HostEnv>>>>>>;

/// How often a missing server is looked for again on its own (a review opening, the window
/// coming back to the front): an install meanwhile changes nothing in the worktree.
pub const RECHECK_EVERY: Duration = Duration::from_secs(30);

/// The language servers of every worktree a review is open on (module docs).
pub struct LanguageServers {
    worktrees: HashMap<Location, Worktree>,
    settings: LspSettings,
    /// What clients and connections report reaches the UI thread through this.
    events: UnboundedSender<Event>,
    next_server: u64,
    next_generation: u64,
    /// How long servers linger: [`LINGER`], shorter in tests.
    linger: Duration,
    starter: Starter,
    env: Option<EnvReader>,
    hosts: Hosts,
    _events: Task<()>,
}

impl EventEmitter<ServersEvent> for LanguageServers {}

impl LanguageServers {
    /// The pool, with `settings.json`'s `lsp` (`lsp.<id>.command`, PLAN §18.4).
    pub fn new(settings: LspSettings, cx: &mut Context<Self>) -> Self {
        let (sender, mut receiver) = unbounded::<Event>();
        let task = cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.next().await {
                if this
                    .update(cx, |this, cx| this.on_event(event, cx))
                    .is_err()
                {
                    return;
                }
            }
        });
        Self {
            worktrees: HashMap::new(),
            settings,
            events: sender,
            next_server: 0,
            next_generation: 0,
            linger: LINGER,
            starter: Arc::new(|backend, command, config, events| {
                LspClient::start(backend, command, config, events, Timeouts::default())
            }),
            env: None,
            hosts: Hosts::default(),
            _events: task,
        }
    }

    /// Starts servers with `starter` instead of `LspClient::start` (tests).
    pub fn set_starter(&mut self, starter: Starter) {
        self.starter = starter;
    }

    /// Has the resolvers read `DV_LSP_<ID>` through `env` instead of this process's
    /// environment (tests); worktrees acquired before keep theirs.
    pub fn set_env(&mut self, env: EnvReader) {
        self.env = Some(env);
    }

    /// A resolver for `backend`'s worktree on `platform`, whose machine is `host` (`None`:
    /// this process's environment).
    fn resolver(
        &self,
        backend: Arc<dyn Backend>,
        platform: Platform,
        host: Option<HostEnv>,
    ) -> Arc<Resolver> {
        let mut resolver = Resolver::new(backend, platform, self.settings.clone());
        if let Some(host) = host {
            resolver = resolver.with_host(host);
        }
        if overrides_only() {
            resolver = resolver.overrides_only();
        }
        Arc::new(match self.env.clone() {
            Some(env) => resolver.with_env(move |name| env(name)),
            None => resolver,
        })
    }

    /// Lingers `linger` instead of [`LINGER`] (tests).
    pub fn set_linger(&mut self, linger: Duration) {
        self.linger = linger;
    }

    /// Nothing is being resolved, starting or about to restart, and no change is being
    /// announced: automation's `settled`. A failed, missing or lingering server is settled.
    pub fn is_settled(&self) -> bool {
        self.worktrees.values().all(|worktree| {
            worktree.resolving.is_none()
                && worktree.queue.is_empty()
                && worktree.platform_task.is_none()
                && worktree.announcing == 0
                && worktree.servers.values().all(|server| {
                    matches!(server.state, ServerState::Ready | ServerState::Failed(_))
                        || (server.state == ServerState::Waiting && worktree.down)
                })
        })
    }

    /// A review on `backend`'s worktree opened: it is used, and lingers no more. `connection`
    /// is its remote backend's, if any.
    pub fn acquire(
        &mut self,
        backend: Arc<dyn Backend>,
        connection: Option<Arc<Connection>>,
        cx: &mut Context<Self>,
    ) -> Location {
        let key = backend.repo().location.clone();
        if let Some(worktree) = self.worktrees.get_mut(&key) {
            worktree.users += 1;
            worktree.linger = None;
            // Another review opens on it: a server missing until now may have been installed.
            self.recheck(&key, false, cx);
            return key;
        }
        let subscription = connection.as_ref().map(|connection| {
            let events = self.events.clone();
            let worktree = key.clone();
            connection.subscribe(move |state| {
                let up = is_up(state);
                let _ = events.unbounded_send(Event::Connection {
                    worktree: worktree.clone(),
                    up,
                });
            })
        });
        let down = connection
            .as_ref()
            .is_some_and(|connection| !is_up(&connection.state()));
        self.worktrees.insert(
            key.clone(),
            Worktree {
                backend,
                connection,
                resolver: None,
                platform_task: None,
                users: 1,
                linger: None,
                servers: BTreeMap::new(),
                routes: HashMap::new(),
                queue: VecDeque::new(),
                resolving: None,
                known: HashMap::new(),
                resolve_generation: 0,
                announcing: 0,
                down,
                rechecked: None,
                rechecks: 0,
                _connection: subscription,
            },
        );
        self.ensure_resolver(&key, cx);
        key
    }

    /// A review on `worktree` closed: once none is left, its servers stop after the linger.
    pub fn release(&mut self, worktree: &Location, cx: &mut Context<Self>) {
        let linger = self.linger;
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return;
        };
        entry.users = entry.users.saturating_sub(1);
        if entry.users > 0 {
            return;
        }
        let key = worktree.clone();
        entry.linger = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(linger).await;
            this.update(cx, |this, cx| {
                if this
                    .worktrees
                    .get(&key)
                    .is_some_and(|worktree| worktree.users == 0)
                {
                    log::info!("the language servers of {key:?} stop: no review uses them");
                    // Dropping the slots stops their servers off the UI thread.
                    let gone = this.worktrees.remove(&key);
                    // Its connection's machine is forgotten once no worktree uses it.
                    if let Some(connection) = gone.and_then(|gone| gone.connection) {
                        let used = this.worktrees.values().any(|other| {
                            other
                                .connection
                                .as_ref()
                                .is_some_and(|known| Arc::ptr_eq(known, &connection))
                        });
                        if !used {
                            let ptr = connection_key(&connection);
                            this.hosts.lock().retain(|(known, _), _| *known != ptr);
                        }
                    }
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Whether `worktree` is known (a review uses it, or its servers linger).
    pub fn has_worktree(&self, worktree: &Location) -> bool {
        self.worktrees.contains_key(worktree)
    }

    /// Starts, in the background, the servers that serve `paths` (the review's files, once its
    /// list shows, PLAN §18.5).
    pub fn serve(
        &mut self,
        worktree: &Location,
        paths: impl IntoIterator<Item = String>,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return;
        };
        let mut queued = false;
        for path in paths {
            for (spec, _) in languages::for_path(&path) {
                let route = (spec.id, parent(&path).to_string());
                entry
                    .known
                    .entry(route.clone())
                    .or_insert_with(|| (spec, path.clone()));
                if let std::collections::hash_map::Entry::Vacant(vacant) = entry.routes.entry(route)
                {
                    vacant.insert(Route::Pending);
                    entry.queue.push_back((spec, path.clone()));
                    queued = true;
                }
            }
        }
        if queued {
            self.pump(worktree, cx);
        }
    }

    /// `spec`'s server for `path` of `worktree`, as a request needs it (module docs): asked
    /// for when it hasn't been yet. Hover asks every spec that serves the file; definition and
    /// references ask them in the registry's order until one answers with anything (PLAN
    /// §18.4).
    pub fn lookup(
        &mut self,
        worktree: &Location,
        path: &str,
        spec: &'static LanguageServerSpec,
        cx: &mut Context<Self>,
    ) -> Lookup {
        let Some(language_id) = spec.language_id(path) else {
            return Lookup::Unavailable;
        };
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return Lookup::Unavailable;
        };
        let key = (spec.id, parent(path).to_string());
        match entry.routes.get(&key) {
            None => {
                entry
                    .known
                    .entry(key.clone())
                    .or_insert_with(|| (spec, path.to_string()));
                entry.routes.insert(key, Route::Pending);
                entry.queue.push_back((spec, path.to_string()));
                self.pump(worktree, cx);
                Lookup::Pending
            }
            Some(Route::Pending) => Lookup::Pending,
            Some(Route::Server(id)) => match entry.servers.get(id) {
                Some(server) => match (&server.state, server.client.as_ref()) {
                    (ServerState::Ready, Some(slot)) => match slot.client() {
                        Some(client) => Lookup::Ready {
                            client: client.clone(),
                            language_id,
                            server: *id,
                        },
                        None => Lookup::Unavailable,
                    },
                    (ServerState::Failed(_), _) => Lookup::Unavailable,
                    _ => Lookup::Pending,
                },
                None => Lookup::Unavailable,
            },
            Some(Route::Missing(_) | Route::NotApplicable) => Lookup::Unavailable,
        }
    }

    /// The servers of `worktree`, in the order they were found.
    pub fn servers(&self, worktree: &Location) -> Vec<ServerInfo> {
        let Some(entry) = self.worktrees.get(worktree) else {
            return Vec::new();
        };
        entry
            .servers
            .iter()
            .map(|(id, server)| ServerInfo {
                id: *id,
                spec_id: server.spec.id,
                name: server.spec.name,
                state: server.state.clone(),
                root: server.command.root.clone(),
                command_line: server.command.command_line(),
                source: source_label(&server.command.source),
                progress: server.progress.clone(),
                stderr: server.stderr.iter().cloned().collect(),
                starts: server.starts,
            })
            .collect()
    }

    /// Why each spec the review's files need is missing, for the specs no server of which
    /// runs: one entry per spec.
    pub fn missing(&self, worktree: &Location) -> Vec<Missing> {
        let Some(entry) = self.worktrees.get(worktree) else {
            return Vec::new();
        };
        let running: HashSet<&str> = entry
            .servers
            .values()
            .map(|server| server.spec.id)
            .collect();
        let mut missing: BTreeMap<&str, Missing> = BTreeMap::new();
        for route in entry.routes.values() {
            if let Route::Missing(m) = route
                && !running.contains(m.spec_id)
            {
                missing.entry(m.spec_id).or_insert_with(|| m.clone());
            }
        }
        missing.into_values().collect()
    }

    /// The reviewer's **Retry** on a failed server: it starts again, its command resolved
    /// afresh, its restart count from the start.
    pub fn retry(&mut self, worktree: &Location, server: u64, cx: &mut Context<Self>) {
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return;
        };
        let Some(found) = entry.servers.get_mut(&server) else {
            return;
        };
        found.restarts.retry();
        self.restart(worktree, server, cx);
    }

    /// The review's watch saw `paths` change (`rescan`: anything may have; module docs).
    pub fn files_changed(
        &mut self,
        worktree: &Location,
        paths: &[String],
        rescan: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return;
        };
        let forgot = entry.resolver.as_ref().is_some_and(|resolver| {
            if rescan {
                resolver.forget();
                true
            } else {
                resolver.files_changed(paths.iter().map(String::as_str))
            }
        });
        if forgot {
            requeue(entry);
        }
        let clients: Vec<Arc<LspClient>> = entry
            .servers
            .values()
            .filter(|server| server.state == ServerState::Ready)
            .filter_map(|server| server.client.as_ref()?.client().cloned())
            .collect();
        if clients.is_empty() || paths.is_empty() {
            if forgot {
                self.pump(worktree, cx);
            }
            return;
        }
        let backend = entry.backend.clone();
        let paths = paths.to_vec();
        entry.announcing += 1;
        let job = cx.background_spawn(async move {
            for client in clients {
                // A server takes an open document's text from dv, not the disk, so a document
                // opened from text the agent has since changed is closed: the next request
                // opens it afresh.
                for path in &paths {
                    client.close_document(path);
                }
                // Only the paths its watchers ask about are looked at (a batch may be large,
                // and each look is a call to a WSL backend).
                let watched = client.watched(&paths);
                if watched.is_empty() {
                    continue;
                }
                let changes: Vec<(String, u8)> = watched
                    .into_iter()
                    .map(|path| {
                        let kind = match backend.worktree_len(&path) {
                            Ok(None) => DELETE,
                            _ => CHANGE,
                        };
                        (path, kind)
                    })
                    .collect();
                client.files_changed(&changes);
            }
        });
        if forgot {
            self.pump(worktree, cx);
        }
        let key = worktree.clone();
        cx.spawn(async move |this, cx| {
            job.await;
            this.update(cx, |this, cx| {
                if let Some(entry) = this.worktrees.get_mut(&key) {
                    entry.announcing = entry.announcing.saturating_sub(1);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Makes the worktree's resolver once its platform is known: this machine's for a local
    /// repository, its server's for a remote one, whose `PATH` is its user's shell's (module
    /// docs).
    fn ensure_resolver(&mut self, worktree: &Location, cx: &mut Context<Self>) {
        let Some(entry) = self.worktrees.get(worktree) else {
            return;
        };
        if entry.resolver.is_some() || entry.platform_task.is_some() {
            return;
        }
        let backend = entry.backend.clone();
        let local = entry.connection.is_none();
        if local {
            let resolver = self.resolver(backend, Platform::this_machine(), None);
            if let Some(entry) = self.worktrees.get_mut(worktree) {
                entry.resolver = Some(resolver);
            }
            return;
        }
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return;
        };
        let Some(connection) = entry.connection.clone() else {
            return;
        };
        if entry.down {
            // Asked again once the connection is back.
            return;
        }
        let hosts = self.hosts.clone();
        let job = cx.background_spawn(async move {
            // Idle, the connection's next call starts the server; `connect` is that call.
            if connection.server().is_none() {
                let _ = connection.connect();
            }
            let hello = connection.server()?;
            let host = read_host(&hosts, &connection, &hello, backend.as_ref(), false);
            Some((Platform::new(hello.os, hello.arch), host))
        });
        let key = worktree.clone();
        entry.platform_task = Some(cx.spawn(async move |this, cx| {
            let platform = job.await;
            this.update(cx, |this, cx| {
                let Some(entry) = this.worktrees.get_mut(&key) else {
                    return;
                };
                entry.platform_task = None;
                match platform {
                    Some((platform, host)) => {
                        let backend = entry.backend.clone();
                        let resolver = this.resolver(backend, platform, Some(host));
                        if let Some(entry) = this.worktrees.get_mut(&key) {
                            entry.resolver = Some(resolver);
                        }
                        this.pump(&key, cx);
                    }
                    None => log::info!(
                        "the language servers of {key:?} wait for the connection to answer"
                    ),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// A remote worktree's connection came back, maybe as another server: its machine is read
    /// again (once per server), and when its `PATH` changed every directory is resolved again.
    fn refresh_host(&mut self, worktree: &Location, cx: &mut Context<Self>) {
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return;
        };
        let (Some(resolver), Some(connection)) = (entry.resolver.clone(), entry.connection.clone())
        else {
            return;
        };
        if entry.platform_task.is_some() {
            return;
        }
        let backend = entry.backend.clone();
        let hosts = self.hosts.clone();
        let job = cx.background_spawn(async move {
            let hello = connection.server()?;
            let host = read_host(&hosts, &connection, &hello, backend.as_ref(), false);
            Some(resolver.set_host(host))
        });
        let key = worktree.clone();
        entry.platform_task = Some(cx.spawn(async move |this, cx| {
            let changed = job.await;
            this.update(cx, |this, cx| {
                let Some(entry) = this.worktrees.get_mut(&key) else {
                    return;
                };
                entry.platform_task = None;
                if changed == Some(true) {
                    requeue(entry);
                }
                this.pump(&key, cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// **Check again** on a missing server: the machine's `PATH` read afresh (a distro's
    /// shell again), everything forgotten and every directory resolved again (PLAN §18.5).
    pub fn check_again(&mut self, worktree: &Location, cx: &mut Context<Self>) {
        self.recheck(worktree, true, cx);
    }

    /// The window came back to the front: missing servers are looked for again, at most every
    /// [`RECHECK_EVERY`].
    pub fn recheck_missing(&mut self, worktree: &Location, cx: &mut Context<Self>) {
        self.recheck(worktree, false, cx);
    }

    /// How many times `worktree`'s missing servers were looked for again (automation).
    pub fn rechecks(&self, worktree: &Location) -> u32 {
        self.worktrees
            .get(worktree)
            .map_or(0, |entry| entry.rechecks)
    }

    /// Looks for `worktree`'s servers again (`force`: whatever is missing and however
    /// recently; else only when one is missing, at most every [`RECHECK_EVERY`]).
    fn recheck(&mut self, worktree: &Location, force: bool, cx: &mut Context<Self>) {
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return;
        };
        let missing = entry
            .routes
            .values()
            .any(|route| matches!(route, Route::Missing(_)));
        let recent = entry
            .rechecked
            .is_some_and(|at| at.elapsed() < RECHECK_EVERY);
        if !force && (!missing || recent) {
            return;
        }
        let Some(resolver) = entry.resolver.clone() else {
            return;
        };
        if entry.platform_task.is_some() || entry.down {
            return;
        }
        entry.rechecked = Some(Instant::now());
        entry.rechecks += 1;
        let backend = entry.backend.clone();
        let connection = entry.connection.clone();
        let hosts = self.hosts.clone();
        let job = cx.background_spawn(async move {
            let host = match &connection {
                Some(connection) => connection
                    .server()
                    .map(|hello| read_host(&hosts, connection, &hello, backend.as_ref(), true)),
                None if overrides_only() => None,
                None => Some(HostEnv::this_process()),
            };
            if let Some(host) = host {
                resolver.set_host(host);
            }
            resolver.forget();
        });
        let key = worktree.clone();
        entry.platform_task = Some(cx.spawn(async move |this, cx| {
            job.await;
            this.update(cx, |this, cx| {
                let Some(entry) = this.worktrees.get_mut(&key) else {
                    return;
                };
                entry.platform_task = None;
                requeue(entry);
                this.pump(&key, cx);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Resolves the queued `(spec, file)`s in the background, one batch at a time.
    fn pump(&mut self, worktree: &Location, cx: &mut Context<Self>) {
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return;
        };
        if entry.resolving.is_some() || entry.queue.is_empty() || entry.down {
            return;
        }
        let Some(resolver) = entry.resolver.clone() else {
            self.ensure_resolver(worktree, cx);
            return;
        };
        let batch: Vec<(&'static LanguageServerSpec, String)> = entry.queue.drain(..).collect();
        let generation = entry.resolve_generation;
        let job = cx.background_spawn(async move {
            batch
                .into_iter()
                .map(|(spec, path)| {
                    let resolved = resolver.resolve(spec, &path);
                    (spec, path, resolved)
                })
                .collect::<Vec<_>>()
        });
        let key = worktree.clone();
        entry.resolving = Some(cx.spawn(async move |this, cx| {
            let resolved = job.await;
            this.update(cx, |this, cx| {
                let current = match this.worktrees.get_mut(&key) {
                    Some(entry) => {
                        entry.resolving = None;
                        entry.resolve_generation == generation
                    }
                    None => return,
                };
                // Resolved before a forget: dropped (the forget queued them again).
                if current {
                    for (spec, path, resolution) in resolved {
                        this.route(&key, spec, &path, resolution, cx);
                    }
                }
                this.pump(&key, cx);
                this.collect_unused(&key);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Once every directory asked for is resolved, stops the servers no route uses any more
    /// (an install changed the executable a directory resolves to): dropping the slot stops
    /// its server off the UI thread.
    fn collect_unused(&mut self, worktree: &Location) {
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return;
        };
        if entry.resolving.is_some()
            || !entry.queue.is_empty()
            || entry
                .routes
                .values()
                .any(|route| matches!(route, Route::Pending))
        {
            return;
        }
        let used: HashSet<u64> = entry
            .routes
            .values()
            .filter_map(|route| match route {
                Route::Server(id) => Some(*id),
                _ => None,
            })
            .collect();
        entry.servers.retain(|id, server| {
            let keep = used.contains(id);
            if !keep {
                log::info!(
                    "{} ({}) serves nothing any more: it stops",
                    server.spec.name,
                    server.command.command_line()
                );
            }
            keep
        });
    }

    /// Routes `(spec, path's directory)` by what resolved, starting its server when it is new.
    fn route(
        &mut self,
        worktree: &Location,
        spec: &'static LanguageServerSpec,
        path: &str,
        resolution: Result<Option<Resolution>, dv_core::BackendError>,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return;
        };
        let key = (spec.id, parent(path).to_string());
        let route = match resolution {
            Ok(None) => Route::NotApplicable,
            Ok(Some(Resolution::Missing(missing))) => {
                log::info!("{} for {path}: missing ({})", spec.name, missing.reason);
                Route::Missing(missing)
            }
            Ok(Some(Resolution::Found(command))) => {
                let known = entry
                    .servers
                    .iter()
                    .find(|(_, server)| server.command == command)
                    .map(|(id, _)| *id);
                let id = match known {
                    Some(id) => id,
                    None => {
                        self.next_server += 1;
                        let id = self.next_server;
                        log::info!(
                            "{} for {path}: {} ({})",
                            spec.name,
                            command.command_line(),
                            source_label(&command.source)
                        );
                        entry.servers.insert(
                            id,
                            Server {
                                spec,
                                command,
                                sample: path.to_string(),
                                state: ServerState::Starting,
                                client: None,
                                generation: 0,
                                progress: Vec::new(),
                                stderr: VecDeque::new(),
                                restarts: Restarts::new(),
                                started_at: Instant::now(),
                                starts: 0,
                                task: None,
                            },
                        );
                        entry.routes.insert(key, Route::Server(id));
                        self.start(worktree, id, cx);
                        return;
                    }
                };
                Route::Server(id)
            }
            Err(error) => {
                // Asked again by the next request (a lost connection, an unreadable path).
                log::warn!("resolving {} for {path}: {error}", spec.name);
                entry.routes.remove(&key);
                return;
            }
        };
        entry.routes.insert(key, route);
    }

    /// Starts server `id` in the background (module docs).
    fn start(&mut self, worktree: &Location, id: u64, cx: &mut Context<Self>) {
        self.next_generation += 1;
        let generation = self.next_generation;
        let events = self.events.clone();
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return;
        };
        let backend = entry.backend.clone();
        let Some(server) = entry.servers.get_mut(&id) else {
            return;
        };
        server.generation = generation;
        server.state = ServerState::Starting;
        server.progress.clear();
        server.client = None;
        server.started_at = Instant::now();
        server.starts += 1;
        let command = server.command.clone();
        let config = server_config(server.spec, &command);
        let sink_worktree = worktree.clone();
        let sink: dv_lsp::EventSink = Arc::new(move |event| {
            let _ = events.unbounded_send(Event::Client {
                worktree: sink_worktree.clone(),
                server: id,
                generation,
                event,
            });
        });
        let starter = self.starter.clone();
        let job = cx.background_spawn(async move { starter(backend, &command, config, sink) });
        let key = worktree.clone();
        server.task = Some(cx.spawn(async move |this, cx| {
            let started = job.await;
            this.update(cx, |this, cx| {
                this.started(&key, id, generation, started, cx)
            })
            .ok();
        }));
        cx.notify();
    }

    fn started(
        &mut self,
        worktree: &Location,
        id: u64,
        generation: u64,
        started: Result<LspClient, StartError>,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .worktrees
            .get(worktree)
            .and_then(|entry| entry.servers.get(&id))
            .is_some_and(|server| server.generation == generation);
        let client = match started {
            Ok(client) if current => client,
            Ok(client) => {
                // Superseded (a retry meanwhile): stopped, never used.
                stop_off_thread(Arc::new(client));
                return;
            }
            Err(_) if !current => return,
            Err(error) => {
                let stderr = match &error {
                    StartError::Exited { stderr, .. } => stderr.clone(),
                    _ => Vec::new(),
                };
                if let Some(server) = self.server_mut(worktree, id) {
                    server.task = None;
                    for line in stderr {
                        push_stderr(&mut server.stderr, line);
                    }
                }
                log::warn!("a language server didn't start: {error}");
                self.ended(worktree, id, error.to_string(), cx);
                return;
            }
        };
        if let Some(server) = self.server_mut(worktree, id) {
            server.task = None;
            server.state = ServerState::Ready;
            server.client = Some(ClientSlot(Some(Arc::new(client))));
        }
        cx.emit(ServersEvent::Ready(worktree.clone()));
        cx.notify();
    }

    fn server_mut(&mut self, worktree: &Location, id: u64) -> Option<&mut Server> {
        self.worktrees.get_mut(worktree)?.servers.get_mut(&id)
    }

    /// Server `id` ended by itself or couldn't start (`why`): it restarts after its next
    /// delay, waits for its connection, or has failed (module docs).
    fn ended(&mut self, worktree: &Location, id: u64, why: String, cx: &mut Context<Self>) {
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return;
        };
        // Its process may end before the connection's news that it is down arrives.
        let down = entry.down
            || entry
                .connection
                .as_ref()
                .is_some_and(|c| !is_up(&c.state()));
        entry.down = down;
        let Some(server) = entry.servers.get_mut(&id) else {
            return;
        };
        // Dropping the slot stops what is left of it off the UI thread.
        server.client = None;
        server.progress.clear();
        if down {
            server.state = ServerState::Waiting;
            server.task = None;
            cx.notify();
            return;
        }
        match server.restarts.after_exit(server.started_at.elapsed()) {
            Some(delay) => {
                server.state = ServerState::Restarting;
                let key = worktree.clone();
                let generation = server.generation;
                server.task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    this.update(cx, |this, cx| {
                        let due = this
                            .server_mut(&key, id)
                            .is_some_and(|server| server.generation == generation);
                        if due {
                            this.restart(&key, id, cx);
                        }
                    })
                    .ok();
                }));
            }
            None => {
                server.state = ServerState::Failed(why);
                server.task = None;
            }
        }
        cx.notify();
    }

    /// Starts server `id` again, its command resolved afresh first (PLAN §18.5).
    fn restart(&mut self, worktree: &Location, id: u64, cx: &mut Context<Self>) {
        self.next_generation += 1;
        let generation = self.next_generation;
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return;
        };
        let Some(resolver) = entry.resolver.clone() else {
            return;
        };
        let Some(server) = entry.servers.get_mut(&id) else {
            return;
        };
        server.generation = generation;
        server.state = ServerState::Starting;
        let (spec, sample) = (server.spec, server.sample.clone());
        let job = cx.background_spawn(async move { resolver.resolve_fresh(spec, &sample) });
        let key = worktree.clone();
        server.task = Some(cx.spawn(async move |this, cx| {
            let resolved = job.await;
            this.update(cx, |this, cx| {
                let Some(server) = this
                    .server_mut(&key, id)
                    .filter(|server| server.generation == generation)
                else {
                    return;
                };
                match resolved {
                    Ok(Some(Resolution::Found(command))) => {
                        server.command = command;
                        this.start(&key, id, cx);
                    }
                    Ok(Some(Resolution::Missing(missing))) => {
                        server.task = None;
                        server.state =
                            ServerState::Failed(format!("{} ({})", missing.reason, missing.hint));
                    }
                    Ok(None) => {
                        server.task = None;
                        server.state = ServerState::Failed(format!(
                            "{} no longer applies to {}",
                            server.spec.name, server.sample
                        ));
                    }
                    Err(error) => {
                        server.task = None;
                        this.ended(&key, id, error.to_string(), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Client {
                worktree,
                server: id,
                generation,
                event,
            } => {
                let Some(server) = self
                    .server_mut(&worktree, id)
                    .filter(|server| server.generation == generation)
                else {
                    return;
                };
                match event {
                    ClientEvent::Progress(progress) => server.progress = progress,
                    ClientEvent::Stderr(line) => push_stderr(&mut server.stderr, line),
                    // A crash: a server that ended while it started is its start's to report.
                    ClientEvent::Status(ServerStatus::Exited(code))
                        if server.state == ServerState::Ready =>
                    {
                        let why = match code {
                            Some(code) => format!("it exited with code {code}"),
                            None => "it was killed".to_string(),
                        };
                        log::warn!("{} ended by itself: {why}", server.spec.name);
                        self.ended(&worktree, id, why, cx);
                        return;
                    }
                    // A stop dv asked for, or the start's own news.
                    ClientEvent::Status(_) => return,
                }
                cx.notify();
            }
            Event::Connection { worktree, up } => {
                let Some(entry) = self.worktrees.get_mut(&worktree) else {
                    return;
                };
                let was_down = entry.down;
                entry.down = !up;
                if !(was_down && up) {
                    return;
                }
                let waiting: Vec<u64> = entry
                    .servers
                    .iter()
                    .filter(|(_, server)| server.state == ServerState::Waiting)
                    .map(|(id, _)| *id)
                    .collect();
                self.ensure_resolver(&worktree, cx);
                self.refresh_host(&worktree, cx);
                for id in waiting {
                    self.restart(&worktree, id, cx);
                }
                self.pump(&worktree, cx);
                cx.notify();
            }
        }
    }
}

/// Every directory asked for is resolved again (the running servers go on until their routes
/// move, `collect_unused`), and an answer under way is dropped: the resolver forgot.
fn requeue(entry: &mut Worktree) {
    entry.resolve_generation += 1;
    entry.routes.clear();
    entry.queue.clear();
    for (key, (spec, path)) in &entry.known {
        entry.routes.insert(key.clone(), Route::Pending);
        entry.queue.push_back((spec, path.clone()));
    }
}

/// Whether servers come only from `DV_LSP_<ID>`: an automated run (the app under
/// `--automation`, its tests) starts only the servers it names, never one a fixture's
/// surroundings happen to hold.
fn overrides_only() -> bool {
    cfg!(test) || crate::automation::is_active()
}

/// The machine of a remote repository's server `hello` (PLAN §18.4): a distro's user's `PATH`
/// read from their shell, once per server (`fresh`: again); a loopback server's on Windows,
/// this machine's own. An automated run reads nothing ([`overrides_only`]). Blocks: called off
/// the UI thread.
fn read_host(
    hosts: &Hosts,
    connection: &Arc<Connection>,
    hello: &dv_backend::proto::HelloReply,
    backend: &dyn Backend,
    fresh: bool,
) -> HostEnv {
    if overrides_only() {
        return HostEnv::unknown(PathStyle::Posix);
    }
    if hello.os == "windows" {
        return HostEnv::this_process();
    }
    let ptr = connection_key(connection);
    let slot = {
        let mut hosts = hosts.lock();
        // An earlier server of this connection is gone.
        hosts.retain(|(known, pid), _| *known != ptr || *pid == hello.pid);
        hosts.entry((ptr, hello.pid)).or_default().clone()
    };
    let mut slot = slot.lock();
    if !fresh && let Some(known) = slot.as_ref() {
        return known.clone();
    }
    let host = dv_lsp::resolve::shell::read(backend);
    log::info!(
        "the language servers' PATH in the distro: {} ({} folders)",
        host.path_label(),
        host.path().len()
    );
    *slot = Some(host.clone());
    host
}

/// A connection's identity in [`Hosts`].
fn connection_key(connection: &Arc<Connection>) -> usize {
    Arc::as_ptr(connection) as *const () as usize
}

/// Whether a connection in `state` takes calls: connected, or idle (the next call starts it).
fn is_up(state: &ConnectionState) -> bool {
    matches!(state, ConnectionState::Connected | ConnectionState::Idle)
}

/// Keeps the last [`STDERR_SHOWN`] lines.
fn push_stderr(lines: &mut VecDeque<String>, line: String) {
    if lines.len() == STDERR_SHOWN {
        lines.pop_front();
    }
    lines.push_back(line);
}

/// Where a command came from, as the status bar's details say it.
fn source_label(source: &dv_lsp::CommandSource) -> String {
    match source {
        dv_lsp::CommandSource::Override(var) => var.clone(),
        dv_lsp::CommandSource::Project(version) => format!("the project's own ({version})"),
        dv_lsp::CommandSource::Settings => "settings.json".to_string(),
        dv_lsp::CommandSource::Path(path) => path.clone(),
        dv_lsp::CommandSource::BunGlobal(version) => format!("bun's global packages ({version})"),
    }
}
