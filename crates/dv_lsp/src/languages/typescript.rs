//! TypeScript and JavaScript through TypeScript 7's native server, `tsc --lsp --stdio`
//! (docs/PLAN.md §18.4).
//!
//! The project resolver is Node's resolution of `typescript` from the file's directory
//! ([`node::find_package`]). TypeScript 7 or later carries its server as a native executable in
//! a package per platform, `@typescript/typescript-<os>-<arch>`, which `typescript` depends on
//! and which therefore resolves from `typescript`'s real directory: its `lib/tsc` (`tsc.exe` on
//! Windows). A project on TypeScript 6 or older needs `typescript-language-server`, which has no
//! spec yet. No Node is needed: the server is the executable itself.
//!
//! Checked 2026-10-17 on the owner's bun monorepo in WSL: `node_modules/typescript` links to
//! `.bun/typescript@7.0.2/node_modules/typescript`, whose sibling
//! `@typescript/typescript-linux-x64` links to
//! `.bun/@typescript+typescript-linux-x64@7.0.2/node_modules/@typescript/typescript-linux-x64`;
//! every workspace package's own `node_modules/typescript` links to the same folder, so the
//! whole repository gets one server.

use dv_core::backend::BackendError;
use serde_json::{Value, json};

use crate::resolve::node::{self, RealPath, Search};
use crate::resolve::{Platform, ProjectOutcome, ResolveContext};
use crate::spec::{DocumentKind, LanguageServerSpec, ResolverKind, Runtime};

pub static SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "typescript",
    name: "TypeScript",
    documents: &[
        DocumentKind {
            extension: "ts",
            language_id: "typescript",
        },
        DocumentKind {
            extension: "mts",
            language_id: "typescript",
        },
        DocumentKind {
            extension: "cts",
            language_id: "typescript",
        },
        DocumentKind {
            extension: "tsx",
            language_id: "typescriptreact",
        },
        DocumentKind {
            extension: "js",
            language_id: "javascript",
        },
        DocumentKind {
            extension: "mjs",
            language_id: "javascript",
        },
        DocumentKind {
            extension: "cjs",
            language_id: "javascript",
        },
        DocumentKind {
            extension: "jsx",
            language_id: "javascriptreact",
        },
    ],
    root_markers: &["tsconfig.json", "jsconfig.json", "package.json"],
    applies: None,
    resolvers: &[ResolverKind::Project(project), ResolverKind::Settings],
    command_name: "tsc",
    args: &["--lsp", "--stdio"],
    runtime: Runtime::Native,
    initialization_options: no_options,
    settings: no_settings,
    missing: "TypeScript 7 isn't installed in this project",
    install_hint: "Add typescript@7 to the project (`npm install -D typescript@7`, or pnpm's or \
                   bun's), or set `lsp.typescript.command` in settings.json",
};

fn no_options() -> Value {
    Value::Null
}

fn no_settings() -> Value {
    json!({})
}

/// The project's TypeScript 7 server for files in the worktree directory `dir` (module docs).
pub fn project(context: &ResolveContext<'_>, dir: &str) -> Result<ProjectOutcome, BackendError> {
    let ResolveContext {
        reader,
        root,
        platform,
    } = *context;
    let typescript = match node::find_package(reader, &root.join(dir), "typescript")? {
        Search::Found(package) => package,
        Search::NotFound { looked_in } => {
            return Ok(ProjectOutcome::NotFound(format!(
                "no node_modules/typescript in {}",
                node::list_dirs(&looked_in)
            )));
        }
        Search::Unreadable {
            found_at,
            at,
            reason,
        } => {
            return Ok(ProjectOutcome::Unusable(format!(
                "{found_at} leads to {at}, which dv can't read: {reason}"
            )));
        }
    };
    let version = typescript.version().unwrap_or("").to_string();
    match node::major_version(&version) {
        Some(major) if major >= 7 => {}
        Some(_) => {
            return Ok(ProjectOutcome::Unusable(format!(
                "TypeScript {version} needs typescript-language-server, which dv doesn't run yet"
            )));
        }
        None => {
            return Ok(ProjectOutcome::Unusable(format!(
                "{}/package.json has no version dv can read",
                typescript.found_at
            )));
        }
    }
    let Some(os) = platform.node_os() else {
        return Ok(ProjectOutcome::Unusable(format!(
            "TypeScript has no native server for {}",
            platform.os
        )));
    };
    let executable = if platform.is_windows() {
        "lib/tsc.exe"
    } else {
        "lib/tsc"
    };
    // The native CPU's package first, then the one dv's own build could run (an emulated dv).
    let packages: Vec<String> = platform
        .archs()
        .iter()
        .filter_map(|arch| Platform::node_arch(arch))
        .map(|arch| format!("@typescript/typescript-{os}-{arch}"))
        .collect();
    for package in &packages {
        let native = match node::find_package(reader, &typescript.real_dir, package)? {
            Search::Found(native) => native,
            Search::NotFound { .. } => continue,
            Search::Unreadable {
                found_at,
                at,
                reason,
            } => {
                return Ok(ProjectOutcome::Unusable(format!(
                    "{found_at} leads to {at}, which dv can't read: {reason}"
                )));
            }
        };
        return Ok(
            match node::real_path(reader, &native.real_dir.join(executable))? {
                RealPath::Found(program) => ProjectOutcome::Found {
                    program: program.native(),
                    version,
                },
                _ => ProjectOutcome::Unusable(format!(
                    "{package} {} has no {executable}",
                    native.version().unwrap_or("")
                )),
            },
        );
    }
    Ok(ProjectOutcome::Unusable(match packages.first() {
        Some(package) => format!(
            "TypeScript {version} is installed without {package}, its server for {os} {}",
            Platform::node_arch(platform.arch()).unwrap_or(platform.arch())
        ),
        None => format!(
            "TypeScript has no native server for {} {}",
            platform.os,
            platform.arch()
        ),
    }))
}
