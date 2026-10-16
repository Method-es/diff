//! Finding a spec's server for a file (docs/PLAN.md §18.4): the `DV_LSP_<ID>` override, else
//! the spec's resolvers in order (the project's own copy, then `settings.json`), else missing
//! with the reason and how to get it. Every resolver reads the repository through its
//! `Backend`, so a WSL repository is resolved inside its distro, against the distro's files
//! and the distro's platform.
//!
//! A resolver reads the backend's machine read-only, inside the worktree and outside it
//! (`Backend::probe_path`): Node resolves packages above the worktree root and through links
//! that leave it ([`node`]). `PATH` (and bun's global folder) come with the JavaScript servers
//! (PLAN §18.4, step 4); TypeScript's resolvers are the project and settings.
//!
//! The [`Resolver`] remembers each directory's answer. The review's watch never reports
//! `node_modules` (git ignores it), so the app tells it about the tracked files an install
//! changes ([`Resolver::files_changed`]: a `package.json`, a lockfile), and resolves afresh
//! before each restart of a server ([`Resolver::resolve_fresh`], PLAN §18.5).

pub mod node;
pub mod platform;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;

use dv_core::backend::{Backend, BackendError, ProcessSpec};
use parking_lot::Mutex;

pub use platform::Platform;

use crate::paths::WorkspacePaths;
use crate::settings::{LspSettings, Override};
use crate::spec::{LanguageServerSpec, ResolverKind};
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
}

impl ServerCommand {
    /// What `Backend::spawn_stdio` runs.
    pub fn process_spec(&self) -> ProcessSpec {
        ProcessSpec::new(self.program.clone())
            .args(self.args.iter().cloned())
            .cwd(self.root.clone())
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
}

impl Resolver {
    /// A resolver over `backend`'s worktree on `platform`, with `settings.json`'s `lsp` and
    /// this process's environment (for `DV_LSP_<ID>`).
    pub fn new(backend: Arc<dyn Backend>, platform: Platform, settings: LspSettings) -> Self {
        let paths = WorkspacePaths::new(&backend.repo().toplevel);
        let root = AbsPath::parse(&backend.repo().toplevel, paths.style());
        Self {
            backend,
            paths,
            root,
            platform,
            settings,
            env: Box::new(|name| std::env::var(name).ok()),
            cache: Mutex::new(HashMap::new()),
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
        let reader = Reader::new(self.backend.as_ref(), self.paths.style());
        let answer = if applies(&reader, root, spec, &dir)? {
            Some(self.resolve_in(&reader, root, spec, &dir)?)
        } else {
            None
        };
        self.cache.lock().insert(key, answer.clone());
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
        self.cache.lock().clear();
    }

    /// Tells the resolver which worktree paths changed (the review's watch): when one can
    /// change what resolves ([`affects_resolution`]), every answer is forgotten. Whether they
    /// were.
    pub fn files_changed<'p>(&self, rels: impl IntoIterator<Item = &'p str>) -> bool {
        let affected = rels.into_iter().any(affects_resolution);
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
                }));
            }
            Ok(None) => {}
            Err(reason) => return Ok(missing(reason.to_string())),
        }
        let context = ResolveContext {
            reader,
            root,
            platform: &self.platform,
        };
        let mut first_reason = None;
        let mut looked = None;
        for resolver in spec.resolvers {
            match resolver {
                ResolverKind::Project(project) => match project(&context, dir)? {
                    ProjectOutcome::Found { program, version } => {
                        return Ok(Resolution::Found(ServerCommand {
                            spec_id: spec.id,
                            program,
                            args: spec_args(),
                            root: String::new(),
                            source: CommandSource::Project(version),
                        }));
                    }
                    ProjectOutcome::Unusable(reason) => {
                        first_reason.get_or_insert(reason);
                    }
                    ProjectOutcome::NotFound(places) => {
                        looked.get_or_insert(places);
                    }
                },
                ResolverKind::Settings => {
                    let Some([program, args @ ..]) = self.settings.command(spec.id) else {
                        continue;
                    };
                    if !is_absolute(program) {
                        first_reason.get_or_insert(format!(
                            "lsp.{}.command names {program}, which isn't an absolute path",
                            spec.id
                        ));
                        continue;
                    }
                    return Ok(Resolution::Found(ServerCommand {
                        spec_id: spec.id,
                        program: program.clone(),
                        args: args.to_vec(),
                        root: String::new(),
                        source: CommandSource::Settings,
                    }));
                }
            }
        }
        Ok(missing(first_reason.unwrap_or_else(|| match looked {
            Some(looked) => format!("{} ({looked})", spec.missing),
            None => spec.missing.to_string(),
        })))
    }
}

/// Whether one of `spec`'s root markers is in the worktree directory `dir` or above it, up to
/// the worktree root.
fn applies(
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

/// The files whose change can change what resolves: a project's manifest, its lockfiles and
/// its TypeScript configuration (a root marker), and anything in a `node_modules` (which the
/// review's watch doesn't report, git ignoring it, but a caller might).
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
    FILES.contains(&name) || rel.split('/').any(|part| part == "node_modules")
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
