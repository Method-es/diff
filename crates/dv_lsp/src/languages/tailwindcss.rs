//! Tailwind CSS through its language server, `@tailwindcss/language-server`
//! (`tailwindcss-language-server --stdio`, JavaScript, run with bun or node: docs/PLAN.md
//! §18.4). Its hover shows the CSS a class stands for.
//!
//! **Where it applies.** A worktree that uses Tailwind: a stylesheet that imports it (v4's
//! `@import "tailwindcss"`, also `@import "tailwindcss/…"` and v3's `@tailwind` directives) or a
//! `tailwind.config.*` (v3). The check asks git (tracked files, and untracked ones git doesn't
//! ignore): `ls-files` for a config, then one `git grep` over every stylesheet for the import.
//! It is remembered until the resolver forgets: a `tailwind.config.*` or a `package.json`
//! changing, and while the answer is no, a stylesheet changing. One server serves the whole
//! worktree:
//! the server itself finds each project's stylesheet and the files it covers, so a component
//! package of a monorepo whose classes another package's stylesheet scans is served too.
//!
//! **Its files** are the server's own defaults: HTML and the templates that embed it (`.html`,
//! `.vue`, `.svelte`, `.astro`), stylesheets (`.css`, `.scss`, `.less`) and JavaScript and
//! TypeScript (`.js .jsx .mjs .cjs .ts .tsx .mts .cts`), where class names sit in JSX and in
//! helpers' strings.
//!
//! **Its settings** answer `workspace/configuration`'s `tailwindCSS` and `editor` sections
//! with the defaults the extension documents (hovers on, `class` / `className` / `ngClass` /
//! `class:list` attributes, a 16 px root font size, the lint levels), and the class functions
//! the common helpers name (`cn`, `cva`, `clsx`, `cx`, `tw`: shadcn's `cn` and `cva` are in the
//! owner's project), whose string arguments it then reads as class lists. The extension's own
//! default is none.
//!
//! Checked 2026-10-13 under bun 1.4.2 on the owner's Tailwind v4 project: its first hover
//! answers after about 9 s while it scans the project, so it starts with the review (§18.5).

use dv_core::backend::{BackendError, GitOpts};
use serde_json::{Value, json};

use crate::resolve::{ProjectOutcome, ResolveContext, js};
use crate::spec::{
    DocumentKind, JsServer, LanguageServerSpec, ResolverKind, Runtime, WorktreeCheck,
};

pub static SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "tailwindcss",
    name: "Tailwind CSS",
    documents: &[
        DocumentKind {
            extension: "html",
            language_id: "html",
        },
        DocumentKind {
            extension: "vue",
            language_id: "vue",
        },
        DocumentKind {
            extension: "svelte",
            language_id: "svelte",
        },
        DocumentKind {
            extension: "astro",
            language_id: "astro",
        },
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
    ],
    root_markers: &[],
    applies: Some(WorktreeCheck {
        check: uses_tailwind,
        watches: is_stylesheet,
    }),
    resolvers: &[
        ResolverKind::Project(project),
        ResolverKind::Settings,
        ResolverKind::Path,
        ResolverKind::BunGlobal,
    ],
    command_name: "tailwindcss-language-server",
    args: &["--stdio"],
    runtime: Runtime::Js(JsServer {
        package: "@tailwindcss/language-server",
        node_only: false,
    }),
    initialization_options: options,
    settings,
    missing: "Tailwind CSS's language server isn't installed",
    install_hint: "Install it with `bun add -g @tailwindcss/language-server` (or `npm install -g \
                   @tailwindcss/language-server`), or set `lsp.tailwindcss.command` in \
                   settings.json",
};

fn options() -> Value {
    json!({})
}

/// The `tailwindCSS` and `editor` sections (module docs).
fn settings() -> Value {
    json!({
        "editor": {"tabSize": 2},
        "tailwindCSS": {
            "emmetCompletions": false,
            "classAttributes": ["class", "className", "ngClass", "class:list"],
            "classFunctions": ["cn", "cva", "clsx", "cx", "tw"],
            "includeLanguages": {},
            "files": {"exclude": [
                "**/.git/**", "**/node_modules/**", "**/.hg/**", "**/.svn/**"
            ]},
            "colorDecorators": true,
            "showPixelEquivalents": true,
            "rootFontSize": 16,
            "hovers": true,
            "suggestions": true,
            "codeActions": true,
            "validate": true,
            "lint": {
                "cssConflict": "warning",
                "invalidApply": "error",
                "invalidScreen": "error",
                "invalidVariant": "error",
                "invalidConfigPath": "error",
                "invalidTailwindDirective": "error",
                "invalidSourceDirective": "error",
                "recommendedVariantOrder": "warning",
                "usedBlocklistedClass": "warning"
            },
            "experimental": {"classRegex": [], "configFile": null}
        }
    })
}

/// The project's own `@tailwindcss/language-server`, as Node finds it from `dir`.
pub fn project(context: &ResolveContext<'_>, dir: &str) -> Result<ProjectOutcome, BackendError> {
    js::project(context, dir, &SPEC)
}

/// Whether the worktree uses Tailwind CSS (module docs): a `tailwind.config.*` (`git ls-files`),
/// else a stylesheet bringing it in (`git grep`, tracked files and the untracked ones git doesn't
/// ignore, every one of them).
pub fn uses_tailwind(context: &ResolveContext<'_>) -> Result<bool, BackendError> {
    let backend = context.reader.backend();
    let configs = backend.git(
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            "*tailwind.config.*",
        ],
        &GitOpts::new(),
    )?;
    let has_config = String::from_utf8_lossy(&configs.stdout)
        .split('\0')
        .any(|path| is_config(path) && !path.split('/').any(|part| part == "node_modules"));
    if has_config {
        return Ok(true);
    }
    // `git grep` exits 1 when nothing matches.
    let mut args = vec![
        "grep",
        "-l",
        "-I",
        "--untracked",
        "-E",
        IMPORT_PATTERN,
        "--",
    ];
    args.extend(STYLESHEETS);
    let found = backend.git(&args, &GitOpts::new().with_ok_codes(&[0, 1]))?;
    Ok(String::from_utf8_lossy(&found.stdout)
        .lines()
        .any(|path| !path.split('/').any(|part| part == "node_modules")))
}

/// The stylesheets the check greps.
const STYLESHEETS: [&str; 6] = ["*.css", "*.scss", "*.sass", "*.less", "*.pcss", "*.postcss"];

/// A line bringing Tailwind in, as `git grep -E` matches it ([`imports_tailwind`]'s rule).
const IMPORT_PATTERN: &str = "^[[:space:]]*@(tailwind[[:space:]]|import[[:space:]]+(url\\([[:space:]]*)?[\"']tailwindcss[\"'/])";

/// A stylesheet the check reads.
pub fn is_stylesheet(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    STYLESHEETS
        .iter()
        .any(|glob| lower.ends_with(glob.trim_start_matches('*')))
}

/// A Tailwind v3 configuration: `tailwind.config.{js,cjs,mjs,ts,cts,mts}`.
pub fn is_config(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_prefix("tailwind.config.")
        .is_some_and(|ext| ["js", "cjs", "mjs", "ts", "cts", "mts"].contains(&ext))
}

/// Whether a stylesheet brings Tailwind in: `@import "tailwindcss"` (or `'tailwindcss/…'`,
/// `url(…)`) or a `@tailwind` directive.
pub fn imports_tailwind(css: &str) -> bool {
    css.lines().map(str::trim_start).any(|line| {
        if line.starts_with("@tailwind ") {
            return true;
        }
        let Some(rest) = line.strip_prefix("@import") else {
            return false;
        };
        let rest = rest.trim_start();
        let rest = rest.strip_prefix("url(").unwrap_or(rest).trim_start();
        let Some(rest) = rest.strip_prefix(['"', '\'']) else {
            return false;
        };
        rest.strip_prefix("tailwindcss")
            .is_some_and(|after| after.starts_with(['"', '\'', '/']))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stylesheets_that_bring_tailwind_in() {
        assert!(imports_tailwind(
            "@import \"tailwindcss\" source(\"../\");\n@import \"./x.css\";\n"
        ));
        assert!(imports_tailwind("  @import 'tailwindcss';"));
        assert!(imports_tailwind(
            "@import url(\"tailwindcss/theme.css\") layer(theme);"
        ));
        assert!(imports_tailwind("@tailwind base;\n@tailwind components;"));
        assert!(!imports_tailwind("@import \"tailwindcss-animate\";"));
        assert!(!imports_tailwind("@import \"./tailwindcss.css\";"));
        assert!(!imports_tailwind("/* @import \"x\" */ body { color: red }"));
        assert!(is_config("apps/web/tailwind.config.ts"));
        assert!(is_config("tailwind.config.cjs"));
        assert!(!is_config("tailwind.config.json") && !is_config("src/tailwind.ts"));
    }
}
