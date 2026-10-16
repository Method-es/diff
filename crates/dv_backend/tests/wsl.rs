//! The real WSL (docs/PLAN.md §5.2, §14.2): `wsl.exe` and a distro, from Windows. Runs only
//! when `DV_TEST_WSL_DISTRO` names a distro, sandboxed by `DV_WSL_HOME` (a fresh folder under
//! `/tmp` in it, which `script/check --linux` makes and removes), one test at a time:
//!
//! ```text
//! DV_TEST_WSL_DISTRO=Ubuntu DV_WSL_HOME=/tmp/dv-check-1 \
//!     cargo test -p dv_backend --test wsl -- --test-threads=1
//! ```
//!
//! Every test works in a home of its own under `DV_WSL_HOME`, removed at its end, and runs
//! every script there with that home as `$HOME`, so nothing reaches the distro user's own home
//! (CLAUDE.md). It needs this app's Linux build (`script/build-linux`): `DV_LINUX_DIR`, else
//! `target/debug`. The owner's check against the real home is `tests/live_wsl.rs`.

#![cfg(windows)]

mod wsl_common;

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use dv_backend::install::{self, BuildState, LinkState};
use dv_backend::wsl::{self, Arch, LAUNCH, Shell, WslError};
use dv_backend::{Connection, ConnectionState, Launcher};
use dv_core::backend::{
    Backend, BackendError, GitOpts, ProcessEvent, ProcessHandle, ProcessSpec, WatchSpec,
};
use dv_core::location::Location;
use pretty_assertions::assert_eq;

use self::wsl_common::*;

/// The distro and the scratch root the tests run in, when they run.
struct Sandbox {
    distro: String,
    root: String,
}

impl Sandbox {
    /// `None` unless `DV_TEST_WSL_DISTRO` is set; then `DV_WSL_HOME` must be a folder under
    /// `/tmp`, or the test fails rather than touch a real home.
    fn get() -> Option<Self> {
        let distro = std::env::var("DV_TEST_WSL_DISTRO")
            .ok()
            .filter(|distro| !distro.trim().is_empty())?;
        let root = std::env::var(install::WSL_HOME_VAR).unwrap_or_default();
        let root = wsl::sandbox(Some(root.clone()))
            .ok()
            .flatten()
            .filter(|root| root.starts_with("/tmp/"))
            .unwrap_or_else(|| {
                panic!(
                    "DV_TEST_WSL_DISTRO is set, so DV_WSL_HOME must be a scratch folder under \
                     /tmp in {distro}, not {root:?}"
                )
            });
        Some(Self {
            distro: distro.trim().to_string(),
            root,
        })
    }

    /// The shell of the sandbox root: what makes and removes the tests' own homes.
    fn root_shell(&self) -> Shell {
        Shell::wsl(self.distro.clone(), Some(self.root.clone())).unwrap()
    }

    /// A home of the test's own, removed when dropped, and the shell whose `$HOME` it is.
    fn home(&self, name: &str) -> (TmpDir, Shell) {
        let dir = TmpDir::new(&self.root_shell(), &self.root, name);
        let shell = Shell::wsl(self.distro.clone(), Some(dir.path.clone())).unwrap();
        (dir, shell)
    }

    /// The distro's server with `home` as its home.
    fn launcher(&self, home: &TmpDir) -> Launcher {
        Launcher::Wsl {
            distro: self.distro.clone(),
            builds: builds_dir(),
            home: Some(home.path.clone()),
        }
    }
}

macro_rules! sandbox {
    () => {
        match Sandbox::get() {
            Some(sandbox) => sandbox,
            None => {
                eprintln!("skipped: DV_TEST_WSL_DISTRO isn't set");
                return;
            }
        }
    };
}

#[test]
fn the_distro_lists_as_running_and_other_names_dont() {
    let sandbox = sandbox!();
    // Running for sure: this starts it if it wasn't.
    sh(&sandbox.root_shell(), "true", &[]);
    let found = wsl::find_distro(&sandbox.distro.to_uppercase()).unwrap();
    assert_eq!(found.name.to_lowercase(), sandbox.distro.to_lowercase());
    assert!(found.running, "{found:?}");
    // What an automatic start asks, listed afresh, and what a listing asks, from the cache.
    assert!(wsl::running_now(&sandbox.distro.to_lowercase()));
    assert!(!wsl::running_now("dv-no-such-distro"));
    assert!(wsl::running(&sandbox.distro.to_uppercase()));
    assert!(!wsl::running("dv-no-such-distro"));
    let Err(WslError::NoDistro { name, known }) = wsl::find_distro("dv-no-such-distro") else {
        panic!("found a distro that isn't there");
    };
    assert_eq!(name, "dv-no-such-distro");
    assert!(known.contains(&found.name), "{known:?}");
}

#[test]
fn scripts_and_their_arguments_arrive_intact() {
    let sandbox = sandbox!();
    let (home, shell) = sandbox.home("quoting");
    let args = [
        "a b",
        "$HOME",
        "\"double\"",
        "'single'",
        r"back\slash\",
        "two\nlines",
        "tab\there",
        "",
        "-",
        "ünïcode",
    ];
    let printed = sh(
        &shell,
        r#"printf '%s\n' "$HOME"; for a; do printf '[%s]\n' "$a"; done"#,
        &args,
    );
    let mut expected = format!("{}\n", home.path);
    for arg in args {
        expected.push_str(&format!("[{arg}]\n"));
    }
    assert_eq!(printed, expected);
    // A script crosses as one argument too, newlines, quotes, `$` and all.
    assert_eq!(sh(&shell, r#"printf '%s' "$1""#, &[LAUNCH]), LAUNCH);
}

#[test]
fn an_install_streams_the_build_and_checks_it() {
    let sandbox = sandbox!();
    let (home, shell) = sandbox.home("install");
    let builds = builds();
    let build = builds.get(Arch::X86_64).unwrap();
    let probe = install::probe(&shell, &builds).unwrap();
    assert_eq!(
        (probe.arch, probe.build, probe.link),
        (Arch::X86_64, BuildState::Missing, LinkState::None)
    );
    assert_eq!(probe.home, home.path);

    let heard = std::sync::Mutex::new(Vec::new());
    let installed = install::install(&shell, build, &|done, total| {
        heard.lock().unwrap().push((done, total));
    })
    .unwrap();
    let path = home.join(&format!(".local/share/dv/bin/dv-{}", build.hash));
    assert_eq!(installed.path, path);
    assert!(installed.linked);
    assert_eq!(installed.warnings, Vec::<String>::new());
    let heard = heard.into_inner().unwrap();
    assert_eq!(heard.first(), Some(&(0, build.len)));
    assert_eq!(heard.last(), Some(&(build.len, build.len)));
    assert!(heard.len() > 2, "progress as it goes: {heard:?}");
    let probe = install::probe(&shell, &builds).unwrap();
    assert!(probe.up_to_date(), "{probe:?}");
    assert_eq!(probe.path, path);
    // `~/.local/bin/dv` runs it.
    assert_eq!(
        sh(&shell, r#""$HOME/.local/bin/dv" --version"#, &[]),
        format!("dv {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        sh(&shell, r#"readlink "$HOME/.local/bin/dv""#, &[]),
        format!("{path}\n")
    );

    // A copy that isn't the build is refused and leaves nothing behind.
    let mut wrong = build.clone();
    wrong.hash = "0".repeat(64);
    let Err(WslError::Failed(message)) = install::install(&shell, &wrong, &|_, _| {}) else {
        panic!("installed a copy that isn't the build");
    };
    assert!(message.contains("nothing was installed"), "{message}");
    assert_eq!(
        sh(&shell, r#"ls -A "$HOME/.local/share/dv/bin""#, &[]),
        format!("dv-{}\n", build.hash)
    );
}

#[test]
fn a_link_folder_dv_cant_write_leaves_the_install_done_and_the_server_starts() {
    let sandbox = sandbox!();
    let (home, shell) = sandbox.home("readonly-bin");
    sh(
        &shell,
        r#"mkdir -p "$HOME/.local/bin" && chmod 555 "$HOME/.local/bin""#,
        &[],
    );
    // Root writes into a folder whatever its mode: there is nothing to refuse then.
    let refused = sh(
        &shell,
        r#"if [ -w "$HOME/.local/bin" ]; then echo no; else echo yes; fi"#,
        &[],
    ) == "yes\n";
    // The app's first open in the distro installs, warns, and connects.
    let connection = Connection::new(&sandbox.distro, sandbox.launcher(&home));
    connection.retry().unwrap();
    assert_eq!(connection.server().unwrap().build_ok, Some(true));
    let builds = builds();
    let probe = install::probe(&shell, &builds).unwrap();
    assert_eq!(probe.build, BuildState::Ok);
    if refused {
        assert_eq!(probe.link, LinkState::None);
        let diagnostics = connection.diagnostics();
        assert!(
            diagnostics.contains(&format!(
                "install: couldn't point {} at this dv (",
                home.join(".local/bin/dv")
            )),
            "{diagnostics}"
        );
        // And an install of its own says so.
        let build = builds.get(Arch::X86_64).unwrap();
        let installed = install::install(&shell, build, &|_, _| {}).unwrap();
        assert!(!installed.linked);
        assert_eq!(installed.warnings.len(), 1, "{:?}", installed.warnings);
    }
    drop(connection);
    sh(&shell, r#"chmod 755 "$HOME/.local/bin""#, &[]);
}

#[test]
fn a_connection_installs_the_build_then_shakes_hands_with_it() {
    let sandbox = sandbox!();
    let (home, _shell) = sandbox.home("connect");
    let connection = Connection::new(&sandbox.distro, sandbox.launcher(&home));
    let (_subscription, rx) = states(&connection);
    // The explicit start an open makes (PLAN §5.2).
    connection.retry().unwrap();
    let seen = states_until(&rx, |state| *state == ConnectionState::Connected);
    assert_eq!(seen.first(), Some(&ConnectionState::Connecting));
    assert!(
        seen.iter().any(
            |state| matches!(state, ConnectionState::Installing { done, total } if done == total)
        ),
        "{seen:?}"
    );
    let server = connection.server().unwrap();
    assert_eq!(server.build_ok, Some(true));
    assert_eq!(
        (server.os.as_str(), server.arch.as_str()),
        ("linux", "x86_64")
    );
    let diagnostics = connection.diagnostics();
    assert!(
        diagnostics.contains(&format!(
            "launcher: wsl.exe -d {} --cd / --exec /usr/bin/env HOME={} /bin/sh -c",
            sandbox.distro, home.path
        )),
        "{diagnostics}"
    );
    drop(connection);

    // Installed now: the next connection starts it without copying it again, automatically
    // (its host check finds the distro running).
    let again = Connection::new(&sandbox.distro, sandbox.launcher(&home));
    let (_subscription, rx) = states(&again);
    again.connect().unwrap();
    let seen = states_until(&rx, |state| *state == ConnectionState::Connected);
    assert_eq!(
        seen,
        [ConnectionState::Connecting, ConnectionState::Connected]
    );
}

#[test]
fn a_repository_in_the_distro_opens_and_its_git_store_and_watch_work() {
    let sandbox = sandbox!();
    let (home, shell) = sandbox.home("repo");
    let repo = home.join("repo");
    scratch_repo(&shell, &repo);
    let connection = Connection::new(&sandbox.distro, sandbox.launcher(&home));
    connection.retry().unwrap();
    let backend = connection.open(&repo, Vec::new()).unwrap();
    assert_eq!(
        backend.repo().location,
        Location::Wsl {
            distro: sandbox.distro.clone(),
            path: repo.clone()
        }
    );
    assert_eq!(backend.repo().toplevel, repo);
    let head = backend
        .git(&["rev-parse", "--abbrev-ref", "HEAD"], &GitOpts::default())
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout), "main\n");
    assert_eq!(backend.worktree_read("a.txt").unwrap().unwrap(), b"a\n");
    backend.store_append("reviews/x.jsonl", b"{}").unwrap();
    assert_eq!(
        backend.store_read("reviews/x.jsonl").unwrap().unwrap(),
        b"{}\n"
    );

    // An edit inside the distro reaches the watch.
    let (handle, events) = watch(&backend, WatchSpec::WORKTREE);
    assert_eq!(handle.worktree_error(), None);
    sh(&shell, r#"printf 'b\n' > "$1/b.txt""#, &[&repo]);
    batch_naming(&events, "b.txt");
    drop(handle);

    // A repository gone is the call's error, not the connection's.
    let missing = connection
        .open(&home.join("none"), Vec::new())
        .err()
        .unwrap();
    assert!(
        !matches!(
            missing,
            BackendError::Unavailable { .. } | BackendError::Disconnected { .. }
        ),
        "{missing:?}"
    );
    assert_eq!(connection.state(), ConnectionState::Connected);
}

/// A process `backend` runs, its events on a channel.
fn spawn(backend: &dyn Backend, spec: &ProcessSpec) -> (ProcessHandle, Receiver<ProcessEvent>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = backend
        .spawn_stdio(
            spec,
            Box::new(move |event| {
                let _ = tx.send(event);
            }),
        )
        .unwrap_or_else(|error| panic!("{spec:?}: {error:?}"));
    (handle, rx)
}

/// What a process printed on stdout and stderr until `done` held, and its exit when it came.
fn printed(
    rx: &Receiver<ProcessEvent>,
    done: impl Fn(&str, Option<Option<i32>>) -> bool,
) -> (String, Vec<String>, Option<Option<i32>>) {
    let (mut stdout, mut stderr, mut exit) = (Vec::new(), Vec::new(), None);
    let deadline = Instant::now() + WITHIN;
    while !done(&String::from_utf8_lossy(&stdout), exit) {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(ProcessEvent::Stdout(bytes)) => stdout.extend(bytes),
            Ok(ProcessEvent::Stderr(line)) => stderr.push(line),
            Ok(ProcessEvent::Exit(code)) => exit = Some(code),
            Err(error) => panic!("not within {WITHIN:?} ({error:?})"),
        }
    }
    (String::from_utf8_lossy(&stdout).into_owned(), stderr, exit)
}

/// Waits until the process `pid` is gone from the distro.
fn assert_gone_in(shell: &Shell, pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let alive = shell
            .run(r#"kill -0 "$1" 2>/dev/null"#, &[&pid.to_string()], WITHIN)
            .unwrap()
            .status
            .success();
        if !alive {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "pid {pid} still runs in the distro"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn a_process_runs_inside_the_distro_and_ends_with_its_connection() {
    let sandbox = sandbox!();
    let (home, shell) = sandbox.home("process");
    let repo = home.join("repo");
    scratch_repo(&shell, &repo);
    sh(&shell, r#"mkdir -p "$1/sub""#, &[&repo]);
    let connection = Connection::new(&sandbox.distro, sandbox.launcher(&home));
    connection.retry().unwrap();
    let backend = connection.open(&repo, Vec::new()).unwrap();

    // Its stdin and stdout, through the distro's own program.
    let (handle, events) = spawn(&backend, &ProcessSpec::new("/usr/bin/cat"));
    handle.write(b"hello from Windows\n").unwrap();
    let (stdout, _, _) = printed(&events, |stdout, _| stdout.ends_with('\n'));
    assert_eq!(stdout, "hello from Windows\n");
    // Its pid is the distro's.
    sh(&shell, r#"kill -0 "$1""#, &[&handle.pid().to_string()]);
    handle.kill();
    let (_, _, exit) = printed(&events, |_, exit| exit.is_some());
    assert_eq!(exit, Some(None), "killed by a signal");
    assert_gone_in(&shell, handle.pid());

    // Its stderr and exit code; its directory and environment.
    let (_handle, events) = spawn(
        &backend,
        &ProcessSpec::new("/usr/bin/ls").args(["/dv-no-such-path"]),
    );
    let (_, stderr, exit) = printed(&events, |_, exit| exit.is_some());
    assert_eq!(exit, Some(Some(2)));
    assert!(
        stderr.iter().any(|line| line.contains("/dv-no-such-path")),
        "{stderr:?}"
    );
    let (_handle, events) = spawn(
        &backend,
        &ProcessSpec::new("/usr/bin/env").env("DV_TEST_VALUE", Some("from the spec")),
    );
    let (stdout, _, exit) = printed(&events, |_, exit| exit.is_some());
    assert_eq!(exit, Some(Some(0)));
    assert!(stdout.contains("DV_TEST_VALUE=from the spec\n"), "{stdout}");
    // The working directory as the process sees it (`getcwd`; `$PWD` is the launch's).
    let (_handle, events) = spawn(
        &backend,
        &ProcessSpec::new("/usr/bin/pwd").args(["-P"]).cwd("sub"),
    );
    let (stdout, _, exit) = printed(&events, |_, exit| exit.is_some());
    assert_eq!(exit, Some(Some(0)));
    assert_eq!(stdout, format!("{repo}/sub\n"));

    // A process outlives neither its handle nor its connection.
    let (handle, events) = spawn(&backend, &ProcessSpec::new("/usr/bin/sleep").args(["600"]));
    let pid = handle.pid();
    sh(&shell, r#"kill -0 "$1""#, &[&pid.to_string()]);
    connection.kill();
    let (_, _, exit) = printed(&events, |_, exit| exit.is_some());
    assert_eq!(exit, Some(None));
    assert_gone_in(&shell, pid);
    drop(handle);
}

#[test]
fn a_build_that_changed_in_the_distro_is_installed_again() {
    let sandbox = sandbox!();
    let (home, shell) = sandbox.home("reinstall");
    let builds = builds();
    let build = builds.get(Arch::X86_64).unwrap();
    install::install(&shell, build, &|_, _| {}).unwrap();
    // Still a program that runs, but not this build's bytes.
    sh(
        &shell,
        r#"printf 'not this build' >> "$HOME/.local/share/dv/bin/dv-$1""#,
        &[&build.hash],
    );
    assert_eq!(
        install::probe(&shell, &builds).unwrap().build,
        BuildState::Corrupt
    );
    let connection = Connection::new(&sandbox.distro, sandbox.launcher(&home));
    connection.retry().unwrap();
    assert_eq!(connection.server().unwrap().build_ok, Some(true));
    let diagnostics = connection.diagnostics();
    assert!(
        diagnostics.contains(&format!(
            "the dv in {} doesn't match this app: installing it again",
            sandbox.distro
        )),
        "{diagnostics}"
    );
    assert!(install::probe(&shell, &builds).unwrap().up_to_date());
}

#[test]
fn a_prune_removes_the_other_builds_and_nothing_else() {
    let sandbox = sandbox!();
    let (_home, shell) = sandbox.home("prune");
    let builds = builds();
    let build = builds.get(Arch::X86_64).unwrap();
    install::install(&shell, build, &|_, _| {}).unwrap();
    sh(
        &shell,
        r#"set -e
cd "$HOME/.local/share/dv"
touch bin/dv-0123 bin/dv-4567
mkdir -p cli host
echo theirs > cli/dv
echo theirs > host/dv-host
"#,
        &[],
    );
    let mut pruned = install::prune(&shell, &builds).unwrap();
    pruned.sort();
    assert_eq!(pruned, ["dv-0123", "dv-4567"]);
    assert_eq!(
        sh(
            &shell,
            r#"cd "$HOME/.local/share/dv" && ls -A bin && cat cli/dv host/dv-host"#,
            &[]
        ),
        format!("dv-{}\ntheirs\ntheirs\n", build.hash)
    );
}

#[test]
fn a_distro_that_isnt_there_is_unavailable_with_wsls_words() {
    let sandbox = sandbox!();
    let launcher = Launcher::Wsl {
        distro: "dv-no-such-distro".into(),
        builds: builds_dir(),
        home: Some(sandbox.root.clone()),
    };
    // An automatic start asks first, and launches nothing on a distro that isn't running.
    let connection = Connection::new("dv-no-such-distro", launcher.clone());
    let error = connection.connect().unwrap_err();
    assert_eq!(
        error.to_string(),
        "dv-no-such-distro can't be opened: dv-no-such-distro isn't running"
    );
    assert_eq!(connection.state(), ConnectionState::Stopped);
    assert_eq!(connection.generation(), 0);
    // An explicit start tries, and says what wsl.exe said.
    let connection = Connection::new("dv-no-such-distro", launcher);
    let error = connection.retry().unwrap_err();
    let BackendError::Unavailable { location, reason } = &error else {
        panic!("{error:?}");
    };
    assert_eq!(location, "dv-no-such-distro");
    assert!(
        reason.starts_with("the connection to dv failed: couldn't start dv: wsl.exe: ")
            && reason.contains("WSL_E_DISTRO_NOT_FOUND"),
        "{reason}"
    );
}
