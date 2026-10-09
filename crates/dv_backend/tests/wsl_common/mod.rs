//! What the real-WSL checks share (`tests/wsl.rs`, the agents' sandboxed run, and
//! `tests/live_wsl.rs`, the owner's): scripts run in the distro, scratch folders under its
//! `/tmp` removed afterwards, scratch repositories, this app's Linux builds, and waiting for
//! states and watch batches.

#![allow(dead_code, reason = "each test file uses its own share of these")]

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use dv_backend::install::{LINUX_DIR_VAR, LinuxBuilds};
use dv_backend::wsl::{Arch, Shell};
use dv_backend::{Connection, ConnectionState, Subscription};
use dv_core::backend::{Backend, WatchEvent, WatchHandle, WatchSpec};

pub const WITHIN: Duration = Duration::from_secs(60);

/// The folder of this app's Linux builds: `DV_LINUX_DIR`, else `target/debug` (this test's
/// executable is in `target/debug/deps`), where `script/build-linux` copies them.
pub fn builds_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(LINUX_DIR_VAR) {
        return PathBuf::from(dir);
    }
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().to_path_buf()
}

/// This app's Linux builds; the x86_64 one must be there.
pub fn builds() -> LinuxBuilds {
    let dir = builds_dir();
    let builds = LinuxBuilds::find(&dir).unwrap();
    assert!(
        builds.get(Arch::X86_64).is_some(),
        "{} has no dv-linux-x86_64: run script/build-linux first",
        dir.display()
    );
    builds
}

/// `script` run by `shell` with `args` to its end; its stdout when it succeeds.
pub fn sh(shell: &Shell, script: &str, args: &[&str]) -> String {
    let output = shell.run(script, args, WITHIN).unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{script} failed ({}): {stdout}{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

/// A folder of a test's own under `/tmp` in a distro, made empty, removed when dropped (by
/// the shell that made it).
pub struct TmpDir {
    pub shell: Shell,
    pub path: String,
}

impl TmpDir {
    /// `<parent>/<name>-<this process>`; `parent` must be under `/tmp`.
    pub fn new(shell: &Shell, parent: &str, name: &str) -> Self {
        let path = format!(
            "{}/{name}-{}",
            parent.trim_end_matches('/'),
            std::process::id()
        );
        assert!(
            path.starts_with("/tmp/") && !path.split('/').any(|part| part == ".."),
            "scratch folders live under /tmp, not {path}"
        );
        sh(shell, r#"rm -rf -- "$1" && mkdir -p -- "$1""#, &[&path]);
        Self {
            shell: shell.clone(),
            path,
        }
    }

    /// `<path>/<rel>`.
    pub fn join(&self, rel: &str) -> String {
        format!("{}/{rel}", self.path)
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let removed = self.shell.run(r#"rm -rf -- "$1""#, &[&self.path], WITHIN);
        if !removed.is_ok_and(|output| output.status.success()) {
            eprintln!("couldn't remove {} in {}", self.path, self.shell.name());
        }
    }
}

/// A repository at `path` in the distro: `a.txt` committed on `main`, made with a fixed
/// identity and no configuration of the user's.
pub fn scratch_repo(shell: &Shell, path: &str) {
    sh(
        shell,
        r#"set -e
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
git init -q -b main "$1"
cd "$1"
printf 'a\n' > a.txt
git add a.txt
git -c user.name=dv -c user.email=dv@example.com commit -qm base
"#,
        &[path],
    );
}

/// Every state `connection` reports from now on, on a channel.
pub fn states(connection: &Connection) -> (Subscription, Receiver<ConnectionState>) {
    let (tx, rx) = mpsc::channel();
    let subscription = connection.subscribe(move |state| {
        let _ = tx.send(state.clone());
    });
    (subscription, rx)
}

/// The states reported until one satisfies `last` (included).
pub fn states_until(
    rx: &Receiver<ConnectionState>,
    last: impl Fn(&ConnectionState) -> bool,
) -> Vec<ConnectionState> {
    let deadline = Instant::now() + WITHIN;
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(state) => {
                let done = last(&state);
                seen.push(state);
                if done {
                    return seen;
                }
            }
            Err(error) => panic!("no such state ({error:?}); saw {seen:?}"),
        }
    }
}

pub fn watch(backend: &dyn Backend, spec: WatchSpec) -> (WatchHandle, Receiver<WatchEvent>) {
    let (tx, rx) = mpsc::channel();
    let handle = backend
        .watch(
            spec,
            Box::new(move |event| {
                let _ = tx.send(event);
            }),
        )
        .unwrap();
    (handle, rx)
}

/// Waits for a batch naming worktree path `rel`.
pub fn batch_naming(events: &Receiver<WatchEvent>, rel: &str) -> WatchEvent {
    let deadline = Instant::now() + WITHIN;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = events
            .recv_timeout(left)
            .unwrap_or_else(|error| panic!("no batch naming {rel} ({error:?})"));
        if event.worktree_paths.iter().any(|path| path == rel) {
            return event;
        }
    }
}
