//! The client side of the protocol (docs/PLAN.md §5.1, §5.2): a [`Connection`] to one
//! `dv backend --stdio` server, [`RemoteBackend`], the `dv_core::Backend` of one worktree
//! through it, and [`Connections`], the app's registry of connections.
//!
//! A connection is shared (`Arc`) by every `RemoteBackend` of its server: one per WSL distro,
//! or one per named loopback server. It starts its server lazily, through its [`Launcher`], and
//! sends `hello` first; calls made meanwhile wait for it. Calls block their thread until the
//! answer comes; many may be in flight at once, matched by id ([`link`]). Its
//! [`ConnectionState`] is readable ([`Connection::state`]) and observable
//! ([`Connection::subscribe`]).
//!
//! **Explicit and automatic starts** (PLAN §5.2's connecting rule: only an explicit open boots
//! a stopped distro). [`Connection::retry`] is the explicit start — the user opening a
//! repository or a review, the switcher's **Start** or **Retry**, the banner's — and starts a
//! server whatever the host's state. Every other start is automatic: a call finding no server
//! (the first call of a listing, or the first after an idle close) and a respawn after a death.
//! An automatic start first asks the launcher whether the server's host runs
//! ([`Launcher::host_running`]: a distro's state, listed afresh); when it doesn't, nothing is
//! launched and the connection is [`ConnectionState::Stopped`], its calls failing with
//! `BackendError::Unavailable`, until an explicit start. A listing's start from `Stopped`
//! ([`Connection::resume`]) takes an answer up to 2 s old instead ([`Asked::Listing`]), so the
//! rows of a stopped distro don't each list WSL's distros afresh. An explicit start made while
//! a reconnect is under way waits for that reconnect to end and takes its outcome: connected,
//! or the error of a connection that failed or found its host stopped;
//! [`Connection::wait_for_reconnect`] waits the same way but never starts anything itself.
//!
//! **Installs** (PLAN §5.2). A launcher may copy a build into place before its server starts
//! (`Launcher::Wsl` installs this app's Linux build into a distro that lacks it): the state is
//! [`ConnectionState::Installing`] while it copies. A server that answers `hello` as another
//! build (`build_ok: false`) is stopped and, when the launcher installs builds, started once
//! more with its build installed again; still another build, the connection fails.
//!
//! **Death and reconnection** (PLAN §5.2). When the server dies (its output ends, a read or a
//! write fails, it prints something that isn't the protocol, its process exits,
//! [`Connection::kill`]):
//! - every call in flight fails at once with `BackendError::Disconnected`;
//! - the connection is [`ConnectionState::Reconnecting`] and starts another server after 1 s,
//!   then 2 s, then 5 s ([`BACKOFF`]), each an automatic start (a distro the user stopped stays
//!   stopped); meanwhile calls fail at once with `Disconnected`, also while an attempt installs
//!   the build again;
//! - once one answers `hello`, every `RemoteBackend` opens its worktree on it again with its
//!   next call (a handle belongs to the server generation that issued it), and every live watch
//!   subscribes again, its sink getting one synthetic `WatchEvent { rescan: true, .. }` for the
//!   changes nobody watched ([`watch`]);
//! - when the last attempt fails too, the connection is [`ConnectionState::Failed`]: calls fail
//!   with `BackendError::Unavailable` and nothing starts until [`Connection::retry`].
//!
//! A server that dies within [`STABLE`] of a respawn continues that count rather than starting
//! a new one, so a server that dies on every start ends `Failed` instead of looping; any other
//! start (the first, an explicit one, one after an idle close) begins a fresh count. A server
//! that can't start the first time, or answers `hello` as another protocol or build, fails the
//! connection at once.
//!
//! **Idle close.** A server with no live watch and no call in flight for [`IDLE`] is closed
//! (the state goes back to [`ConnectionState::Idle`]), so a connection made for a listing
//! keeps no process in the distro, and WSL can shut an unused distro down. The period counts
//! from when the server last became unused: the end of its last call, or of its last watch.
//! Its `RemoteBackend`s stay valid: the next call starts a server again, automatically. An
//! open review holds a watch, so its server never idles out.
//!
//! Dropping the last `Arc<Connection>` (every `RemoteBackend` and remote watch handle holds
//! one) closes the server's stdin, which ends it, and reaps its process.

mod connections;
mod link;
mod observe;
mod remote;
mod watch;

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::fmt;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use dv_core::backend::{BackendError, RepoInfo};
use dv_core::location::Location;
use parking_lot::{Condvar, Mutex};

use self::link::{CallError, Link};
use self::observe::Observers;
use self::remote::Worktree;
use self::watch::WatchEntry;
use crate::launch::{Asked, Host, Launcher, StartOptions};
use crate::proto::{HelloReply, Op, OpenReply, PROTO};

pub use connections::{ConnectionKey, Connections};
pub use observe::{ConnectionState, Subscription};
pub use remote::RemoteBackend;

/// How long the server has to answer `hello` once started: a cold WSL distro boots in a few
/// seconds; a server silent for this long is treated as dead.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// The waits before each attempt to start another server after one died (PLAN §5.2); when the
/// last attempt fails, the connection is `Failed` until `retry`.
pub const BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
];

/// How long a server must have been up for its death to start a fresh count of attempts
/// (module docs).
pub const STABLE: Duration = Duration::from_secs(10);

/// How long a server goes unused (no watch, no call) before it is closed (module docs).
pub const IDLE: Duration = Duration::from_secs(60);

/// How many events the diagnostics' history keeps.
const HISTORY_LINES: usize = 100;

/// How a [`Connection`] paces itself: the waits before each respawn attempt, and how long its
/// server may go unused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionOptions {
    /// One wait per respawn attempt ([`BACKOFF`]); none fails the connection at its server's
    /// death.
    pub backoff: Vec<Duration>,
    /// When a server with no watch and no call in flight is closed ([`IDLE`]); `None` keeps it.
    pub idle: Option<Duration>,
}

impl Default for ConnectionOptions {
    fn default() -> Self {
        Self {
            backoff: BACKOFF.to_vec(),
            idle: Some(IDLE),
        }
    }
}

/// A connection to one `dv backend --stdio` server at a time (module docs).
pub struct Connection {
    name: String,
    launcher: Launcher,
    backoff: Vec<Duration>,
    idle: Option<Duration>,
    /// This connection, for the threads it starts, which must not keep it alive.
    me: Weak<Connection>,
    created: Instant,
    inner: Mutex<Inner>,
    /// Signalled with every change of state: what an explicit start made while a reconnect is
    /// under way waits on ([`Connection::retry`]).
    changed: Condvar,
    /// Held while a server starts, so concurrent first calls start one.
    connecting: Mutex<()>,
    /// The last lines of the servers' stderr, and of anything one printed before `hello`.
    output: Arc<Mutex<VecDeque<String>>>,
    observers: Arc<Observers>,
    /// The live watches, which subscribe again on every new server ([`watch`]).
    watches: Mutex<HashMap<u64, Arc<WatchEntry>>>,
    next_watch: AtomicU64,
}

struct Inner {
    state: ConnectionState,
    /// The live link, while `Connected`.
    link: Option<Arc<Link>>,
    /// Counts the servers started; a link, a worktree handle and a server watch belong to one.
    generation: u64,
    /// The live server's answer to `hello`.
    hello: Option<HelloReply>,
    /// Why the last server died: what calls fail with while reconnecting.
    death: Option<String>,
    /// The respawn attempts made since a server last stayed up for [`STABLE`], or since a start
    /// that wasn't a respawn (module docs).
    attempts: u32,
    /// When the live (or the last) server answered `hello`.
    connected_at: Option<Instant>,
    /// What happened, for the diagnostics.
    history: VecDeque<String>,
    /// The notifier thread's channel, once something subscribed ([`observe`]).
    notifier: Option<Sender<ConnectionState>>,
    /// The reconnect thread's wake-up: dropped with the connection, it ends the thread's wait.
    reconnect: Option<Sender<()>>,
    /// A reconnect attempt is under way: calls fail at once, also while it installs.
    reconnecting: bool,
    /// The calls in flight ([`Busy`]).
    busy: usize,
    /// When the server last became unused (its last call ended, its last watch went) or the
    /// live server answered: what the idle close times.
    last_used: Instant,
    /// The live server's idle thread's wake-up: replaced (or dropped) with that server, which
    /// ends the thread's wait.
    idle_watch: Option<Sender<()>>,
}

/// A call in flight (module docs): while one is, the server isn't closed for idleness.
pub(crate) struct Busy<'a> {
    connection: &'a Connection,
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        let mut inner = self.connection.inner.lock();
        inner.busy = inner.busy.saturating_sub(1);
        inner.last_used = Instant::now();
    }
}

/// Why a server didn't start.
enum StartError {
    /// The next attempt may do better: the launch failed, or the server died or stayed silent
    /// before it answered `hello`.
    Failed(String),
    /// It answered `hello` as a server this app can't use (another protocol or build);
    /// starting another won't change that.
    Refused(String),
}

impl StartError {
    fn reason(&self) -> &str {
        match self {
            StartError::Failed(reason) | StartError::Refused(reason) => reason,
        }
    }
}

/// A server that answered `hello`.
enum Started {
    /// It is the live link now.
    Live(Arc<Link>),
    /// It isn't this app's build (`build_ok: false`), on `arch` (`hello`'s); it was stopped.
    Mismatch { arch: String, reason: String },
}

impl Connection {
    /// A connection named `name` (the distro, or a loopback's name, as messages say it: "lost
    /// the connection to dv in Ubuntu") whose server `launcher` starts, reconnecting after
    /// [`BACKOFF`] and closing its server after [`IDLE`] unused. Nothing starts yet.
    pub fn new(name: impl Into<String>, launcher: Launcher) -> Arc<Self> {
        Self::with_options(name, launcher, ConnectionOptions::default())
    }

    /// [`Connection::new`] with its own waits before each respawn attempt: `backoff.len()`
    /// attempts (tests shorten [`BACKOFF`]; none fails the connection at its server's death).
    pub fn with_backoff(
        name: impl Into<String>,
        launcher: Launcher,
        backoff: Vec<Duration>,
    ) -> Arc<Self> {
        Self::with_options(
            name,
            launcher,
            ConnectionOptions {
                backoff,
                ..ConnectionOptions::default()
            },
        )
    }

    /// [`Connection::new`] paced as `options` say.
    pub fn with_options(
        name: impl Into<String>,
        launcher: Launcher,
        options: ConnectionOptions,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            name: name.into(),
            launcher,
            backoff: options.backoff,
            idle: options.idle,
            me: me.clone(),
            created: Instant::now(),
            inner: Mutex::new(Inner {
                state: ConnectionState::Idle,
                link: None,
                generation: 0,
                hello: None,
                death: None,
                attempts: 0,
                connected_at: None,
                history: VecDeque::new(),
                notifier: None,
                reconnect: None,
                reconnecting: false,
                busy: 0,
                last_used: Instant::now(),
                idle_watch: None,
            }),
            changed: Condvar::new(),
            connecting: Mutex::new(()),
            output: Arc::default(),
            observers: Arc::default(),
            watches: Mutex::default(),
            next_watch: AtomicU64::new(1),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn launcher(&self) -> &Launcher {
        &self.launcher
    }

    pub fn host(&self) -> Host {
        self.launcher.host()
    }

    pub fn state(&self) -> ConnectionState {
        self.inner.lock().state.clone()
    }

    /// How many servers this connection has started, counting attempts that failed.
    pub fn generation(&self) -> u64 {
        self.inner.lock().generation
    }

    /// The live server's answer to `hello`; `None` while there is none.
    pub fn server(&self) -> Option<HelloReply> {
        self.inner.lock().hello.clone()
    }

    /// Calls `callback` with every new state, in order, on a thread of the connection's own
    /// ([`observe`]), until the subscription is dropped. It doesn't hear the current state:
    /// read [`Connection::state`] after subscribing.
    pub fn subscribe(
        &self,
        callback: impl Fn(&ConnectionState) + Send + Sync + 'static,
    ) -> Subscription {
        let subscription = self.observers.add(Arc::new(callback));
        let mut inner = self.inner.lock();
        if inner.notifier.is_none() {
            let (tx, rx) = mpsc::channel();
            let observers = Arc::clone(&self.observers);
            let started = std::thread::Builder::new()
                .name("dv-backend-notify".into())
                .spawn(move || observe::notify(&observers, rx));
            match started {
                Ok(_) => inner.notifier = Some(tx),
                Err(error) => log::error!(
                    "couldn't start the thread that reports the state of {}: {error}",
                    self.name
                ),
            }
        }
        subscription
    }

    /// Starts the server, automatically (module docs), when none runs; what every call does
    /// first. A host that isn't running leaves the connection `Stopped`.
    pub fn connect(&self) -> Result<(), BackendError> {
        self.link().map(drop)
    }

    /// An automatic start from `Stopped` (module docs): when the host runs again (the user
    /// started the distro), starts a server as [`Connection::connect`] would from `Idle`; while
    /// it is still stopped, fails as calls do, starting nothing. On any other state it is
    /// [`Connection::connect`]. A listing uses it, so a distro started outside dv lists again;
    /// it asks the host check as a listing does ([`Asked::Listing`]).
    pub fn resume(&self) -> Result<(), BackendError> {
        let connecting = self.connecting.lock();
        if self.inner.lock().state != ConnectionState::Stopped {
            drop(connecting);
            return self.connect();
        }
        if self.launcher.host_running(Asked::Listing) == Some(false) {
            return Err(self.stopped());
        }
        {
            let mut inner = self.inner.lock();
            inner.attempts = 0;
            self.note(&mut inner, format!("{} runs again", self.name));
        }
        self.connect_now(true)
            .map(drop)
            .map_err(|error| self.fail(error))
    }

    /// Whether the host this connection's server runs on is up ([`Launcher::host_running`]),
    /// as fresh as `asked` says: `None` when there is nothing to ask. Blocks; never call it on
    /// a UI thread.
    pub fn host_running(&self, asked: Asked) -> Option<bool> {
        self.launcher.host_running(asked)
    }

    /// Marks a call in flight until the guard drops (module docs).
    pub(crate) fn busy(&self) -> Busy<'_> {
        self.inner.lock().busy += 1;
        Busy { connection: self }
    }

    /// Opens the worktree containing `path` (a path on the server's side: POSIX inside a
    /// distro) as a [`RemoteBackend`]. `env` is the environment overlay of every git the server
    /// runs for it (`LocalBackendOptions::env`): a test's fixture environment, else empty.
    pub fn open(
        self: &Arc<Self>,
        path: &str,
        env: Vec<(OsString, Option<OsString>)>,
    ) -> Result<RemoteBackend, BackendError> {
        let env = env
            .into_iter()
            .map(|(name, value)| {
                let name = name.into_string().map_err(|name| self.not_unicode(&name))?;
                let value = value
                    .map(|value| {
                        value
                            .into_string()
                            .map_err(|value| self.not_unicode(&value))
                    })
                    .transpose()?;
                Ok((name, value))
            })
            .collect::<Result<Vec<_>, BackendError>>()?;
        let _busy = self.busy();
        let link = self.link()?;
        let reply: OpenReply = link
            .call(
                Op::Open {
                    path: path.to_string(),
                    env: env.clone(),
                },
                None,
            )
            .map_err(|error| self.error(error))?;
        let worktree = Worktree::new(
            self.me.clone(),
            reply.info.toplevel.clone(),
            env,
            link.generation(),
            reply.repo,
        );
        Ok(RemoteBackend::new(
            Arc::clone(self),
            Arc::new(worktree),
            located(&self.host(), reply.info),
        ))
    }

    /// The explicit start (module docs): starts a server when none runs — none started yet,
    /// closed for idleness, `Stopped` or `Failed` — without asking whether its host runs, so
    /// it boots a stopped distro; blocks until it answers (and every live watch has subscribed
    /// again) or fails, so never call it on a UI thread. Connected, there is nothing to do.
    /// While a reconnect is under way it waits for that reconnect to end and takes its
    /// outcome: `Ok` once connected, else the error calls get from the connection that failed
    /// or found its host stopped (which the backend-down banner explains).
    pub fn retry(&self) -> Result<(), BackendError> {
        let _connecting = loop {
            if let Some(outcome) = self.wait_out_reconnect() {
                // The reconnect is `Connected` before its watches have subscribed again, and
                // holds `connecting` until they have: once it is free, they are back.
                drop(self.connecting.lock());
                return outcome;
            }
            let connecting = self.connecting.lock();
            // A death between the wait and the lock starts another reconnect: wait for it.
            if !reconnecting(&self.inner.lock()) {
                break connecting;
            }
        };
        {
            let mut inner = self.inner.lock();
            match inner.state {
                ConnectionState::Failed { .. } | ConnectionState::Stopped => {
                    self.note(&mut inner, "retry".into());
                }
                ConnectionState::Connected if inner.link.is_some() => return Ok(()),
                _ => {}
            }
            // An explicit start is never a respawn: a fresh count of attempts.
            inner.attempts = 0;
        }
        self.connect_now(true)
            .map(drop)
            .map_err(|error| self.fail(error))
    }

    /// Waits for a reconnect under way to end, starting nothing itself (an open that lost its
    /// connection under it waits so before it runs again, PLAN §5.2): `Ok` once connected again
    /// (its watches subscribed again), or when nothing is down (a call may go ahead, starting a
    /// server as calls do); else the error calls get from a connection that failed or found its
    /// host stopped, which only an explicit start ([`Connection::retry`]) brings back. A start
    /// under way ends first. Blocks; never call it on a UI thread.
    pub fn wait_for_reconnect(&self) -> Result<(), BackendError> {
        loop {
            if let Some(outcome) = self.wait_out_reconnect() {
                // As in `retry`: once `connecting` is free, the watches are back.
                drop(self.connecting.lock());
                return outcome;
            }
            let _connecting = self.connecting.lock();
            let inner = self.inner.lock();
            // A death between the wait and the lock starts another reconnect: wait for it.
            if reconnecting(&inner) {
                continue;
            }
            return match &inner.state {
                ConnectionState::Failed { error } => Err(self.failed(error)),
                ConnectionState::Stopped => Err(self.stopped()),
                _ => Ok(()),
            };
        }
    }

    /// While a reconnect is under way, waits for it to end ([`Connection::retry`]): `Some`
    /// with its outcome once it connected, failed or found its host stopped; `None` when none
    /// was under way, or it ended some other way.
    fn wait_out_reconnect(&self) -> Option<Result<(), BackendError>> {
        let mut inner = self.inner.lock();
        if !reconnecting(&inner) {
            return None;
        }
        while reconnecting(&inner) {
            self.changed.wait(&mut inner);
        }
        match &inner.state {
            ConnectionState::Connected => Some(Ok(())),
            ConnectionState::Failed { error } => Some(Err(self.failed(error))),
            ConnectionState::Stopped => Some(Err(self.stopped())),
            _ => None,
        }
    }

    /// Kills the server as a crash would (a test seam): every call in flight fails with
    /// `BackendError::Disconnected` and the connection reconnects.
    pub fn kill(&self) {
        let link = self.inner.lock().link.clone();
        if let Some(link) = link {
            link.die("dv was killed".into());
        }
    }

    /// What to paste into a bug report: the connection, how its server starts, its state and
    /// server, what happened to it, and the last lines its servers printed on stderr.
    pub fn diagnostics(&self) -> String {
        let (state, generation, hello, history) = {
            let inner = self.inner.lock();
            (
                inner.state.clone(),
                inner.generation,
                inner.hello.clone(),
                inner.history.clone(),
            )
        };
        let mut text = format!(
            "connection: {}\nlauncher: {}\nstate: {state}\nservers started: {generation}\n",
            self.name,
            self.launcher.describe()
        );
        if let Some(hello) = hello {
            text.push_str(&format!(
                "server: dv {} (protocol {}), {} {}, pid {}\n",
                hello.version, hello.proto, hello.os, hello.arch, hello.pid
            ));
        }
        text.push_str("history:\n");
        for line in &history {
            text.push_str("  ");
            text.push_str(line);
            text.push('\n');
        }
        let output = self.output.lock();
        if !output.is_empty() {
            text.push_str("server output:\n");
            for line in output.iter() {
                text.push_str("  ");
                text.push_str(line);
                text.push('\n');
            }
        }
        text
    }

    /// The live link, starting the server automatically (module docs) when none runs; fails
    /// at once while reconnecting, stopped or failed.
    pub(crate) fn link(&self) -> Result<Arc<Link>, BackendError> {
        if let Some(link) = self.ready()? {
            return Ok(link);
        }
        let _connecting = self.connecting.lock();
        if let Some(link) = self.ready()? {
            return Ok(link);
        }
        if self.launcher.host_running(Asked::Fresh) == Some(false) {
            let mut inner = self.inner.lock();
            self.note(
                &mut inner,
                format!("{} isn't running, so dv wasn't started", self.name),
            );
            self.set_state(&mut inner, ConnectionState::Stopped);
            return Err(self.stopped());
        }
        // Not a respawn (that is `reconnect_attempt`'s): a fresh count of attempts.
        self.inner.lock().attempts = 0;
        self.connect_now(true).map_err(|error| self.fail(error))
    }

    /// The live link, `None` when one may be started, or why a call can't be made now.
    fn ready(&self) -> Result<Option<Arc<Link>>, BackendError> {
        let inner = self.inner.lock();
        let disconnected = || {
            self.disconnected(
                inner
                    .death
                    .clone()
                    .unwrap_or_else(|| "dv stopped".to_string()),
            )
        };
        match &inner.state {
            ConnectionState::Connected => Ok(inner.link.clone()),
            ConnectionState::Failed { error } => Err(self.failed(error)),
            ConnectionState::Stopped => Err(self.stopped()),
            ConnectionState::Reconnecting { .. } => Err(disconnected()),
            // A reconnect that installs the build again fails calls as any reconnect does.
            ConnectionState::Installing { .. } if inner.reconnecting => Err(disconnected()),
            ConnectionState::Idle
            | ConnectionState::Connecting
            | ConnectionState::Installing { .. } => Ok(None),
        }
    }

    /// Starts a server and subscribes every live watch on it again. Runs with `connecting`
    /// held.
    fn connect_now(&self, announce: bool) -> Result<Arc<Link>, StartError> {
        let link = self.start(announce)?;
        self.resubscribe(&link);
        Ok(link)
    }

    /// Starts a server and shakes hands with it; `announce` makes the state `Connecting`
    /// meanwhile (a reconnect stays `Reconnecting`). A server whose build doesn't match is
    /// started once more with its build installed again, when the launcher installs builds
    /// (PLAN §5.2).
    fn start(&self, announce: bool) -> Result<Arc<Link>, StartError> {
        let mut reinstall = None;
        loop {
            match self.start_once(announce, reinstall.as_deref())? {
                Started::Live(link) => {
                    self.launcher.connected();
                    return Ok(link);
                }
                Started::Mismatch { arch, reason }
                    if reinstall.is_none() && self.launcher.reinstalls() =>
                {
                    let mut inner = self.inner.lock();
                    self.note(&mut inner, format!("{reason}: installing it again"));
                    reinstall = Some(arch);
                }
                Started::Mismatch { reason, .. } => return Err(StartError::Refused(reason)),
            }
        }
    }

    /// One start ([`Connection::start`]): the launch, with the state `Installing` while a
    /// build is copied, and the handshake.
    fn start_once(&self, announce: bool, reinstall: Option<&str>) -> Result<Started, StartError> {
        let (generation, before) = {
            let mut inner = self.inner.lock();
            if announce {
                self.set_state(&mut inner, ConnectionState::Connecting);
            }
            inner.generation += 1;
            (inner.generation, inner.state.clone())
        };
        let progress = |done: u64, total: u64| {
            let mut inner = self.inner.lock();
            self.set_state(&mut inner, ConnectionState::Installing { done, total });
        };
        let launched = self.launcher.launch(&StartOptions {
            reinstall,
            progress: &progress,
        });
        {
            let mut inner = self.inner.lock();
            if matches!(inner.state, ConnectionState::Installing { .. }) {
                self.set_state(&mut inner, before);
            }
        }
        let transport = launched.map_err(|error| {
            let reason = format!("couldn't start dv: {}", error.message);
            if error.permanent {
                StartError::Refused(reason)
            } else {
                StartError::Failed(reason)
            }
        })?;
        let couldnt =
            |error: std::io::Error| StartError::Failed(format!("couldn't start dv: {error}"));
        let build = transport.build.clone();
        let link = Link::start(
            transport,
            generation,
            Arc::clone(&self.output),
            self.me.clone(),
        )
        .map_err(couldnt)?;
        let hello: HelloReply = link
            .call(
                Op::Hello {
                    proto: PROTO,
                    build,
                },
                Some(HANDSHAKE_TIMEOUT),
            )
            .map_err(|error| {
                let reason = match error {
                    CallError::Dead(reason) => reason,
                    CallError::Remote(error) => error.into_backend(&self.name).to_string(),
                };
                link.die(reason.clone());
                StartError::Failed(reason)
            })?;
        {
            let mut inner = self.inner.lock();
            self.note(
                &mut inner,
                format!(
                    "dv {} answered: {} {}, pid {}",
                    hello.version, hello.os, hello.arch, hello.pid
                ),
            );
        }
        if hello.proto != PROTO {
            let reason = format!(
                "dv in {} speaks protocol {}; this app speaks {PROTO}",
                self.name, hello.proto
            );
            link.die(reason.clone());
            return Err(StartError::Refused(reason));
        }
        if hello.build_ok == Some(false) {
            let reason = format!("the dv in {} doesn't match this app", self.name);
            link.die(reason.clone());
            return Ok(Started::Mismatch {
                arch: hello.arch,
                reason,
            });
        }
        let mut inner = self.inner.lock();
        // A server that died since it answered never became the live link, so the death
        // passed the connection by (`link_died`): it is this start's failure.
        if let Some(reason) = link.death() {
            return Err(StartError::Failed(reason));
        }
        inner.link = Some(Arc::clone(&link));
        inner.hello = Some(hello);
        inner.death = None;
        inner.connected_at = Some(Instant::now());
        inner.last_used = Instant::now();
        inner.reconnect = None;
        inner.idle_watch = self.watch_idleness(generation);
        self.set_state(&mut inner, ConnectionState::Connected);
        Ok(Started::Live(link))
    }

    /// Starts the thread that closes the server of `generation` once it goes unused for the
    /// idle period (module docs); its wake-up, which ends it when dropped. `None` without an
    /// idle period.
    fn watch_idleness(&self, generation: u64) -> Option<Sender<()>> {
        let idle = self.idle?;
        let (wake, wait) = mpsc::channel();
        let me = self.me.clone();
        let started = std::thread::Builder::new()
            .name("dv-backend-idle".into())
            .spawn(move || close_when_idle(&me, &wait, generation, idle));
        match started {
            Ok(_) => Some(wake),
            Err(error) => {
                log::warn!(
                    "couldn't start the thread that closes an unused dv in {}: {error}",
                    self.name
                );
                None
            }
        }
    }

    /// Closes the server of `generation` when it has gone unused for `idle` (module docs);
    /// otherwise how much longer it must stay unused. `None` once that server is no longer the
    /// live one, closed here or not.
    fn close_if_idle(&self, generation: u64, idle: Duration) -> Option<Duration> {
        // A start under way replaces the server anyway; look again later.
        let Some(_connecting) = self.connecting.try_lock() else {
            return Some(idle);
        };
        let watched = !self.watches.lock().is_empty();
        let unused = {
            let mut inner = self.inner.lock();
            let live = inner.state == ConnectionState::Connected
                && inner
                    .link
                    .as_ref()
                    .is_some_and(|link| link.generation() == generation);
            if !live {
                return None;
            }
            if watched || inner.busy > 0 {
                return Some(idle);
            }
            let quiet = inner.last_used.elapsed();
            if quiet < idle {
                return Some(idle - quiet);
            }
            let unused = inner.link.take();
            inner.hello = None;
            inner.idle_watch = None;
            // Closed, not killed: whatever starts next isn't a respawn.
            inner.attempts = 0;
            self.note(
                &mut inner,
                format!("dv closed after {} s unused", idle.as_secs_f32()),
            );
            self.set_state(&mut inner, ConnectionState::Idle);
            unused
        };
        // Outside the lock: the last `Arc<Link>` closes the server's stdin and reaps it.
        drop(unused);
        None
    }

    /// The link of `generation` died ([`Link`]): when it is the live one, the connection
    /// reconnects, or fails with no attempts left (module docs).
    pub(crate) fn link_died(&self, generation: u64, reason: &str) {
        let dead = {
            let mut inner = self.inner.lock();
            if inner
                .link
                .as_ref()
                .is_none_or(|link| link.generation() != generation)
            {
                return;
            }
            let dead = inner.link.take();
            inner.hello = None;
            inner.idle_watch = None;
            inner.death = Some(reason.to_string());
            if inner.connected_at.is_some_and(|at| at.elapsed() >= STABLE) {
                inner.attempts = 0;
            }
            self.note(&mut inner, format!("dv died: {reason}"));
            let attempt = inner.attempts + 1;
            if attempt as usize > self.backoff.len() {
                self.set_state(
                    &mut inner,
                    ConnectionState::Failed {
                        error: reason.to_string(),
                    },
                );
            } else {
                self.set_state(&mut inner, ConnectionState::Reconnecting { attempt });
                let (wake, wait) = mpsc::channel();
                let me = self.me.clone();
                let started = std::thread::Builder::new()
                    .name("dv-backend-reconnect".into())
                    .spawn(move || reconnect(&me, &wait));
                match started {
                    Ok(_) => inner.reconnect = Some(wake),
                    Err(error) => self.set_state(
                        &mut inner,
                        ConnectionState::Failed {
                            error: format!("{reason} (couldn't start reconnecting: {error})"),
                        },
                    ),
                }
            }
            dead
        };
        // Outside the lock: the last `Arc<Link>` reaps the server's process.
        drop(dead);
    }

    /// The wait before the reconnect attempt the state names; `None` when not reconnecting.
    fn backoff_now(&self) -> Option<Duration> {
        match self.inner.lock().state {
            ConnectionState::Reconnecting { attempt } => self
                .backoff
                .get((attempt as usize).saturating_sub(1))
                .copied(),
            _ => None,
        }
    }

    /// One reconnect attempt, an automatic start (module docs); whether another follows.
    fn reconnect_attempt(&self) -> bool {
        let _connecting = self.connecting.lock();
        let attempt = {
            let mut inner = self.inner.lock();
            let ConnectionState::Reconnecting { attempt } = inner.state else {
                return false;
            };
            inner.attempts = attempt;
            attempt
        };
        if self.launcher.host_running(Asked::Fresh) == Some(false) {
            let mut inner = self.inner.lock();
            inner.reconnect = None;
            self.note(
                &mut inner,
                format!(
                    "attempt {attempt}: {} isn't running, so dv wasn't started",
                    self.name
                ),
            );
            self.set_state(&mut inner, ConnectionState::Stopped);
            return false;
        }
        self.inner.lock().reconnecting = true;
        let started = self.connect_now(false);
        let mut inner = self.inner.lock();
        inner.reconnecting = false;
        let error = match started {
            Ok(_) => return false,
            Err(error) => error,
        };
        self.note(
            &mut inner,
            format!("attempt {attempt} failed: {}", error.reason()),
        );
        let last = attempt as usize >= self.backoff.len();
        if last || matches!(error, StartError::Refused(_)) {
            inner.reconnect = None;
            self.set_state(
                &mut inner,
                ConnectionState::Failed {
                    error: error.reason().to_string(),
                },
            );
            false
        } else {
            self.set_state(
                &mut inner,
                ConnectionState::Reconnecting {
                    attempt: attempt + 1,
                },
            );
            true
        }
    }

    /// A start that failed fails the connection; the error calls get.
    fn fail(&self, error: StartError) -> BackendError {
        let reason = error.reason().to_string();
        let mut inner = self.inner.lock();
        self.set_state(
            &mut inner,
            ConnectionState::Failed {
                error: reason.clone(),
            },
        );
        self.failed(&reason)
    }

    fn set_state(&self, inner: &mut Inner, state: ConnectionState) {
        if inner.state == state {
            return;
        }
        // An install's progress is one event in the history.
        let progress = matches!(
            (&inner.state, &state),
            (
                ConnectionState::Installing { .. },
                ConnectionState::Installing { .. }
            )
        );
        if !progress {
            self.note(inner, state.to_string());
        }
        inner.state = state.clone();
        self.changed.notify_all();
        if let Some(notifier) = &inner.notifier {
            let _ = notifier.send(state);
        }
    }

    /// The server became unused now (its last watch went): the idle close counts from here
    /// (module docs).
    fn unused_now(&self) {
        self.inner.lock().last_used = Instant::now();
    }

    /// Adds `event` to the history, timed from the connection's creation.
    fn note(&self, inner: &mut Inner, event: String) {
        if inner.history.len() == HISTORY_LINES {
            inner.history.pop_front();
        }
        let at = self.created.elapsed();
        inner.history.push_back(format!(
            "+{}.{:03} s {event}",
            at.as_secs(),
            at.subsec_millis()
        ));
    }

    /// Sends `op` over the link of `generation` without waiting, when that link is still live
    /// (`close`, `unwatch`: never worth starting a server for).
    pub(crate) fn notify(&self, generation: u64, op: Op) {
        let link = self.inner.lock().link.clone();
        if let Some(link) = link
            && link.generation() == generation
        {
            link.notify(op);
        }
    }

    pub(crate) fn error(&self, error: CallError) -> BackendError {
        match error {
            CallError::Remote(error) => error.into_backend(&self.name),
            CallError::Dead(reason) => self.disconnected(reason),
        }
    }

    fn disconnected(&self, reason: String) -> BackendError {
        BackendError::Disconnected {
            location: self.name.clone(),
            reason,
        }
    }

    fn failed(&self, error: &str) -> BackendError {
        BackendError::Unavailable {
            location: self.name.clone(),
            reason: format!("the connection to dv failed: {error}"),
        }
    }

    /// What calls fail with while the connection is `Stopped`.
    fn stopped(&self) -> BackendError {
        BackendError::Unavailable {
            location: self.name.clone(),
            reason: format!("{} isn't running", self.name),
        }
    }

    fn not_unicode(&self, text: &OsString) -> BackendError {
        BackendError::Unavailable {
            location: self.name.clone(),
            reason: format!(
                "{} isn't Unicode, which dv's protocol needs",
                text.display()
            ),
        }
    }
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("name", &self.name)
            .field("launcher", &self.launcher)
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

/// Whether a reconnect is under way: the connection is `Reconnecting`, or an attempt installs
/// the build again.
fn reconnecting(inner: &Inner) -> bool {
    match inner.state {
        ConnectionState::Reconnecting { .. } => true,
        ConnectionState::Installing { .. } => inner.reconnecting,
        _ => false,
    }
}

/// The reconnect thread (module docs): waits out each attempt's backoff, then starts a server,
/// until one answers, the attempts run out, or the connection is dropped. It holds the
/// connection only during an attempt, so dropping it ends the wait.
fn reconnect(me: &Weak<Connection>, wake: &Receiver<()>) {
    loop {
        let Some(delay) = me.upgrade().and_then(|connection| connection.backoff_now()) else {
            return;
        };
        if let Err(RecvTimeoutError::Disconnected) = wake.recv_timeout(delay) {
            return;
        }
        let Some(connection) = me.upgrade() else {
            return;
        };
        if !connection.reconnect_attempt() {
            return;
        }
    }
}

/// The idle thread of the server of `generation` (module docs): looks whenever that server may
/// have gone unused for `idle`, and closes it then. It holds the connection only while it
/// looks; dropping the connection, or the server going, ends the wait.
fn close_when_idle(me: &Weak<Connection>, wake: &Receiver<()>, generation: u64, idle: Duration) {
    let mut wait = idle;
    loop {
        if let Err(RecvTimeoutError::Disconnected) = wake.recv_timeout(wait) {
            return;
        }
        let Some(connection) = me.upgrade() else {
            return;
        };
        match connection.close_if_idle(generation, idle) {
            Some(next) => wait = next.max(Duration::from_millis(10)),
            None => return,
        }
    }
}

/// `info` as the client names it: on a WSL host, the worktree root inside the distro.
fn located(host: &Host, mut info: RepoInfo) -> RepoInfo {
    if let Host::Wsl { distro } = host {
        info.location = Location::Wsl {
            distro: distro.clone(),
            path: info.toplevel.clone(),
        };
    }
    info
}

#[cfg(test)]
mod tests;
