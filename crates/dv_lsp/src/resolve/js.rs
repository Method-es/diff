//! JavaScript servers (docs/PLAN.md §18.4 "JavaScript servers"): a server's package found, its
//! entry script read from the package's `bin`, and the runtime dv runs it with.
//!
//! dv runs the entry script itself, never the `bin` shim beside it: a shim's `#!/usr/bin/env
//! node` line would pick the runtime (and on Windows npm's shims are batch files, which
//! `spawn_stdio` refuses). A hit on `PATH` is traced back to its package: on POSIX the link's
//! real path (`~/.bun/bin/x` → `../install/global/node_modules/<package>/bin/x`), then the
//! nearest `package.json` above it with the package's name; on Windows npm's `<bin>.cmd` shim
//! is read for the script it calls (`"%dp0%\node_modules\<package>\bin\x"`), or the package is
//! found beside it in `node_modules`.
//!
//! The runtime is **bun** first, then **node**: `lsp.runtime` in settings, else bun on `PATH`,
//! else bun's default install (`~/.bun/bin/bun`, or under `BUN_INSTALL`), else node on `PATH`.
//! A spec whose server breaks under bun takes node only.

use dv_core::backend::BackendError;
use serde_json::Value;

use super::host::{HostEnv, PathHit, real_file};
use super::node::{self, AbsPath, Package, Reader, Search};
use super::{ProjectOutcome, ResolveContext};
use crate::paths::PathStyle;
use crate::spec::{JsServer, LanguageServerSpec};

/// Which runtime runs a script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeKind {
    Bun,
    Node,
    /// `lsp.runtime` names something else; it runs as given.
    Other,
}

impl RuntimeKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Bun => "bun",
            Self::Node => "node",
            Self::Other => "the configured runtime",
        }
    }
}

/// The runtime a JavaScript server runs with.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JsRuntime {
    pub kind: RuntimeKind,
    /// Its executable, an absolute path where the repository lives.
    pub program: String,
    /// Arguments before the script (`lsp.runtime` as an argument list).
    pub args: Vec<String>,
}

/// The runtimes on the backend's machine, as a missing server's hint needs them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimesSeen {
    pub bun: bool,
    pub node: bool,
}

/// Why `lsp.runtime` can't be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    /// It names a program that isn't an absolute path on the machine and isn't on its `PATH`.
    #[error("lsp.runtime names {name}, which isn't on {path}")]
    NotFound { name: String, path: String },
}

/// The runtime for `js` (module docs): `Ok(None)` when there is none, `Err` when
/// `lsp.runtime` names one dv can't use.
pub fn find_runtime(
    reader: &Reader<'_>,
    host: &HostEnv,
    configured: Option<&[String]>,
    js: &JsServer,
) -> Result<Result<Option<JsRuntime>, RuntimeError>, BackendError> {
    if let Some([name, args @ ..]) = configured {
        let kind = kind_of(name);
        if !(js.node_only && kind == RuntimeKind::Bun) {
            // Absolute as the machine spells paths: a distro's `C:\x` is no path there.
            let program = if AbsPath::parse(name, host.style()).is_some() {
                Some(name.clone())
            } else {
                host.find(reader, name)?.map(|hit| hit.at.native())
            };
            return Ok(match program {
                Some(program) => Ok(Some(JsRuntime {
                    kind,
                    program,
                    args: args.to_vec(),
                })),
                None => Err(RuntimeError::NotFound {
                    name: name.clone(),
                    path: host.path_label(),
                }),
            });
        }
    }
    if !js.node_only
        && let Some(bun) = find_bun(reader, host)?
    {
        return Ok(Ok(Some(bun)));
    }
    Ok(Ok(host.find(reader, "node")?.map(|hit| JsRuntime {
        kind: RuntimeKind::Node,
        program: hit.at.native(),
        args: Vec::new(),
    })))
}

/// Which of bun and node the machine has, for a missing server's hint.
pub fn runtimes_seen(reader: &Reader<'_>, host: &HostEnv) -> Result<RuntimesSeen, BackendError> {
    Ok(RuntimesSeen {
        bun: find_bun(reader, host)?.is_some(),
        node: host.find(reader, "node")?.is_some(),
    })
}

/// bun on `PATH`, else in its default install.
fn find_bun(reader: &Reader<'_>, host: &HostEnv) -> Result<Option<JsRuntime>, BackendError> {
    let bun = |at: &AbsPath| JsRuntime {
        kind: RuntimeKind::Bun,
        program: at.native(),
        args: Vec::new(),
    };
    if let Some(hit) = host.find(reader, "bun")? {
        return Ok(Some(bun(&hit.at)));
    }
    let Some(dir) = host.bun_dir() else {
        return Ok(None);
    };
    let file = match host.style() {
        PathStyle::Windows => "bin/bun.exe",
        PathStyle::Posix => "bin/bun",
    };
    let at = dir.join(file);
    Ok(real_file(reader, &at)?.map(|_| bun(&at)))
}

/// Which runtime an executable is, by its name.
fn kind_of(program: &str) -> RuntimeKind {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let stem = name
        .strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name);
    match stem {
        "bun" => RuntimeKind::Bun,
        "node" | "nodejs" => RuntimeKind::Node,
        _ => RuntimeKind::Other,
    }
}

/// Whether a configured command's program is a script a runtime runs.
pub fn is_script(program: &str) -> bool {
    [".js", ".mjs", ".cjs"]
        .iter()
        .any(|extension| program.to_ascii_lowercase().ends_with(extension))
}

/// What a JavaScript server's resolver found: its entry script, or why the thing found isn't
/// one dv can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// The script (absolute, links resolved) and its package's version.
    Script {
        script: String,
        version: String,
    },
    /// Not a script: an executable that runs as it is (a POSIX wrapper that picks its own
    /// runtime).
    Executable(String),
    Unusable(String),
}

/// The entry script of `package` that its `bin` named `bin` points at (a `bin` that is a string
/// is the package's only one).
pub fn entry_script(
    reader: &Reader<'_>,
    package: &Package,
    bin: &str,
) -> Result<Found, BackendError> {
    let name = package
        .manifest
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("the package");
    let relative = match package.manifest.get("bin") {
        Some(Value::String(path)) => Some(path.as_str()),
        Some(Value::Object(bins)) => bins.get(bin).and_then(Value::as_str),
        _ => None,
    };
    let Some(relative) = relative else {
        return Ok(Found::Unusable(format!(
            "{name}'s package.json names no bin {bin}"
        )));
    };
    let at = package.real_dir.join(relative);
    Ok(match real_file(reader, &at)? {
        Some(script) => Found::Script {
            script: script.native(),
            version: package.version().unwrap_or("").to_string(),
        },
        None => Found::Unusable(format!("{name}'s bin {bin} ({}) isn't there", at.native())),
    })
}

/// The project's own copy of `spec`'s package, found as Node finds it from the worktree
/// directory `dir` (a spec's project resolver).
pub fn project(
    context: &ResolveContext<'_>,
    dir: &str,
    spec: &LanguageServerSpec,
) -> Result<ProjectOutcome, BackendError> {
    let Some(js) = spec.js() else {
        return Ok(ProjectOutcome::NotFound(String::new()));
    };
    let reader = context.reader;
    match node::find_package(reader, &context.root.join(dir), js.package)? {
        Search::Found(package) => Ok(match entry_script(reader, &package, spec.command_name)? {
            Found::Script { script, version } => ProjectOutcome::Found {
                program: script,
                version,
            },
            Found::Executable(program) => ProjectOutcome::Found {
                program,
                version: package.version().unwrap_or("").to_string(),
            },
            Found::Unusable(reason) => ProjectOutcome::Unusable(reason),
        }),
        Search::NotFound { looked_in } => Ok(ProjectOutcome::NotFound(format!(
            "no node_modules/{} in {}",
            js.package,
            node::list_dirs(&looked_in)
        ))),
        Search::Unreadable {
            found_at,
            at,
            reason,
        } => Ok(ProjectOutcome::Unusable(format!(
            "{found_at} leads to {at}, which dv can't read: {reason}"
        ))),
    }
}

/// `spec`'s package in bun's global folder (`<bun>/install/global/node_modules`): `None` when
/// it isn't there.
pub fn bun_global(
    reader: &Reader<'_>,
    host: &HostEnv,
    spec: &LanguageServerSpec,
) -> Result<Option<Found>, BackendError> {
    let (Some(js), Some(dir)) = (spec.js(), bun_global_dir(host)) else {
        return Ok(None);
    };
    let found_at = dir.join(js.package);
    let node::RealPath::Found(real_dir) = node::real_path(reader, &found_at)? else {
        return Ok(None);
    };
    let Some(package) = read_package(reader, found_at, real_dir)? else {
        return Ok(None);
    };
    Ok(Some(entry_script(reader, &package, spec.command_name)?))
}

/// bun's global packages: `<bun>/install/global/node_modules`.
pub fn bun_global_dir(host: &HostEnv) -> Option<AbsPath> {
    Some(host.bun_dir()?.join("install/global/node_modules"))
}

/// The package whose real directory is `real_dir`, when it has a `package.json`.
fn read_package(
    reader: &Reader<'_>,
    found_at: AbsPath,
    real_dir: AbsPath,
) -> Result<Option<Package>, BackendError> {
    let Some(bytes) = reader.read_small(&real_dir.join("package.json"))? else {
        return Ok(None);
    };
    Ok(Some(Package {
        found_at,
        real_dir,
        manifest: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }))
}

/// `spec`'s server on `PATH`, traced to its entry script (module docs): `None` when `PATH`
/// doesn't have it.
pub fn on_path(
    reader: &Reader<'_>,
    host: &HostEnv,
    spec: &LanguageServerSpec,
) -> Result<Option<Found>, BackendError> {
    let Some(js) = spec.js() else {
        return Ok(None);
    };
    match host.style() {
        PathStyle::Posix => match host.find(reader, spec.command_name)? {
            Some(hit) => Ok(Some(trace_posix(reader, &hit, js, spec.command_name)?)),
            None => Ok(None),
        },
        PathStyle::Windows => {
            let shim = format!("{}.cmd", spec.command_name);
            match host.find_file(reader, &shim)? {
                Some(hit) => Ok(Some(trace_cmd_shim(reader, &hit, js, spec.command_name)?)),
                None => Ok(None),
            }
        }
    }
}

/// A POSIX `PATH` hit: its package's entry script, else the script itself when it is one, else
/// the executable as it is.
fn trace_posix(
    reader: &Reader<'_>,
    hit: &PathHit,
    js: &JsServer,
    bin: &str,
) -> Result<Found, BackendError> {
    if let Some(package) = package_above(reader, &hit.real, js.package)? {
        return entry_script(reader, &package, bin);
    }
    // A shim is a few lines; an executable is far larger than this and isn't read.
    let head = reader.read(&hit.real, 64 * 1024)?.unwrap_or_default();
    let text = String::from_utf8_lossy(&head).into_owned();
    let first_line = text.lines().next().unwrap_or("").to_string();
    // A package manager's POSIX shim (pnpm's, npm's `cmd-shim`): `exec node "$basedir/…"`,
    // its folder `$0`'s, the folder PATH has it in.
    if first_line.starts_with("#!")
        && first_line.contains("sh")
        && let (Some(relative), Some(dir)) = (shim_script(&text), hit.at.parent())
        && let Some(script) = real_file(reader, &dir.join(&relative))?
    {
        if let Some(package) = package_above(reader, &script, js.package)? {
            return entry_script(reader, &package, bin);
        }
        return Ok(Found::Script {
            script: script.native(),
            version: String::new(),
        });
    }
    Ok(
        if first_line.starts_with("#!")
            && (first_line.contains("node") || first_line.contains("bun"))
        {
            Found::Script {
                script: hit.real.native(),
                version: String::new(),
            }
        } else {
            Found::Executable(hit.at.native())
        },
    )
}

/// The nearest folder above `file` holding the package `name`'s `package.json`.
fn package_above(
    reader: &Reader<'_>,
    file: &AbsPath,
    name: &str,
) -> Result<Option<Package>, BackendError> {
    let Some(start) = file.parent() else {
        return Ok(None);
    };
    for dir in start.ancestors() {
        if dir.file_name() == Some("node_modules") {
            break;
        }
        let Some(package) = read_package(reader, dir.clone(), dir.clone())? else {
            continue;
        };
        if package.manifest.get("name").and_then(Value::as_str) == Some(name) {
            return Ok(Some(package));
        }
    }
    Ok(None)
}

/// npm's Windows shim `<bin>.cmd`: the script it calls, else the package beside it.
fn trace_cmd_shim(
    reader: &Reader<'_>,
    hit: &PathHit,
    js: &JsServer,
    bin: &str,
) -> Result<Found, BackendError> {
    let Some(dir) = hit.at.parent() else {
        return Ok(Found::Unusable(format!("{} has no folder", hit.at)));
    };
    let text = reader
        .read(&hit.real, dv_core::backend::PROBE_READ_SMALL)?
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    if let Some(relative) = shim_script(&text) {
        let script = dir.join(&relative.replace('\\', "/"));
        if let Some(real) = real_file(reader, &script)? {
            if let Some(package) = package_above(reader, &real, js.package)? {
                return entry_script(reader, &package, bin);
            }
            return Ok(Found::Script {
                script: real.native(),
                version: String::new(),
            });
        }
    }
    let beside = dir.join("node_modules").join(js.package);
    if let node::RealPath::Found(real_dir) = node::real_path(reader, &beside)?
        && let Some(package) = read_package(reader, beside, real_dir)?
    {
        return entry_script(reader, &package, bin);
    }
    Ok(Found::Unusable(format!(
        "{} doesn't say which script it runs",
        hit.at
    )))
}

/// The script a package manager's shim runs, relative to the shim's folder, with `/`
/// separators: the last quoted `%dp0%\…` (npm's `.cmd`), `%~dp0\…` (older npm, pnpm) or
/// `$basedir/…` (the POSIX shims pnpm and npm write) path that isn't node itself or another
/// batch file (yarn's shim calls the `.cmd` in its `.bin`, which dv doesn't follow).
pub fn shim_script(text: &str) -> Option<String> {
    let mut found: Option<(usize, String)> = None;
    for marker in ["%dp0%\\", "%~dp0\\", "$basedir/"] {
        let mut from = 0;
        while let Some(at) = text[from..].find(marker) {
            let start = from + at + marker.len();
            from = start;
            let rest = &text[start..];
            let end = rest.find(['"', '\r', '\n']).unwrap_or(rest.len());
            let path = rest[..end].trim().replace('\\', "/");
            let name = path
                .rsplit('/')
                .next()
                .unwrap_or(&path)
                .to_ascii_lowercase();
            let skipped = path.is_empty()
                || name == "node"
                || name == "node.exe"
                || name.ends_with(".cmd")
                || name.ends_with(".bat");
            if !skipped && found.as_ref().is_none_or(|(known, _)| *known < start) {
                found = Some((start, path));
            }
        }
    }
    found.map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npm_and_pnpm_shims_name_their_script() {
        let npm = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\n\
                   SETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \
                   \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n)\r\n\r\n\
                   endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \
                   \"%dp0%\\node_modules\\@tailwindcss\\language-server\\bin\\tailwindcss-language-server\" %*\r\n";
        assert_eq!(
            shim_script(npm).as_deref(),
            Some("node_modules/@tailwindcss/language-server/bin/tailwindcss-language-server")
        );
        let old = "@IF EXIST \"%~dp0\\node.exe\" (\r\n  \"%~dp0\\node.exe\"  \
                   \"%~dp0\\..\\lib\\cli.js\" %*\r\n) ELSE (\r\n  node  \"%~dp0\\..\\lib\\cli.js\" %*\r\n)";
        assert_eq!(shim_script(old).as_deref(), Some("../lib/cli.js"));
        assert_eq!(shim_script("@echo off\r\nnode x.js"), None);
        // pnpm's POSIX shim: the script, not `$basedir/node`.
        let pnpm = "#!/bin/sh\nbasedir=$(dirname \"$(echo \"$0\" | sed -e 's,\\\\,/,g')\")\n\
                    if [ -x \"$basedir/node\" ]; then\n  exec \"$basedir/node\"  \
                    \"$basedir/global/5/node_modules/@tailwindcss/language-server/bin/x\" \"$@\"\n\
                    else\n  exec node  \
                    \"$basedir/global/5/node_modules/@tailwindcss/language-server/bin/x\" \"$@\"\nfi\n";
        assert_eq!(
            shim_script(pnpm).as_deref(),
            Some("global/5/node_modules/@tailwindcss/language-server/bin/x")
        );
        // yarn's calls another batch file, which isn't followed.
        let yarn = "@\"%~dp0\\..\\Data\\global\\node_modules\\.bin\\x.cmd\" %*\r\n";
        assert_eq!(shim_script(yarn), None);
    }

    #[test]
    fn runtimes_are_told_apart_by_name() {
        assert_eq!(kind_of("/home/k/.bun/bin/bun"), RuntimeKind::Bun);
        assert_eq!(kind_of(r"C:\Users\k\.bun\bin\bun.exe"), RuntimeKind::Bun);
        assert_eq!(kind_of("/usr/bin/node"), RuntimeKind::Node);
        assert_eq!(kind_of("/opt/deno"), RuntimeKind::Other);
        assert!(is_script("/x/server.MJS") && is_script("/x/a.js") && !is_script("/x/a"));
    }
}
