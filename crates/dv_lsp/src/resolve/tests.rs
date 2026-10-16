//! The resolvers on fixture trees with the npm, pnpm and bun layouts (docs/PLAN.md §18.7),
//! through `LocalBackend` and through `RemoteBackend` over an in-process server, so a WSL
//! repository resolves the same way. The layouts' links are real symbolic links; where this OS
//! won't make them (Windows without developer mode) a test says so and passes.

use std::path::Path;
use std::sync::Arc;

use dv_backend::{Connection, Launcher};
use dv_core::backend::{Backend, LocalBackend, LocalBackendOptions};
use dv_test_support::FixtureRepo;
use pretty_assertions::assert_eq;

use super::*;
use crate::languages::typescript;

const LINUX: (&str, &str) = ("linux", "x86_64");
const NATIVE: &str = "@typescript/typescript-linux-x64";

/// An absolute path as this machine spells it (settings' programs are this machine's).
const OPT_TSC: &str = if cfg!(windows) {
    r"C:\opt\ts\tsc.exe"
} else {
    "/opt/ts/tsc"
};

fn local(repo: &FixtureRepo) -> Arc<dyn Backend> {
    Arc::new(
        LocalBackend::open_with(
            repo.path(),
            LocalBackendOptions {
                env: repo.env_overlay(),
            },
        )
        .unwrap(),
    )
}

fn remote(repo: &FixtureRepo) -> Arc<dyn Backend> {
    let connection = Connection::new("in-process", Launcher::InProcess);
    Arc::new(
        connection
            .open(repo.path().to_str().unwrap(), repo.env_overlay())
            .unwrap(),
    )
}

/// A resolver on Linux that knows no `PATH` (the JavaScript cases give theirs).
fn resolver(backend: Arc<dyn Backend>, settings: LspSettings) -> Resolver {
    let style = WorkspacePaths::new(&backend.repo().toplevel).style();
    Resolver::new(backend, Platform::new(LINUX.0, LINUX.1), settings)
        .with_env(|_| None)
        .with_host(HostEnv::unknown(style))
}

/// A symbolic link to a directory at `at` (worktree-relative) pointing at `target` as written;
/// `false` when this OS refuses to make one.
fn link(repo: &FixtureRepo, at: &str, target: &str) -> bool {
    let path = repo.path().join(at);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(target, &path);
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(target.replace('/', "\\"), &path);
    match made {
        Ok(()) => true,
        Err(err) => {
            eprintln!(
                "skipping: this OS won't make a symbolic link ({err}); on Windows it needs \
                 developer mode"
            );
            false
        }
    }
}

/// A `package.json` naming `version`.
fn package(repo: &FixtureRepo, dir: &str, name: &str, version: &str) {
    repo.write(
        Path::new(dir).join("package.json"),
        format!(r#"{{"name": "{name}", "version": "{version}", "bin": {{"tsc": "./bin/tsc"}}}}"#),
    );
}

/// A native server package with its executable at `dir`.
fn native(repo: &FixtureRepo, dir: &str) {
    package(repo, dir, NATIVE, "7.0.2");
    repo.write(Path::new(dir).join("lib/tsc"), b"\x7fELF");
}

fn project(repo: &FixtureRepo) {
    repo.write(
        "package.json",
        r#"{"name": "app", "workspaces": ["packages/*"]}"#,
    );
    repo.write("tsconfig.json", "{}");
    repo.write("src/a.ts", "export const a = 1;\n");
    repo.commit("project");
}

fn found(resolver: &Resolver, rel: &str) -> ServerCommand {
    match resolver.resolve(&typescript::SPEC, rel).unwrap() {
        Some(Resolution::Found(command)) => command,
        other => panic!("expected a server for {rel}, got {other:?}"),
    }
}

fn missing(resolver: &Resolver, rel: &str) -> Missing {
    match resolver.resolve(&typescript::SPEC, rel).unwrap() {
        Some(Resolution::Missing(missing)) => missing,
        other => panic!("expected nothing for {rel}, got {other:?}"),
    }
}

fn expect_project(resolver: &Resolver, rel: &str, executable: &str) {
    let command = found(resolver, rel);
    assert_eq!(
        command,
        ServerCommand {
            spec_id: "typescript",
            program: resolver.paths().absolute(executable),
            args: vec!["--lsp".into(), "--stdio".into()],
            root: String::new(),
            source: CommandSource::Project("7.0.2".into()),
            env: Vec::new(),
        }
    );
}

#[test]
fn npm_hoists_both_packages() {
    let repo = FixtureRepo::new();
    project(&repo);
    package(&repo, "node_modules/typescript", "typescript", "7.0.2");
    native(&repo, &format!("node_modules/{NATIVE}"));
    for backend in [local(&repo), remote(&repo)] {
        let resolver = resolver(backend, LspSettings::default());
        expect_project(
            &resolver,
            "src/a.ts",
            &format!("node_modules/{NATIVE}/lib/tsc"),
        );
    }
}

#[test]
fn windows_runs_tsc_exe_from_the_win32_package() {
    let repo = FixtureRepo::new();
    project(&repo);
    package(&repo, "node_modules/typescript", "typescript", "7.1.0");
    let win = "node_modules/@typescript/typescript-win32-arm64";
    package(&repo, win, "@typescript/typescript-win32-arm64", "7.1.0");
    repo.write(format!("{win}/lib/tsc.exe"), b"MZ");
    let resolver = Resolver::new(
        local(&repo),
        Platform::new("windows", "aarch64"),
        LspSettings::default(),
    )
    .with_env(|_| None)
    .with_host(HostEnv::unknown(crate::paths::PathStyle::Posix));
    let command = found(&resolver, "src/a.ts");
    assert_eq!(
        command.program,
        resolver.paths().absolute(&format!("{win}/lib/tsc.exe"))
    );
    assert_eq!(command.source, CommandSource::Project("7.1.0".into()));
}

#[test]
fn pnpm_links_each_package_beside_its_dependencies() {
    let repo = FixtureRepo::new();
    project(&repo);
    let store = "node_modules/.pnpm";
    package(
        &repo,
        &format!("{store}/typescript@7.0.2/node_modules/typescript"),
        "typescript",
        "7.0.2",
    );
    let real_native =
        format!("{store}/@typescript+typescript-linux-x64@7.0.2/node_modules/{NATIVE}");
    native(&repo, &real_native);
    if !link(
        &repo,
        "node_modules/typescript",
        ".pnpm/typescript@7.0.2/node_modules/typescript",
    ) || !link(
        &repo,
        &format!("{store}/typescript@7.0.2/node_modules/{NATIVE}"),
        &format!("../../../@typescript+typescript-linux-x64@7.0.2/node_modules/{NATIVE}"),
    ) {
        return;
    }
    for backend in [local(&repo), remote(&repo)] {
        let resolver = resolver(backend, LspSettings::default());
        expect_project(&resolver, "src/a.ts", &format!("{real_native}/lib/tsc"));
    }
}

/// The owner's layout (checked 2026-10-17): bun's `.bun` folder, the root's and every
/// workspace package's `node_modules/typescript` a link into it, and a package on an older
/// TypeScript.
#[test]
fn bun_links_workspace_packages_to_one_copy() {
    let repo = FixtureRepo::new();
    project(&repo);
    repo.write("packages/web/package.json", r#"{"name": "web"}"#);
    repo.write("packages/web/src/app.tsx", "export {};\n");
    repo.write("packages/old/package.json", r#"{"name": "old"}"#);
    repo.write("packages/old/x.ts", "export {};\n");
    let store = "node_modules/.bun";
    for version in ["7.0.2", "6.0.3"] {
        package(
            &repo,
            &format!("{store}/typescript@{version}/node_modules/typescript"),
            "typescript",
            version,
        );
    }
    let real_native =
        format!("{store}/@typescript+typescript-linux-x64@7.0.2/node_modules/{NATIVE}");
    native(&repo, &real_native);
    let links = [
        (
            "node_modules/typescript".to_string(),
            ".bun/typescript@7.0.2/node_modules/typescript".to_string(),
        ),
        (
            format!("{store}/typescript@7.0.2/node_modules/{NATIVE}"),
            format!("../../../@typescript+typescript-linux-x64@7.0.2/node_modules/{NATIVE}"),
        ),
        (
            "packages/web/node_modules/typescript".into(),
            "../../../node_modules/.bun/typescript@7.0.2/node_modules/typescript".into(),
        ),
        (
            "packages/old/node_modules/typescript".into(),
            "../../../node_modules/.bun/typescript@6.0.3/node_modules/typescript".into(),
        ),
    ];
    for (at, target) in &links {
        if !link(&repo, at, target) {
            return;
        }
    }
    for backend in [local(&repo), remote(&repo)] {
        let resolver = resolver(backend, LspSettings::default());
        let tsc = format!("{real_native}/lib/tsc");
        expect_project(&resolver, "src/a.ts", &tsc);
        // The package's own link reaches the same copy: the same server.
        expect_project(&resolver, "packages/web/src/app.tsx", &tsc);
        assert_eq!(
            missing(&resolver, "packages/old/x.ts").reason,
            "TypeScript 6.0.3 needs typescript-language-server, which dv doesn't run yet"
        );
    }
}

/// `path` as this OS spells it, every separator its own.
fn spelled(path: &Path) -> String {
    path.to_str()
        .unwrap()
        .replace('/', std::path::MAIN_SEPARATOR_STR)
}

/// pnpm's global store: `node_modules/typescript` links to a folder outside the worktree, its
/// native package's link beside it there.
#[test]
fn a_link_out_of_the_worktree_is_followed() {
    let repo = FixtureRepo::new();
    project(&repo);
    let store = repo.outside_dir("store");
    let ts = store.join("typescript@7.0.2/node_modules/typescript");
    std::fs::create_dir_all(&ts).unwrap();
    std::fs::write(ts.join("package.json"), r#"{"version": "7.0.2"}"#).unwrap();
    let real_native = store.join(format!("native@7.0.2/node_modules/{NATIVE}"));
    std::fs::create_dir_all(real_native.join("lib")).unwrap();
    std::fs::write(real_native.join("package.json"), r#"{"version": "7.0.2"}"#).unwrap();
    std::fs::write(real_native.join("lib/tsc"), b"\x7fELF").unwrap();
    let beside = store.join(format!("typescript@7.0.2/node_modules/{NATIVE}"));
    std::fs::create_dir_all(beside.parent().unwrap()).unwrap();
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&real_native, &beside);
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(&real_native, &beside);
    if made.is_err() || !link(&repo, "node_modules/typescript", ts.to_str().unwrap()) {
        eprintln!("skipping: this OS won't make a symbolic link");
        return;
    }
    for backend in [local(&repo), remote(&repo)] {
        let resolver = resolver(backend, LspSettings::default());
        let command = found(&resolver, "src/a.ts");
        assert_eq!(command.program, spelled(&real_native.join("lib/tsc")));
    }
}

/// A linked worktree inside the main checkout has no `node_modules` of its own: Node finds
/// the checkout's, above the worktree root.
#[test]
fn packages_resolve_above_the_worktree_root() {
    let repo = FixtureRepo::new();
    project(&repo);
    // `<base>/node_modules`, beside the worktree `<base>/repo`: its parent's.
    let above = repo.outside_dir("node_modules");
    std::fs::create_dir_all(above.join("typescript")).unwrap();
    std::fs::write(
        above.join("typescript/package.json"),
        r#"{"version": "7.0.2"}"#,
    )
    .unwrap();
    let native_dir = above.join(NATIVE);
    std::fs::create_dir_all(native_dir.join("lib")).unwrap();
    std::fs::write(native_dir.join("package.json"), r#"{"version": "7.0.2"}"#).unwrap();
    std::fs::write(native_dir.join("lib/tsc"), b"\x7fELF").unwrap();
    for backend in [local(&repo), remote(&repo)] {
        let resolver = resolver(backend, LspSettings::default());
        assert_eq!(
            found(&resolver, "src/a.ts").program,
            spelled(&native_dir.join("lib/tsc"))
        );
    }
}

/// A backend that refuses to read one place, as a store dv may not read would.
struct Denies {
    inner: Arc<dyn Backend>,
    denied: String,
}

impl Backend for Denies {
    fn repo(&self) -> &dv_core::backend::RepoInfo {
        self.inner.repo()
    }
    fn git(
        &self,
        args: &[&str],
        opts: &dv_core::backend::GitOpts,
    ) -> Result<dv_core::backend::GitOutput, BackendError> {
        self.inner.git(args, opts)
    }
    fn worktree_read(&self, rel: &str) -> Result<Option<Vec<u8>>, BackendError> {
        self.inner.worktree_read(rel)
    }
    fn worktree_len(&self, rel: &str) -> Result<Option<u64>, BackendError> {
        self.inner.worktree_len(rel)
    }
    fn probe_path(&self, path: &str) -> Result<dv_core::backend::PathProbe, BackendError> {
        if path.starts_with(&self.denied) {
            return Err(BackendError::Io {
                op: "probing".into(),
                path: path.to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "permission denied",
                ),
            });
        }
        self.inner.probe_path(path)
    }
    fn store_append(&self, rel: &str, line: &[u8]) -> Result<(), BackendError> {
        self.inner.store_append(rel, line)
    }
    fn store_read(&self, rel: &str) -> Result<Option<Vec<u8>>, BackendError> {
        self.inner.store_read(rel)
    }
    fn store_list(&self, dir: &str) -> Result<Vec<String>, BackendError> {
        self.inner.store_list(dir)
    }
    fn store_write_atomic(&self, rel: &str, bytes: &[u8]) -> Result<(), BackendError> {
        self.inner.store_write_atomic(rel, bytes)
    }
    fn store_remove(&self, rel: &str) -> Result<bool, BackendError> {
        self.inner.store_remove(rel)
    }
    fn copy_index_to_store(&self, rel: &str) -> Result<bool, BackendError> {
        self.inner.copy_index_to_store(rel)
    }
    fn watch(
        &self,
        spec: dv_core::backend::WatchSpec,
        sink: dv_core::backend::WatchSink,
    ) -> Result<dv_core::backend::WatchHandle, BackendError> {
        self.inner.watch(spec, sink)
    }
    fn spawn_stdio(
        &self,
        spec: &dv_core::backend::ProcessSpec,
        sink: dv_core::backend::ProcessSink,
    ) -> Result<dv_core::backend::ProcessHandle, BackendError> {
        self.inner.spawn_stdio(spec, sink)
    }
}

#[test]
fn a_link_dv_cant_follow_says_so() {
    let repo = FixtureRepo::new();
    project(&repo);
    let store = repo.outside_dir("store");
    std::fs::create_dir_all(store.join("typescript")).unwrap();
    if !link(
        &repo,
        "node_modules/typescript",
        store.join("typescript").to_str().unwrap(),
    ) {
        return;
    }
    let denied: Arc<dyn Backend> = Arc::new(Denies {
        inner: local(&repo),
        denied: store.to_str().unwrap().to_string(),
    });
    let resolver = resolver(denied, LspSettings::default());
    let reason = missing(&resolver, "src/a.ts").reason;
    let found_at = resolver.paths().absolute("node_modules/typescript");
    assert!(
        reason.starts_with(&format!("{found_at} leads to {}", store.display()))
            && reason.contains("which dv can't read")
            && reason.contains("permission denied"),
        "{reason}"
    );
}

#[test]
fn a_typescript_without_its_native_package() {
    let repo = FixtureRepo::new();
    project(&repo);
    package(&repo, "node_modules/typescript", "typescript", "7.0.2");
    let resolver = resolver(local(&repo), LspSettings::default());
    assert_eq!(
        missing(&resolver, "src/a.ts"),
        Missing {
            spec_id: "typescript",
            reason: format!(
                "TypeScript 7.0.2 is installed without {NATIVE}, its server for linux x64"
            ),
            hint: typescript::SPEC.install_hint.into(),
        }
    );
}

#[test]
fn a_project_without_typescript_falls_back_to_settings() {
    let repo = FixtureRepo::new();
    project(&repo);
    let backend = local(&repo);
    let none = resolver(backend.clone(), LspSettings::default());
    // It says where it looked: the file's directory and every one above it.
    let reason = missing(&none, "src/a.ts").reason;
    let src = none.paths().absolute("src");
    assert!(
        reason.starts_with(&format!(
            "TypeScript 7 isn't installed in this project (no node_modules/typescript in {src}, "
        )),
        "{reason}"
    );

    let configured = resolver(
        backend.clone(),
        LspSettings::default().with_command(
            "typescript",
            vec![OPT_TSC.into(), "--lsp".into(), "--stdio".into()],
        ),
    );
    assert_eq!(
        found(&configured, "src/a.ts"),
        ServerCommand {
            spec_id: "typescript",
            program: OPT_TSC.into(),
            args: vec!["--lsp".into(), "--stdio".into()],
            root: String::new(),
            source: CommandSource::Settings,
            env: Vec::new(),
        }
    );

    // A bare name is looked up on PATH.
    let settings = LspSettings::default().with_command("typescript", vec!["tsc".into()]);
    let style = none.paths().style();
    let bare = resolver(backend.clone(), settings.clone()).with_host(HostEnv::unknown(style));
    assert_eq!(
        missing(&bare, "src/a.ts").reason,
        "lsp.typescript.command names tsc, which isn't on PATH (unknown)"
    );
    let bin = repo.outside_dir("bin");
    let tsc = bin.join(if cfg!(windows) { "tsc.exe" } else { "tsc" });
    std::fs::write(&tsc, b"MZ").unwrap();
    let on_path = resolver(backend, settings).with_host(HostEnv::new(
        style,
        Some(&spelled(&bin)),
        None,
        None,
        PathFrom::Process,
    ));
    assert_eq!(found(&on_path, "src/a.ts").program, spelled(&tsc));
}

#[test]
fn an_emulated_dv_tries_the_native_cpu_first_then_its_own() {
    let repo = FixtureRepo::new();
    project(&repo);
    package(&repo, "node_modules/typescript", "typescript", "7.0.2");
    // Only the x64 package is installed: an ARM64 machine still runs it, emulated.
    native(&repo, &format!("node_modules/{NATIVE}"));
    let resolver = Resolver::new(
        local(&repo),
        Platform::new("linux", "aarch64").with_fallback_arch("x86_64"),
        LspSettings::default(),
    )
    .with_env(|_| None)
    .with_host(HostEnv::unknown(crate::paths::PathStyle::Posix));
    expect_project(
        &resolver,
        "src/a.ts",
        &format!("node_modules/{NATIVE}/lib/tsc"),
    );
    // Without the fallback, the native CPU's package is what's missing.
    let arm_only = Resolver::new(
        local(&repo),
        Platform::new("linux", "aarch64"),
        LspSettings::default(),
    )
    .with_env(|_| None)
    .with_host(HostEnv::unknown(crate::paths::PathStyle::Posix));
    assert_eq!(
        missing(&arm_only, "src/a.ts").reason,
        "TypeScript 7.0.2 is installed without @typescript/typescript-linux-arm64, its server for \
         linux arm64"
    );
}

#[test]
fn the_override_must_be_absolute() {
    let repo = FixtureRepo::new();
    project(&repo);
    let resolver = resolver(local(&repo), LspSettings::default())
        .with_env(|name| (name == "DV_LSP_TYPESCRIPT").then(|| "mock_lsp".to_string()));
    assert_eq!(
        missing(&resolver, "src/a.ts").reason,
        "DV_LSP_TYPESCRIPT names mock_lsp, which isn't an absolute path"
    );
}

#[test]
fn overrides_only_ignores_the_project_and_settings() {
    let repo = FixtureRepo::new();
    project(&repo);
    package(&repo, "node_modules/typescript", "typescript", "7.0.2");
    native(&repo, &format!("node_modules/{NATIVE}"));
    let settings = LspSettings::default().with_command("typescript", vec!["/opt/tsc".into()]);
    let unnamed = resolver(local(&repo), settings.clone())
        .with_env(|_| None)
        .overrides_only();
    assert_eq!(
        missing(&unnamed, "src/a.ts").reason,
        "DV_LSP_TYPESCRIPT isn't set, and automated runs find language servers only through it"
    );
    let named = resolver(local(&repo), settings)
        .with_env(|name| (name == "DV_LSP_TYPESCRIPT").then(|| "/bin/mock_lsp".to_string()))
        .overrides_only();
    assert!(matches!(
        named.resolve(&typescript::SPEC, "src/a.ts").unwrap(),
        Some(Resolution::Found(ServerCommand { ref program, .. })) if program == "/bin/mock_lsp"
    ));
}

#[test]
fn an_install_is_seen_once_the_resolver_hears_of_it() {
    let repo = FixtureRepo::new();
    project(&repo);
    let resolver = resolver(local(&repo), LspSettings::default());
    assert!(matches!(
        resolver.resolve(&typescript::SPEC, "src/a.ts").unwrap(),
        Some(Resolution::Missing(_))
    ));
    package(&repo, "node_modules/typescript", "typescript", "7.0.2");
    native(&repo, &format!("node_modules/{NATIVE}"));
    // Remembered until told: a source file's change says nothing about packages.
    assert!(!resolver.files_changed(["src/a.ts"]));
    assert!(matches!(
        resolver.resolve(&typescript::SPEC, "src/a.ts").unwrap(),
        Some(Resolution::Missing(_))
    ));
    // Before a restart, afresh.
    assert!(matches!(
        resolver
            .resolve_fresh(&typescript::SPEC, "src/a.ts")
            .unwrap(),
        Some(Resolution::Found(_))
    ));
    // A lockfile's change forgets everything.
    std::fs::remove_dir_all(repo.path().join("node_modules")).unwrap();
    assert!(resolver.files_changed(["src/a.ts", "bun.lock"]));
    assert!(matches!(
        resolver.resolve(&typescript::SPEC, "src/a.ts").unwrap(),
        Some(Resolution::Missing(_))
    ));
    for rel in [
        "package.json",
        "apps/web/package.json",
        "pnpm-lock.yaml",
        "node_modules/x/y.js",
        "tsconfig.json",
    ] {
        assert!(affects_resolution(rel), "{rel}");
    }
    for rel in ["src/a.ts", "package.json.bak", "docs/node_modules.md"] {
        assert!(!affects_resolution(rel), "{rel}");
    }
}

#[test]
fn real_paths_resolve_links_dot_dots_and_loops() {
    let repo = FixtureRepo::new();
    repo.write("a/b/file.txt", "x");
    repo.commit("tree");
    let backend = local(&repo);
    let paths = WorkspacePaths::new(&backend.repo().toplevel);
    let root = node::AbsPath::parse(&backend.repo().toplevel, paths.style()).unwrap();
    let found = |rel: &str| node::RealPath::Found(root.join(rel));
    {
        let reader = node::Reader::new(backend.as_ref(), paths.style());
        let real = |rel: &str| node::real_path(&reader, &root.join(rel)).unwrap();
        assert_eq!(real("a/b/file.txt"), found("a/b/file.txt"));
        assert_eq!(real("a/missing"), node::RealPath::Missing);
        assert_eq!(real("a/b/file.txt/deeper"), node::RealPath::Missing);
    }
    if !link(&repo, "c", "a/b") || !link(&repo, "a/up", "..") || !link(&repo, "loop", "loop") {
        return;
    }
    let reader = node::Reader::new(backend.as_ref(), paths.style());
    let real = |rel: &str| node::real_path(&reader, &root.join(rel)).unwrap();
    assert_eq!(real("c/file.txt"), found("a/b/file.txt"));
    assert_eq!(real("a/up/c"), found("a/b"));
    assert!(matches!(real("loop/x"), node::RealPath::Unreadable { .. }));
}

#[test]
fn the_override_replaces_the_resolvers() {
    let repo = FixtureRepo::new();
    project(&repo);
    package(&repo, "node_modules/typescript", "typescript", "7.0.2");
    native(&repo, &format!("node_modules/{NATIVE}"));
    let resolver = Resolver::new(
        local(&repo),
        Platform::new(LINUX.0, LINUX.1),
        LspSettings::default(),
    )
    .with_env(|name| match name {
        "DV_LSP_TYPESCRIPT" => Some("/bin/mock_lsp".into()),
        "DV_LSP_TYPESCRIPT_ARGS" => Some(r#"["--scenario", "/tmp/s.json"]"#.into()),
        _ => None,
    });
    assert_eq!(
        found(&resolver, "src/a.ts"),
        ServerCommand {
            spec_id: "typescript",
            program: "/bin/mock_lsp".into(),
            args: vec!["--scenario".into(), "/tmp/s.json".into()],
            root: String::new(),
            source: CommandSource::Override("DV_LSP_TYPESCRIPT".into()),
            env: Vec::new(),
        }
    );
}

#[test]
fn a_file_the_spec_doesnt_serve_or_outside_a_project() {
    let repo = FixtureRepo::new();
    repo.write("notes/a.ts", "x\n");
    repo.write("README.md", "x\n");
    repo.commit("no project");
    let resolver = resolver(local(&repo), LspSettings::default());
    assert_eq!(
        resolver.resolve(&typescript::SPEC, "README.md").unwrap(),
        None
    );
    // No package.json or tsconfig.json above it.
    assert_eq!(
        resolver.resolve(&typescript::SPEC, "notes/a.ts").unwrap(),
        None
    );
    // A marker in a subdirectory applies below it.
    repo.write("notes/tsconfig.json", "{}");
    resolver.forget();
    assert!(matches!(
        resolver.resolve(&typescript::SPEC, "notes/a.ts").unwrap(),
        Some(Resolution::Missing(_))
    ));
}

#[test]
fn ancestors_end_at_the_root() {
    assert_eq!(ancestors("a/b").collect::<Vec<_>>(), ["a/b", "a", ""]);
    assert_eq!(ancestors("").collect::<Vec<_>>(), [""]);
    assert!(is_absolute("/x") && is_absolute(r"C:\x") && is_absolute("c:/x"));
    assert!(is_absolute(r"\\server\share"));
    assert!(!is_absolute("tsc") && !is_absolute("./tsc") && !is_absolute("C:x"));
    assert_eq!(node::major_version("7.0.2"), Some(7));
    assert_eq!(node::major_version("10.1.0-dev.1"), Some(10));
    assert_eq!(node::major_version("v6"), Some(6));
    assert_eq!(node::major_version("next"), None);
}
mod js;
