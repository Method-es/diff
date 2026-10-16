//! JavaScript servers and `PATH` (docs/PLAN.md §18.4, §18.7): Tailwind CSS from bun's global
//! folder, a `PATH` link traced to its package, npm's Windows `.cmd` shim, the runtime order
//! (`lsp.runtime`, bun, node; a node-only spec), what a missing one says per machine, and
//! where Tailwind CSS applies (v4's stylesheet, v3's config).

use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::*;
use crate::languages::{css, tailwindcss};
use crate::paths::PathStyle;
use crate::spec::{DocumentKind, JsServer, Runtime};

const PACKAGE: &str = "@tailwindcss/language-server";
const BIN: &str = "tailwindcss-language-server";

/// How this OS spells paths, as the resolver reads the fixture.
fn style() -> PathStyle {
    if cfg!(windows) {
        PathStyle::Windows
    } else {
        PathStyle::Posix
    }
}

fn native(path: &Path) -> String {
    AbsPath::parse(&path.display().to_string(), style())
        .unwrap()
        .native()
}

/// A machine whose `PATH` is `dirs` and whose home is `home`.
fn host(dirs: &[&Path], home: Option<&Path>) -> HostEnv {
    let separator = if cfg!(windows) { ";" } else { ":" };
    let path: Vec<String> = dirs.iter().map(|dir| native(dir)).collect();
    HostEnv::new(
        style(),
        Some(&path.join(separator)),
        home.map(native).as_deref(),
        None,
        PathFrom::Process,
    )
}

/// The `PATH` a server of [`host`]`(dirs, …)` runs with.
fn path_env(dirs: &[&Path]) -> Vec<(String, String)> {
    let separator = if cfg!(windows) { ";" } else { ":" };
    let path: Vec<String> = dirs.iter().map(|dir| native(dir)).collect();
    vec![("PATH".to_string(), path.join(separator))]
}

/// An executable's file name on this OS.
fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn write(path: &Path, text: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A fake runtime `name` in `dir`; its path.
fn runtime(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(exe(name));
    write(&path, b"\x7fELF");
    path
}

/// The Tailwind server's package at `dir`, version 0.14.29; its entry script.
fn tailwind_package(dir: &Path) -> PathBuf {
    write(
        &dir.join("package.json"),
        format!(
            r#"{{"name": "{PACKAGE}", "version": "0.14.29", "bin": {{"{BIN}": "./bin/{BIN}"}}}}"#
        ),
    );
    let script = dir.join("bin").join(BIN);
    write(
        &script,
        "#!/usr/bin/env node\nrequire('../dist/index.js')\n",
    );
    script
}

/// A project using Tailwind v4: a stylesheet imports it.
fn tailwind_project(repo: &FixtureRepo) {
    repo.write("package.json", r#"{"name": "web"}"#);
    repo.write(
        "src/styles/app.css",
        "@import \"tailwindcss\" source(\"../\");\n",
    );
    repo.write(
        "src/App.tsx",
        "export const App = () => <div className=\"p-4\" />;\n",
    );
    repo.commit("project");
}

fn js_resolver(backend: Arc<dyn Backend>, host: HostEnv, settings: LspSettings) -> Resolver {
    Resolver::new(backend, Platform::new(LINUX.0, LINUX.1), settings)
        .with_env(|_| None)
        .with_host(host)
}

fn resolved(resolver: &Resolver, spec: &'static LanguageServerSpec, rel: &str) -> Resolution {
    resolver
        .resolve(spec, rel)
        .unwrap()
        .unwrap_or_else(|| panic!("{} doesn't apply to {rel}", spec.id))
}

fn command(resolver: &Resolver, spec: &'static LanguageServerSpec, rel: &str) -> ServerCommand {
    match resolved(resolver, spec, rel) {
        Resolution::Found(command) => command,
        Resolution::Missing(missing) => panic!("expected a server for {rel}, got {missing:?}"),
    }
}

fn why_missing(resolver: &Resolver, spec: &'static LanguageServerSpec, rel: &str) -> Missing {
    match resolved(resolver, spec, rel) {
        Resolution::Missing(missing) => missing,
        Resolution::Found(command) => panic!("expected nothing for {rel}, got {command:?}"),
    }
}

#[test]
fn bun_globals_tailwind_runs_under_bun() {
    let repo = FixtureRepo::new();
    tailwind_project(&repo);
    let home = repo.outside_dir("user");
    let bun = runtime(&home.join(".bun/bin"), "bun");
    let script = tailwind_package(&home.join(".bun/install/global/node_modules").join(PACKAGE));
    // bun isn't on PATH (a distro's `wsl --exec` PATH): its default install is used.
    let empty = repo.outside_dir("empty");
    for backend in [local(&repo), remote(&repo)] {
        let resolver = js_resolver(
            backend,
            host(&[&empty], Some(&home)),
            LspSettings::default(),
        );
        assert_eq!(
            command(&resolver, &tailwindcss::SPEC, "src/App.tsx"),
            ServerCommand {
                spec_id: "tailwindcss",
                program: native(&bun),
                args: vec![native(&script), "--stdio".into()],
                root: String::new(),
                source: CommandSource::BunGlobal("0.14.29".into()),
                env: path_env(&[&empty]),
            }
        );
        // A stylesheet and a TypeScript file of the project get the same server.
        assert_eq!(
            command(&resolver, &tailwindcss::SPEC, "src/styles/app.css").program,
            native(&bun)
        );
    }
}

#[test]
fn the_projects_own_copy_comes_first() {
    let repo = FixtureRepo::new();
    tailwind_project(&repo);
    let script = tailwind_package(&repo.path().join("node_modules").join(PACKAGE));
    let bin = repo.outside_dir("bin");
    let node = runtime(&bin, "node");
    let resolver = js_resolver(local(&repo), host(&[&bin], None), LspSettings::default());
    let found = command(&resolver, &tailwindcss::SPEC, "src/App.tsx");
    assert_eq!(found.program, native(&node));
    assert_eq!(found.args, [native(&script), "--stdio".to_string()]);
    assert_eq!(found.source, CommandSource::Project("0.14.29".into()));
}

/// bun's `~/.bun/bin/<bin>` (and npm's global `bin`) is a link to the package's script. (On
/// Windows a `.exe` on PATH is never taken for a JavaScript server: bun's shims there are found
/// through its global folder, npm's through their `.cmd`.)
#[cfg(unix)]
#[test]
fn a_path_link_is_traced_to_its_package() {
    let repo = FixtureRepo::new();
    tailwind_project(&repo);
    let global = repo.outside_dir("global");
    let script = tailwind_package(&global.join("node_modules").join(PACKAGE));
    let bin = global.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(
        format!("../node_modules/{PACKAGE}/bin/{BIN}"),
        bin.join(BIN),
    )
    .unwrap();
    let tools = repo.outside_dir("tools");
    let bun = runtime(&tools, "bun");
    for backend in [local(&repo), remote(&repo)] {
        let resolver = js_resolver(backend, host(&[&tools, &bin], None), LspSettings::default());
        let found = command(&resolver, &tailwindcss::SPEC, "src/App.tsx");
        assert_eq!(found.program, native(&bun));
        assert_eq!(found.args, [native(&script), "--stdio".to_string()]);
        assert_eq!(found.source, CommandSource::Path("PATH".into()));
    }
    // A wrapper that isn't in the package and isn't a script runs as it is.
    let wrappers = repo.outside_dir("wrappers");
    write(
        &wrappers.join(BIN),
        "#!/bin/sh\nexec node /opt/x.js \"$@\"\n",
    );
    let resolver = js_resolver(
        local(&repo),
        host(&[&wrappers], None),
        LspSettings::default(),
    );
    let found = command(&resolver, &tailwindcss::SPEC, "src/App.tsx");
    assert_eq!(found.program, native(&wrappers.join(BIN)));
    assert_eq!(found.args, ["--stdio".to_string()]);
}

/// npm on Windows installs a `.cmd` shim, which `spawn_stdio` won't run: dv reads the script
/// it calls, or finds the package beside it.
#[cfg(windows)]
#[test]
fn npms_windows_shim_names_the_script() {
    let repo = FixtureRepo::new();
    tailwind_project(&repo);
    let npm = repo.outside_dir("npm");
    let script = tailwind_package(&npm.join("node_modules").join(PACKAGE));
    write(
        &npm.join(format!("{BIN}.cmd")),
        format!(
            "@ECHO off\r\nSETLOCAL\r\nCALL :find_dp0\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \
             \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n)\r\nendLocal & \
             \"%_prog%\"  \"%dp0%\\node_modules\\@tailwindcss\\language-server\\bin\\{BIN}\" %*\r\n"
        ),
    );
    let nodejs = repo.outside_dir("nodejs");
    let node = runtime(&nodejs, "node");
    let resolver = js_resolver(
        local(&repo),
        host(&[&nodejs, &npm], None),
        LspSettings::default(),
    );
    let found = command(&resolver, &tailwindcss::SPEC, "src/App.tsx");
    assert_eq!(found.program, native(&node));
    assert_eq!(found.args, [native(&script), "--stdio".to_string()]);
    // A shim dv can't read still has its package beside it.
    write(&npm.join(format!("{BIN}.cmd")), "@echo off\r\n");
    resolver.forget();
    let found = command(&resolver, &tailwindcss::SPEC, "src/App.tsx");
    assert_eq!(found.args, [native(&script), "--stdio".to_string()]);
}

#[test]
fn lsp_runtime_comes_before_bun_and_node() {
    let repo = FixtureRepo::new();
    tailwind_project(&repo);
    let home = repo.outside_dir("user");
    let script = tailwind_package(&home.join(".bun/install/global/node_modules").join(PACKAGE));
    let tools = repo.outside_dir("tools");
    let bun = runtime(&tools, "bun");
    let node = runtime(&tools, "node");
    let with =
        |settings: LspSettings| js_resolver(local(&repo), host(&[&tools], Some(&home)), settings);
    // bun before node.
    assert_eq!(
        command(
            &with(LspSettings::default()),
            &tailwindcss::SPEC,
            "src/App.tsx"
        )
        .program,
        native(&bun)
    );
    // An argument list, its program absolute.
    let settings = LspSettings::default()
        .with_runtime(vec![native(&node), "--max-old-space-size=4096".into()]);
    let found = command(&with(settings), &tailwindcss::SPEC, "src/App.tsx");
    assert_eq!(found.program, native(&node));
    assert_eq!(
        found.args,
        [
            "--max-old-space-size=4096".to_string(),
            native(&script),
            "--stdio".into()
        ]
    );
    // A bare name is looked up on PATH; one PATH lacks says so.
    let found = command(
        &with(LspSettings::default().with_runtime(vec!["node".into()])),
        &tailwindcss::SPEC,
        "src/App.tsx",
    );
    assert_eq!(found.program, native(&node));
    let missing = why_missing(
        &with(LspSettings::default().with_runtime(vec!["deno".into()])),
        &tailwindcss::SPEC,
        "src/App.tsx",
    );
    assert_eq!(
        missing.reason,
        "lsp.runtime names deno, which isn't on PATH"
    );
    assert_eq!(missing.hint, "Fix lsp.runtime in settings.json");
}

/// A spec whose server breaks under bun.
static NODE_ONLY: LanguageServerSpec = LanguageServerSpec {
    id: "nodeonly",
    name: "Node only",
    documents: &[DocumentKind {
        extension: "css",
        language_id: "css",
    }],
    root_markers: &[],
    applies: None,
    resolvers: &[ResolverKind::BunGlobal],
    command_name: "node-only-ls",
    args: &["--stdio"],
    runtime: Runtime::Js(JsServer {
        package: "node-only-ls",
        node_only: true,
    }),
    initialization_options: || serde_json::Value::Null,
    settings: || serde_json::Value::Null,
    missing: "Node only isn't installed",
    install_hint: "npm install -g node-only-ls",
};

#[test]
fn a_node_only_spec_never_runs_under_bun() {
    let repo = FixtureRepo::new();
    repo.write("a.css", "a {}\n");
    repo.commit("css");
    let home = repo.outside_dir("user");
    let global = home.join(".bun/install/global/node_modules/node-only-ls");
    write(
        &global.join("package.json"),
        r#"{"name": "node-only-ls", "version": "1.0.0", "bin": "./cli.js"}"#,
    );
    write(&global.join("cli.js"), "#!/usr/bin/env node\n");
    let tools = repo.outside_dir("tools");
    runtime(&tools, "bun");
    runtime(&home.join(".bun/bin"), "bun");
    let resolver =
        |settings: LspSettings| js_resolver(local(&repo), host(&[&tools], Some(&home)), settings);
    // bun on PATH, in its default install and in lsp.runtime: none of them will do.
    let settings = LspSettings::default().with_runtime(vec![native(&tools.join(exe("bun")))]);
    let missing = why_missing(&resolver(settings), &NODE_ONLY, "a.css");
    assert_eq!(
        missing.reason,
        "Node only runs with node, which isn't on PATH"
    );
    let node = runtime(&tools, "node");
    let found = command(&resolver(LspSettings::default()), &NODE_ONLY, "a.css");
    assert_eq!(found.program, native(&node));
    assert_eq!(
        found.args,
        [native(&global.join("cli.js")), "--stdio".to_string()]
    );
}

#[test]
fn a_missing_server_says_how_to_get_it_on_this_machine() {
    let repo = FixtureRepo::new();
    tailwind_project(&repo);
    let home = repo.outside_dir("user");
    let tools = repo.outside_dir("tools");
    let resolver = || {
        js_resolver(
            local(&repo),
            host(&[&tools], Some(&home)),
            LspSettings::default(),
        )
    };
    // Neither bun nor node.
    let missing = why_missing(&resolver(), &tailwindcss::SPEC, "src/App.tsx");
    let global = native(&home.join(".bun/install/global/node_modules"));
    assert!(
        missing.reason.starts_with(
            "Tailwind CSS's language server isn't installed (no node_modules/@tailwindcss/\
             language-server in "
        ),
        "{}",
        missing.reason
    );
    assert!(
        missing
            .reason
            .ends_with(&format!("; not on PATH; not in {global})")),
        "{}",
        missing.reason
    );
    assert_eq!(
        missing.hint,
        "Install bun (bun.sh) or Node.js, then bun add -g @tailwindcss/language-server (or npm \
         install -g @tailwindcss/language-server), or set lsp.tailwindcss.command in settings.json"
    );
    // Node only: npm's way.
    let node = runtime(&tools, "node");
    assert_eq!(
        why_missing(&resolver(), &tailwindcss::SPEC, "src/App.tsx").hint,
        "Install it with npm install -g @tailwindcss/language-server, or set \
         lsp.tailwindcss.command in settings.json"
    );
    // bun too: bun's way first.
    runtime(&home.join(".bun/bin"), "bun");
    assert_eq!(
        why_missing(&resolver(), &tailwindcss::SPEC, "src/App.tsx").hint,
        "Install it with bun add -g @tailwindcss/language-server (or npm install -g \
         @tailwindcss/language-server), or set lsp.tailwindcss.command in settings.json"
    );
    // The server without a runtime is missing too, saying so.
    std::fs::remove_file(&node).unwrap();
    std::fs::remove_file(home.join(".bun/bin").join(exe("bun"))).unwrap();
    tailwind_package(&home.join(".bun/install/global/node_modules").join(PACKAGE));
    let missing = why_missing(&resolver(), &tailwindcss::SPEC, "src/App.tsx");
    assert_eq!(
        missing.reason,
        format!(
            "Tailwind CSS runs with bun or node, and neither is on PATH or in {}",
            native(&home.join(".bun/bin"))
        )
    );
}

#[test]
fn tailwind_applies_where_a_stylesheet_or_config_brings_it_in() {
    let repo = FixtureRepo::new();
    repo.write("package.json", r#"{"name": "web"}"#);
    repo.write("src/App.tsx", "export {};\n");
    repo.write("src/plain.css", "body { margin: 0 }\n");
    repo.commit("no tailwind");
    let resolver = js_resolver(
        local(&repo),
        HostEnv::unknown(style()),
        LspSettings::default(),
    );
    assert_eq!(
        resolver.resolve(&tailwindcss::SPEC, "src/App.tsx").unwrap(),
        None
    );
    // CSS applies to every stylesheet, whatever the project.
    assert!(matches!(
        resolver.resolve(&css::SPEC, "src/plain.css").unwrap(),
        Some(Resolution::Missing(_))
    ));
    assert_eq!(resolver.resolve(&css::SPEC, "src/App.tsx").unwrap(), None);
    // v3: a config, untracked; the resolver hears of it.
    repo.write("tailwind.config.js", "module.exports = {};\n");
    assert!(resolver.files_changed(["tailwind.config.js"]));
    assert!(
        resolver
            .resolve(&tailwindcss::SPEC, "src/App.tsx")
            .unwrap()
            .is_some()
    );
    // Without it again (a rescan forgets everything): no Tailwind, and a stylesheet changing
    // is then something the resolver listens for.
    repo.remove("tailwind.config.js");
    resolver.forget();
    assert_eq!(
        resolver.resolve(&tailwindcss::SPEC, "src/App.tsx").unwrap(),
        None
    );
    assert!(!resolver.files_changed(["src/App.tsx"]));
    // v4: a stylesheet, untracked, importing it.
    repo.write("src/styles/new.scss", "@import 'tailwindcss';\n");
    assert!(resolver.files_changed(["src/styles/new.scss"]));
    assert!(
        resolver
            .resolve(&tailwindcss::SPEC, "src/App.tsx")
            .unwrap()
            .is_some()
    );
    // Once it applies, a stylesheet changing forgets nothing.
    assert!(!resolver.files_changed(["src/plain.css"]));
    repo.remove("src/styles/new.scss");
    repo.write("src/plain.css", "@import 'tailwindcss';\n");
    let remote = js_resolver(
        remote(&repo),
        HostEnv::unknown(style()),
        LspSettings::default(),
    );
    assert!(
        remote
            .resolve(&tailwindcss::SPEC, "src/App.tsx")
            .unwrap()
            .is_some()
    );
}

/// A native server found on `PATH`, as it is.
static NATIVE_ON_PATH: LanguageServerSpec = LanguageServerSpec {
    id: "nativels",
    name: "Native",
    documents: &[DocumentKind {
        extension: "nat",
        language_id: "nat",
    }],
    root_markers: &[],
    applies: None,
    resolvers: &[ResolverKind::Settings, ResolverKind::Path],
    command_name: "native-ls",
    args: &["--stdio"],
    runtime: Runtime::Native,
    initialization_options: || serde_json::Value::Null,
    settings: || serde_json::Value::Null,
    missing: "Native isn't installed",
    install_hint: "install native-ls",
};

#[test]
fn a_native_server_on_path_runs_as_it_is() {
    let repo = FixtureRepo::new();
    repo.write("a.nat", "x\n");
    repo.commit("nat");
    let first = repo.outside_dir("first");
    let second = repo.outside_dir("second");
    let server = runtime(&second, "native-ls");
    let resolver = js_resolver(
        local(&repo),
        host(&[&first, &second], None),
        LspSettings::default(),
    );
    assert_eq!(
        command(&resolver, &NATIVE_ON_PATH, "a.nat"),
        ServerCommand {
            spec_id: "nativels",
            program: native(&server),
            args: vec!["--stdio".into()],
            root: String::new(),
            source: CommandSource::Path("PATH".into()),
            env: path_env(&[&first, &second]),
        }
    );
    let empty = js_resolver(local(&repo), host(&[&first], None), LspSettings::default());
    assert_eq!(
        why_missing(&empty, &NATIVE_ON_PATH, "a.nat").reason,
        "Native isn't installed (not on PATH)"
    );
}

/// pnpm's global `bin` holds POSIX shims, not links: `exec node "$basedir/…"`, traced to the
/// package so the server runs under the runtime dv picks.
#[cfg(unix)]
#[test]
fn a_pnpm_posix_shim_is_traced_to_its_package() {
    let repo = FixtureRepo::new();
    tailwind_project(&repo);
    let pnpm = repo.outside_dir("pnpm");
    let script = tailwind_package(&pnpm.join("global/5/node_modules").join(PACKAGE));
    write(
        &pnpm.join(BIN),
        format!(
            "#!/bin/sh\nbasedir=$(dirname \"$(echo \"$0\" | sed -e 's,\\,/,g')\")\n\
             if [ -x \"$basedir/node\" ]; then\n  exec \"$basedir/node\" \
             \"$basedir/global/5/node_modules/{PACKAGE}/bin/{BIN}\" \"$@\"\nelse\n  exec node \
             \"$basedir/global/5/node_modules/{PACKAGE}/bin/{BIN}\" \"$@\"\nfi\n"
        ),
    );
    let tools = repo.outside_dir("tools");
    let bun = runtime(&tools, "bun");
    let resolver = js_resolver(
        local(&repo),
        host(&[&tools, &pnpm], None),
        LspSettings::default(),
    );
    let found = command(&resolver, &tailwindcss::SPEC, "src/App.tsx");
    assert_eq!(found.program, native(&bun));
    assert_eq!(found.args, [native(&script), "--stdio".to_string()]);
}

/// asdf's `node` is a shim, `exec asdf exec "node" "$@"`, and `asdf` is on the shell's PATH
/// only: the server runs with that PATH, so the shim finds it.
#[cfg(unix)]
#[test]
fn a_shimmed_runtime_runs_with_the_shells_path() {
    use std::os::unix::fs::PermissionsExt as _;
    let repo = FixtureRepo::new();
    tailwind_project(&repo);
    let home = repo.outside_dir("user");
    let script = tailwind_package(&home.join(".bun/install/global/node_modules").join(PACKAGE));
    let executable = |path: &Path, text: &str| {
        write(path, text);
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    let shims = repo.outside_dir("shims");
    let asdf_bin = repo.outside_dir("asdf-bin");
    let real = repo.outside_dir("real");
    executable(
        &shims.join("node"),
        "#!/bin/sh\nexec asdf exec \"node\" \"$@\"\n",
    );
    executable(
        &asdf_bin.join("asdf"),
        &format!(
            "#!/bin/sh\nshift\nshift\nexec {} \"$@\"\n",
            real.join("node").display()
        ),
    );
    executable(&real.join("node"), "#!/bin/sh\necho \"node ran $1 $2\"\n");
    let path = format!("{}:{}:/usr/bin:/bin", shims.display(), asdf_bin.display());
    let host = HostEnv::new(
        PathStyle::Posix,
        Some(&path),
        Some(&home.display().to_string()),
        None,
        PathFrom::Shell {
            shell: "/bin/zsh".into(),
            flags: "-l -i".into(),
        },
    );
    let backend = local(&repo);
    let resolver = js_resolver(backend.clone(), host, LspSettings::default());
    let found = command(&resolver, &tailwindcss::SPEC, "src/App.tsx");
    assert_eq!(found.program, native(&shims.join("node")));
    assert_eq!(found.env, [("PATH".to_string(), path)]);
    let run = |spec: dv_core::backend::ProcessSpec| {
        let (sender, events) = std::sync::mpsc::channel();
        let _handle = backend
            .spawn_stdio(&spec, Box::new(move |event| drop(sender.send(event))))
            .unwrap();
        let mut stdout = Vec::new();
        loop {
            match events
                .recv_timeout(std::time::Duration::from_secs(20))
                .unwrap()
            {
                dv_core::backend::ProcessEvent::Stdout(chunk) => stdout.extend(chunk),
                dv_core::backend::ProcessEvent::Stderr(_) => {}
                dv_core::backend::ProcessEvent::Exit(code) => {
                    return (code, String::from_utf8_lossy(&stdout).into_owned());
                }
            }
        }
    };
    assert_eq!(
        run(found.process_spec()),
        (Some(0), format!("node ran {} --stdio\n", native(&script)))
    );
    // With a PATH that lacks it (the backend's own), the shim can't find asdf.
    let mut bare = found.process_spec();
    bare.env = vec![("PATH".to_string(), Some("/usr/bin:/bin".to_string()))];
    assert_eq!(run(bare).0, Some(127));
}
