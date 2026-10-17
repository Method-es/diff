//! Finding a spec's server for a file (docs/PLAN.md §18.4): the `DV_LSP_<ID>` override, else
//! the spec's resolvers in order (the project's own copy, `settings.json`, `PATH`, bun's global
//! folder), else missing with the reason and how to get it. Every resolver reads the
//! repository's machine through its `Backend`, so a WSL repository is resolved inside its
//! distro, against the distro's files and the distro's platform.
//!
//! A resolver reads the backend's machine read-only, inside the worktree and outside it
//! (`Backend::probe_path`): Node resolves packages above the worktree root and through links
//! that leave it ([`node`]); `PATH` and the user's home come from a [`HostEnv`] ([`host`]: this
//! process's environment, or a distro user's shell read through the backend, [`shell`]). A
//! JavaScript server resolves to its package's entry script and runs with bun or node ([`js`]).
//!
//! The [`Resolver`] remembers each directory's answer, and what `PATH`, bun's global folder,
//! the runtime and a spec's worktree check found. The review's watch never reports
//! `node_modules` (git ignores it), so the app tells it about the tracked files an install
//! changes ([`Resolver::files_changed`]: a `package.json`, a lockfile), and resolves afresh
//! before each restart of a server ([`Resolver::resolve_fresh`], PLAN §18.5).

pub mod host;
pub mod js;
pub mod node;
pub mod platform;
pub mod shell;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dv_core::backend::{Backend, BackendError, ProcessSpec};
use parking_lot::Mutex;

pub use host::{HostEnv, PathFrom};
pub use platform::Platform;

use crate::paths::WorkspacePaths;
use crate::settings::{LspSettings, Override};
use crate::spec::{LanguageServerSpec, ResolverKind};
use js::{Found, JsRuntime, RuntimeError, RuntimesSeen};
use node::{AbsPath, Reader};

/// What a resolver reads with.
pub struct ResolveContext<'a> {
    /// Reads paths on the backend's machine, inside the worktree or not (`Backend::probe_path`).
    pub reader: &'a Reader<'a>,
    /// The worktree root.
    pub root: &'a AbsPath,
    pub platform: &'a Platform,
}

/// What a spec's project resolver found from one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectOutcome {
    /// The server: its executable (absolute, where the repository lives) and the version of
    /// the package it came from.
    Found { program: String, version: String },
    /// The project has the package, but not a server dv can run: why.
    Unusable(String),
    /// The project doesn't have it: where the resolver looked.
    NotFound(String),
}

/// Where a server's command came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CommandSource {
    /// `DV_LSP_<ID>`, the variable's name.
    Override(String),
    /// The project's own copy, at this version.
    Project(String),
    /// `settings.json`'s `lsp.<id>.command`.
    Settings,
    /// `PATH`, as [`HostEnv::path_label`] names it.
    Path(String),
    /// bun's global folder, at this version.
    BunGlobal(String),
}

/// A server to start: one per distinct executable and root (PLAN §18.5).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerCommand {
    pub spec_id: &'static str,
    /// The executable, an absolute path where the repository lives.
    pub program: String,
    pub args: Vec<String>,
    /// The directory it serves and runs in, worktree-relative (`""` is the root).
    pub root: String,
    pub source: CommandSource,
    /// Environment variables it runs with besides the backend's: `PATH`, the machine's as its
    /// user's shell has it (PLAN §18.4), so the shims a server or its runtime goes through
    /// find what they call.
    pub env: Vec<(String, String)>,
}

impl ServerCommand {
    /// What `Backend::spawn_stdio` runs.
    pub fn process_spec(&self) -> ProcessSpec {
        let mut spec = ProcessSpec::new(self.program.clone())
            .args(self.args.iter().cloned())
            .cwd(self.root.clone());
        for (name, value) in &self.env {
            spec = spec.env(name.clone(), Some(value));
        }
        spec
    }

    /// The command line as the status bar's failure details show it.
    pub fn command_line(&self) -> String {
        std::iter::once(&self.program)
            .chain(&self.args)
            .map(|arg| {
                if arg.is_empty() || arg.contains([' ', '"', '\'']) {
                    format!("{arg:?}")
                } else {
                    arg.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Why a spec's server can't run for a file, and how to get it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Missing {
    pub spec_id: &'static str,
    pub reason: String,
    pub hint: String,
}

/// A spec's server for a file: found, or missing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Resolution {
    Found(ServerCommand),
    Missing(Missing),
}

/// Reads one environment variable.
type EnvReader = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Resolves servers for the files of one worktree, remembering each directory's answer (module
/// docs).
pub struct Resolver {
    backend: Arc<dyn Backend>,
    paths: WorkspacePaths,
    /// The worktree root as an absolute path; `None` when `toplevel` isn't one (never, for a
    /// backend that opened a repository).
    root: Option<AbsPath>,
    platform: Platform,
    settings: LspSettings,
    env: EnvReader,
    /// Answers by spec id and directory: `None` where the spec doesn't apply.
    cache: Mutex<HashMap<(&'static str, String), Option<Resolution>>>,
    /// Bumped by every [`Resolver::forget`]: an answer worked out before one isn't kept.
    generation: AtomicU64,
    /// Only `DV_LSP_<ID>` finds a server ([`Resolver::overrides_only`]).
    overrides_only: bool,
    /// The backend's machine: `PATH` and the home.
    host: Mutex<Arc<HostEnv>>,
    /// Each spec's worktree check (`LanguageServerSpec::applies`), by spec id.
    checks: Mutex<HashMap<&'static str, bool>>,
    /// What `PATH` and bun's global folder have of each spec, by spec id and resolver.
    machine: Mutex<HashMap<(&'static str, Machine), Option<Found>>>,
    /// The JavaScript runtime, by whether only node will do.
    runtimes: Mutex<HashMap<bool, Result<Option<JsRuntime>, RuntimeError>>>,
    /// Which runtimes the machine has, for a missing server's hint.
    seen: Mutex<Option<RuntimesSeen>>,
}

/// A resolver whose answer doesn't depend on the file's directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Machine {
    Path,
    BunGlobal,
}

/// What one resolver found for a spec: an executable to run, or a script a runtime runs.
struct Hit {
    program: String,
    script: bool,
    args: Vec<String>,
    source: CommandSource,
}

impl Resolver {
    /// A resolver over `backend`'s worktree on `platform`, with `settings.json`'s `lsp` and
    /// this process's environment (for `DV_LSP_<ID>`, and as the machine's `PATH` until
    /// [`Resolver::with_host`] says otherwise).
    pub fn new(backend: Arc<dyn Backend>, platform: Platform, settings: LspSettings) -> Self {
        let paths = WorkspacePaths::new(&backend.repo().toplevel);
        let root = AbsPath::parse(&backend.repo().toplevel, paths.style());
        Self {
            host: Mutex::new(Arc::new(HostEnv::this_process())),
            checks: Mutex::new(HashMap::new()),
            machine: Mutex::new(HashMap::new()),
            runtimes: Mutex::new(HashMap::new()),
            seen: Mutex::new(None),
            backend,
            paths,
            root,
            platform,
            settings,
            env: Box::new(|name| std::env::var(name).ok()),
            cache: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
            overrides_only: false,
        }
    }

    /// The environment read for `DV_LSP_<ID>`, in place of this process's.
    pub fn with_env(
        mut self,
        env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.env = Box::new(env);
        self
    }

    /// Finds servers only through `DV_LSP_<ID>`: the project, settings and `PATH` resolvers are
    /// off, so an automated run (the app under `--automation`, its tests) never starts a
    /// server it didn't name, whatever `node_modules` lies above its fixtures. Without the
    /// override a spec that applies is missing, saying so.
    pub fn overrides_only(mut self) -> Self {
        self.overrides_only = true;
        self
    }

    /// `host` as the backend's machine (a distro's, read from its user's shell).
    pub fn with_host(self, host: HostEnv) -> Self {
        *self.host.lock() = Arc::new(host);
        self
    }

    /// Takes `host` as the backend's machine from now on (a connection that came back):
    /// whether it differs from the one before, in which case every answer is forgotten.
    pub fn set_host(&self, host: HostEnv) -> bool {
        let changed = {
            let mut current = self.host.lock();
            let changed = **current != host;
            if changed {
                *current = Arc::new(host);
            }
            changed
        };
        if changed {
            self.forget();
        }
        changed
    }

    /// The backend's machine as the resolvers see it.
    pub fn host(&self) -> Arc<HostEnv> {
        self.host.lock().clone()
    }

    pub fn paths(&self) -> &WorkspacePaths {
        &self.paths
    }

    /// `spec`'s server for the worktree file `rel`: `None` when the spec doesn't serve the
    /// file or doesn't apply to its project (no root marker above it).
    pub fn resolve(
        &self,
        spec: &'static LanguageServerSpec,
        rel: &str,
    ) -> Result<Option<Resolution>, BackendError> {
        if spec.language_id(rel).is_none() {
            return Ok(None);
        }
        let dir = parent(rel).to_string();
        let key = (spec.id, dir.clone());
        if let Some(known) = self.cache.lock().get(&key) {
            return Ok(known.clone());
        }
        let Some(root) = &self.root else {
            return Err(BackendError::InvalidPath {
                path: self.backend.repo().toplevel.clone(),
                reason: "the worktree root isn't an absolute path".into(),
            });
        };
        let generation = self.generation.load(Ordering::SeqCst);
        let reader = Reader::new(self.backend.as_ref(), self.paths.style());
        let answer = if self.applies(&reader, root, spec, &dir)? {
            Some(self.resolve_in(&reader, root, spec, &dir)?)
        } else {
            None
        };
        {
            // A `forget` while this was worked out: the answer may be stale; it is given but
            // not kept.
            let mut cache = self.cache.lock();
            if self.generation.load(Ordering::SeqCst) == generation {
                cache.insert(key, answer.clone());
            }
        }
        Ok(answer)
    }

    /// [`Resolver::resolve`] without the remembered answer for `rel`'s directory: before a
    /// server starts again (PLAN §18.5), so an install since the last start is seen.
    pub fn resolve_fresh(
        &self,
        spec: &'static LanguageServerSpec,
        rel: &str,
    ) -> Result<Option<Resolution>, BackendError> {
        self.cache
            .lock()
            .remove(&(spec.id, parent(rel).to_string()));
        self.resolve(spec, rel)
    }

    /// Forgets every answer.
    pub fn forget(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cache.lock().clear();
        self.checks.lock().clear();
        self.machine.lock().clear();
        self.runtimes.lock().clear();
        *self.seen.lock() = None;
    }

    /// Tells the resolver which worktree paths changed (the review's watch): when one can
    /// change what resolves ([`affects_resolution`]), every answer is forgotten. Whether they
    /// were.
    pub fn files_changed<'p>(&self, rels: impl IntoIterator<Item = &'p str>) -> bool {
        // A spec whose worktree check said no watches the files that could make it say yes.
        let watching: Vec<fn(&str) -> bool> = {
            let checks = self.checks.lock();
            crate::languages::ALL
                .iter()
                .filter(|spec| checks.get(spec.id) == Some(&false))
                .filter_map(|spec| spec.applies.map(|check| check.watches))
                .collect()
        };
        let affected = rels
            .into_iter()
            .any(|rel| affects_resolution(rel) || watching.iter().any(|watches| watches(rel)));
        if affected {
            self.forget();
        }
        affected
    }

    fn resolve_in(
        &self,
        reader: &Reader<'_>,
        root: &AbsPath,
        spec: &'static LanguageServerSpec,
        dir: &str,
    ) -> Result<Resolution, BackendError> {
        let missing = |reason: String| {
            Resolution::Missing(Missing {
                spec_id: spec.id,
                reason,
                hint: spec.install_hint.to_string(),
            })
        };
        let spec_args = || spec.args.iter().map(|arg| arg.to_string()).collect();
        match Override::from_env(spec, &*self.env) {
            Ok(Some(found)) => {
                return Ok(Resolution::Found(ServerCommand {
                    spec_id: spec.id,
                    program: found.program,
                    args: found.args.unwrap_or_else(spec_args),
                    root: String::new(),
                    source: CommandSource::Override(found.var),
                    env: Vec::new(),
                }));
            }
            Ok(None) => {}
            Err(reason) => return Ok(missing(reason.to_string())),
        }
        if self.overrides_only {
            return Ok(missing(format!(
                "{} isn't set, and automated runs find language servers only through it",
                spec.override_var()
            )));
        }
        let host = self.host();
        let context = ResolveContext {
            reader,
            root,
            platform: &self.platform,
        };
        let js = spec.js().is_some();
        let mut first_reason = None;
        let mut looked: Vec<String> = Vec::new();
        for resolver in spec.resolvers {
            let hit = match resolver {
                ResolverKind::Project(project) => match project(&context, dir)? {
                    ProjectOutcome::Found { program, version } => Some(Hit {
                        program,
                        script: js,
                        args: spec_args(),
                        source: CommandSource::Project(version),
                    }),
                    ProjectOutcome::Unusable(reason) => {
                        first_reason.get_or_insert(reason);
                        None
                    }
                    ProjectOutcome::NotFound(places) => {
                        if !places.is_empty() {
                            looked.push(places);
                        }
                        None
                    }
                },
                ResolverKind::Settings => match self.settings.command(spec.id) {
                    // Absolute as the machine spells paths (POSIX in a distro).
                    Some([program, args @ ..])
                        if AbsPath::parse(program, self.paths.style()).is_some() =>
                    {
                        Some(Hit {
                            program: program.clone(),
                            script: js && js::is_script(program),
                            args: args.to_vec(),
                            source: CommandSource::Settings,
                        })
                    }
                    // A bare name is looked up on `PATH` and runs as found there.
                    Some([program, args @ ..]) => match host.find(reader, program)? {
                        Some(found) => Some(Hit {
                            program: found.at.native(),
                            script: js && js::is_script(program),
                            args: args.to_vec(),
                            source: CommandSource::Settings,
                        }),
                        None => {
                            first_reason.get_or_insert(format!(
                                "lsp.{}.command names {program}, which isn't on {}",
                                spec.id,
                                host.path_label()
                            ));
                            None
                        }
                    },
                    _ => None,
                },
                ResolverKind::Path if js => {
                    match self.machine(reader, &host, spec, Machine::Path)? {
                        Some(found) => hit(found, &host, spec, Machine::Path, &mut first_reason),
                        None => {
                            looked.push(format!("not on {}", host.path_label()));
                            None
                        }
                    }
                }
                ResolverKind::Path => match host.find(reader, spec.command_name)? {
                    Some(on_path) => Some(Hit {
                        program: on_path.at.native(),
                        script: false,
                        args: spec_args(),
                        source: CommandSource::Path(host.path_label()),
                    }),
                    None => {
                        looked.push(format!("not on {}", host.path_label()));
                        None
                    }
                },
                ResolverKind::BunGlobal => {
                    match self.machine(reader, &host, spec, Machine::BunGlobal)? {
                        Some(found) => {
                            hit(found, &host, spec, Machine::BunGlobal, &mut first_reason)
                        }
                        None => {
                            if let Some(dir) = js::bun_global_dir(&host) {
                                looked.push(format!("not in {dir}"));
                            }
                            None
                        }
                    }
                }
            };
            if let Some(hit) = hit {
                return self.command(reader, &host, spec, hit);
            }
        }
        let reason = first_reason.unwrap_or_else(|| {
            if looked.is_empty() {
                spec.missing.to_string()
            } else {
                format!("{} ({})", spec.missing, looked.join("; "))
            }
        });
        let hint = match spec.js() {
            Some(_) => js_hint(spec, self.runtimes_seen(reader, &host)?),
            None => spec.install_hint.to_string(),
        };
        Ok(Resolution::Missing(Missing {
            spec_id: spec.id,
            reason,
            hint,
        }))
    }

    /// What `PATH` or bun's global folder has of `spec`, remembered.
    fn machine(
        &self,
        reader: &Reader<'_>,
        host: &HostEnv,
        spec: &'static LanguageServerSpec,
        machine: Machine,
    ) -> Result<Option<Found>, BackendError> {
        let key = (spec.id, machine);
        if let Some(known) = self.machine.lock().get(&key) {
            return Ok(known.clone());
        }
        let found = match machine {
            Machine::Path => js::on_path(reader, host, spec)?,
            Machine::BunGlobal => js::bun_global(reader, host, spec)?,
        };
        self.machine.lock().insert(key, found.clone());
        Ok(found)
    }

    /// The command for `hit`: a script runs with the spec's runtime, and without one the server
    /// is missing, saying so.
    fn command(
        &self,
        reader: &Reader<'_>,
        host: &HostEnv,
        spec: &'static LanguageServerSpec,
        hit: Hit,
    ) -> Result<Resolution, BackendError> {
        let Hit {
            program,
            script,
            args,
            source,
        } = hit;
        let env: Vec<(String, String)> = host
            .raw_path()
            .map(|path| ("PATH".to_string(), path.to_string()))
            .into_iter()
            .collect();
        let found = |program: String, args: Vec<String>| {
            Resolution::Found(ServerCommand {
                spec_id: spec.id,
                program,
                args,
                root: String::new(),
                source: source.clone(),
                env: env.clone(),
            })
        };
        let Some(js) = spec.js().filter(|_| script) else {
            return Ok(found(program, args));
        };
        let known = self.runtimes.lock().get(&js.node_only).cloned();
        let runtime = match known {
            Some(known) => known,
            None => {
                let runtime = js::find_runtime(reader, host, self.settings.runtime(), js)?;
                self.runtimes.lock().insert(js.node_only, runtime.clone());
                runtime
            }
        };
        let missing = |reason: String, hint: &str| {
            Resolution::Missing(Missing {
                spec_id: spec.id,
                reason,
                hint: hint.to_string(),
            })
        };
        Ok(match runtime {
            Ok(Some(runtime)) => {
                let argv = runtime
                    .args
                    .into_iter()
                    .chain(std::iter::once(program))
                    .chain(args)
                    .collect();
                found(runtime.program, argv)
            }
            Ok(None) if js.node_only => missing(
                format!(
                    "{} runs with node, which isn't on {}",
                    spec.name,
                    host.path_label()
                ),
                "Install Node.js, or set `lsp.runtime` in settings.json",
            ),
            Ok(None) => {
                let places = match host.bun_dir() {
                    Some(bun) => format!("{} or in {}", host.path_label(), bun.join("bin")),
                    None => host.path_label(),
                };
                missing(
                    format!(
                        "{} runs with bun or node, and neither is on {places}",
                        spec.name
                    ),
                    "Install bun (bun.sh) or Node.js, or set `lsp.runtime` in settings.json",
                )
            }
            Err(error) => missing(error.to_string(), "Fix `lsp.runtime` in settings.json"),
        })
    }

    /// Which runtimes the machine has, remembered.
    fn runtimes_seen(
        &self,
        reader: &Reader<'_>,
        host: &HostEnv,
    ) -> Result<RuntimesSeen, BackendError> {
        if let Some(seen) = *self.seen.lock() {
            return Ok(seen);
        }
        let seen = js::runtimes_seen(reader, host)?;
        *self.seen.lock() = Some(seen);
        Ok(seen)
    }

    /// Whether `spec` applies to the worktree directory `dir`: one of its root markers in it or
    /// above it, up to the worktree root (any directory, for a spec without markers), and its
    /// worktree check, asked once.
    fn applies(
        &self,
        reader: &Reader<'_>,
        root: &AbsPath,
        spec: &'static LanguageServerSpec,
        dir: &str,
    ) -> Result<bool, BackendError> {
        if !spec.root_markers.is_empty() && !has_marker(reader, root, spec, dir)? {
            return Ok(false);
        }
        let Some(check) = spec.applies else {
            return Ok(true);
        };
        if let Some(known) = self.checks.lock().get(spec.id) {
            return Ok(*known);
        }
        let context = ResolveContext {
            reader,
            root,
            platform: &self.platform,
        };
        let applies = (check.check)(&context)?;
        self.checks.lock().insert(spec.id, applies);
        Ok(applies)
    }
}

/// A JavaScript server's [`Found`] as a hit; an unusable one's reason kept.
fn hit(
    found: Found,
    host: &HostEnv,
    spec: &LanguageServerSpec,
    machine: Machine,
    first_reason: &mut Option<String>,
) -> Option<Hit> {
    let source = |version: String| match machine {
        Machine::Path => CommandSource::Path(host.path_label()),
        Machine::BunGlobal => CommandSource::BunGlobal(version),
    };
    let (program, script, source) = match found {
        Found::Script { script, version } => (script, true, source(version)),
        Found::Executable(program) => (program, false, source(String::new())),
        Found::Unusable(reason) => {
            first_reason.get_or_insert(reason);
            return None;
        }
    };
    Some(Hit {
        program,
        script,
        args: spec.args.iter().map(|arg| arg.to_string()).collect(),
        source,
    })
}

/// How to get a missing JavaScript server, for the runtimes the machine has (PLAN §18.5): bun's
/// command first when bun is there, npm's when only node is.
fn js_hint(spec: &LanguageServerSpec, seen: RuntimesSeen) -> String {
    let Some(js) = spec.js() else {
        return spec.install_hint.to_string();
    };
    let package = js.package;
    let settings = format!("or set `lsp.{}.command` in settings.json", spec.id);
    match (seen.bun && !js.node_only, seen.node) {
        (true, _) => format!(
            "Install it with `bun add -g {package}` (or `npm install -g {package}`), {settings}"
        ),
        (false, true) => format!("Install it with `npm install -g {package}`, {settings}"),
        (false, false) if js.node_only => {
            format!("Install Node.js, then `npm install -g {package}`, {settings}")
        }
        (false, false) => format!(
            "Install bun (bun.sh) or Node.js, then `bun add -g {package}` (or `npm install -g \
             {package}`), {settings}"
        ),
    }
}

/// Whether one of `spec`'s root markers is in the worktree directory `dir` or above it, up to
/// the worktree root.
fn has_marker(
    reader: &Reader<'_>,
    root: &AbsPath,
    spec: &LanguageServerSpec,
    dir: &str,
) -> Result<bool, BackendError> {
    for ancestor in ancestors(dir) {
        let here = root.join(ancestor);
        for marker in spec.root_markers {
            if reader.exists(&here.join(marker))? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// The files whose change can change what resolves: a project's manifest, its lockfiles, its
/// TypeScript configuration (a root marker) and its Tailwind configuration (Tailwind CSS's
/// worktree check), and anything in a `node_modules` (which the review's watch doesn't report,
/// git ignoring it, but a caller might).
pub fn affects_resolution(rel: &str) -> bool {
    const FILES: &[&str] = &[
        "package.json",
        "package-lock.json",
        "npm-shrinkwrap.json",
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        "yarn.lock",
        "bun.lock",
        "bun.lockb",
        ".npmrc",
        "tsconfig.json",
        "jsconfig.json",
    ];
    let name = rel.rsplit('/').next().unwrap_or(rel);
    FILES.contains(&name)
        || name.starts_with("tailwind.config.")
        || rel.split('/').any(|part| part == "node_modules")
}

/// An absolute path on either kind of machine: `/x`, `C:\x`, `C:/x`, `\\server\share`.
pub fn is_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || path.starts_with("\\\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'))
}

/// The directory of a worktree path (`""` for a file in the root).
pub fn parent(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// `dir` (worktree-relative) and every directory above it, ending with the root (`""`).
pub fn ancestors(dir: &str) -> impl Iterator<Item = &str> {
    let mut next = Some(dir);
    std::iter::from_fn(move || {
        let current = next?;
        next = if current.is_empty() {
            None
        } else {
            Some(parent(current))
        };
        Some(current)
    })
}
