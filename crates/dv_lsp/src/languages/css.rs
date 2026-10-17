//! CSS, SCSS and Less through VS Code's CSS language server, `vscode-css-language-server
//! --stdio` (the `vscode-langservers-extracted` package, JavaScript, run with bun or node:
//! docs/PLAN.md §18.4). Its hover documents properties and selectors.
//!
//! It applies to every stylesheet. It comes last in the registry: checked 2026-10-13 under bun
//! on the owner's Tailwind v4 project, its go to definition found no custom property in a
//! Tailwind v4 stylesheet, even one defined in the same file, so it is for hover; definition
//! and references take another server's answer first (§18.4).
//!
//! Its settings answer `workspace/configuration`'s `css`, `scss` and `less` sections:
//! validation, and hover with documentation and references.

use dv_core::backend::BackendError;
use serde_json::{Value, json};

use crate::resolve::{ProjectOutcome, ResolveContext, js};
use crate::spec::{DocumentKind, JsServer, LanguageServerSpec, ResolverKind, Runtime};

pub static SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "css",
    name: "CSS",
    documents: &[
        DocumentKind {
            extension: "css",
            language_id: "css",
        },
        DocumentKind {
            extension: "scss",
            language_id: "scss",
        },
        DocumentKind {
            extension: "less",
            language_id: "less",
        },
    ],
    root_markers: &[],
    applies: None,
    resolvers: &[
        ResolverKind::Project(project),
        ResolverKind::Settings,
        ResolverKind::Path,
        ResolverKind::BunGlobal,
    ],
    command_name: "vscode-css-language-server",
    args: &["--stdio"],
    runtime: Runtime::Js(JsServer {
        package: "vscode-langservers-extracted",
        node_only: false,
    }),
    initialization_options: options,
    settings,
    missing: "The CSS language server isn't installed",
    install_hint: "Install it with `bun add -g vscode-langservers-extracted` (or `npm install -g \
                   vscode-langservers-extracted`), or set `lsp.css.command` in settings.json",
};

fn options() -> Value {
    json!({"provideFormatter": false})
}

/// The `css`, `scss` and `less` sections (module docs).
fn settings() -> Value {
    let language = json!({
        "validate": true,
        "hover": {"documentation": true, "references": true}
    });
    json!({"css": language, "scss": language, "less": language})
}

/// The project's own `vscode-langservers-extracted`, as Node finds it from `dir`.
pub fn project(context: &ResolveContext<'_>, dir: &str) -> Result<ProjectOutcome, BackendError> {
    js::project(context, dir, &SPEC)
}
