//! A language server as data (docs/PLAN.md §18.4): which documents it serves, when it applies
//! to a project, how its executable is found and how it is started. Each spec is one module of
//! [`crate::languages`].

use dv_core::backend::BackendError;
use serde_json::Value;

use crate::resolve::{ProjectOutcome, ResolveContext};

/// One kind of document a server serves: files with this extension, opened with this language
/// id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentKind {
    /// Without the dot, lower case.
    pub extension: &'static str,
    pub language_id: &'static str,
}

/// How a server runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    /// An executable of its own (TypeScript 7's `tsc`).
    Native,
    /// A JavaScript server: dv runs its package's entry script with bun or node (PLAN §18.4
    /// "JavaScript servers").
    Js(JsServer),
}

/// A JavaScript server's package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsServer {
    /// The npm package: `@tailwindcss/language-server`.
    pub package: &'static str,
    /// Only node runs it: a server known to break under bun.
    pub node_only: bool,
}

/// One way of finding a server's executable; a spec lists them in the order they are tried.
#[derive(Clone, Copy)]
pub enum ResolverKind {
    /// The project's own copy, found by the spec's function from a file's directory, reading
    /// the repository through its backend.
    Project(fn(&ResolveContext<'_>, &str) -> Result<ProjectOutcome, BackendError>),
    /// `lsp.<id>.command` in `settings.json`: an argument list, never a shell string.
    Settings,
    /// [`LanguageServerSpec::command_name`] on the backend's `PATH` (a distro's from its user's
    /// shell); a JavaScript server's hit traced to its package's entry script.
    Path,
    /// A JavaScript server's package in bun's global folder
    /// (`~/.bun/install/global/node_modules`, where `bun add -g` puts it).
    BunGlobal,
}

impl std::fmt::Debug for ResolverKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolverKind::Project(_) => f.write_str("Project"),
            ResolverKind::Settings => f.write_str("Settings"),
            ResolverKind::Path => f.write_str("Path"),
            ResolverKind::BunGlobal => f.write_str("BunGlobal"),
        }
    }
}

/// Whether a server applies to a worktree at all, beyond its root markers (Tailwind CSS: a
/// stylesheet imports it). Asked once per worktree and remembered until the resolver forgets.
#[derive(Debug, Clone, Copy)]
pub struct WorktreeCheck {
    pub check: fn(&ResolveContext<'_>) -> Result<bool, BackendError>,
    /// The worktree paths whose change can turn a `false` into a `true` (a stylesheet that now
    /// imports Tailwind): while the check is `false`, one of them changing makes the resolver
    /// forget.
    pub watches: fn(&str) -> bool,
}

/// A language server (module docs).
#[derive(Debug)]
pub struct LanguageServerSpec {
    /// The id settings and the override use: `lsp.<id>.command`, `DV_LSP_<ID>`.
    pub id: &'static str,
    /// What the status bar calls it.
    pub name: &'static str,
    pub documents: &'static [DocumentKind],
    /// The server applies to a file with one of these in its directory or one above it, up to
    /// the worktree root; none: every directory.
    pub root_markers: &'static [&'static str],
    /// A check of the whole worktree the server also needs to pass, if any.
    pub applies: Option<WorktreeCheck>,
    pub resolvers: &'static [ResolverKind],
    /// The executable's name on `PATH`, without `.exe`: a native server's program, a
    /// JavaScript server's `bin` (its `package.json` names the entry script under it).
    pub command_name: &'static str,
    /// The arguments it runs with when found by a resolver other than settings.
    pub args: &'static [&'static str],
    pub runtime: Runtime,
    pub initialization_options: fn() -> Value,
    /// What `workspace/configuration` answers from.
    pub settings: fn() -> Value,
    /// Why it is missing when no resolver found anything, and how to get it.
    pub missing: &'static str,
    pub install_hint: &'static str,
}

impl LanguageServerSpec {
    /// The language id this server opens `path` with, `None` when it doesn't serve it.
    pub fn language_id(&self, path: &str) -> Option<&'static str> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let (_, extension) = name.rsplit_once('.')?;
        let extension = extension.to_ascii_lowercase();
        self.documents
            .iter()
            .find(|kind| kind.extension == extension)
            .map(|kind| kind.language_id)
    }

    /// The name of the variable that overrides the resolvers: `DV_LSP_<ID>`, the id upper
    /// case with `-` as `_`.
    pub fn override_var(&self) -> String {
        format!("DV_LSP_{}", self.id.to_ascii_uppercase().replace('-', "_"))
    }

    /// Its package when it is a JavaScript server.
    pub fn js(&self) -> Option<&JsServer> {
        match &self.runtime {
            Runtime::Js(js) => Some(js),
            Runtime::Native => None,
        }
    }
}
