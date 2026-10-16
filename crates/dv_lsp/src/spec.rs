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
    /// A JavaScript entry script dv runs with bun or node (PLAN §18.4, step 4).
    Js,
}

/// One way of finding a server's executable; a spec lists them in the order they are tried.
#[derive(Clone, Copy)]
pub enum ResolverKind {
    /// The project's own copy, found by the spec's function from a file's directory, reading
    /// the repository through its backend.
    Project(fn(&ResolveContext<'_>, &str) -> Result<ProjectOutcome, BackendError>),
    /// `lsp.<id>.command` in `settings.json`: an argument list, never a shell string.
    Settings,
}

impl std::fmt::Debug for ResolverKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolverKind::Project(_) => f.write_str("Project"),
            ResolverKind::Settings => f.write_str("Settings"),
        }
    }
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
    /// the worktree root.
    pub root_markers: &'static [&'static str],
    pub resolvers: &'static [ResolverKind],
    /// The arguments it runs with when found by the project resolver.
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
}
