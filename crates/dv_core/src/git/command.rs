//! The one place dv builds and runs a `git` process: the invocation policy (docs/PLAN.md §5.1,
//! §2.4, D4).
//!
//! Every git dv runs gets the same treatment, so its output is parseable and the same on every
//! machine whatever the user's configuration says about presentation:
//!
//! - `LC_ALL=C`: untranslated messages and C collation, because dv parses git's output.
//! - `--no-pager` and `GIT_PAGER=cat`: nothing waits on a pager.
//! - `GIT_TERMINAL_PROMPT=0`: a credential prompt fails instead of hanging a headless process.
//! - `GIT_OPTIONAL_LOCKS=0` and `-c diff.autoRefreshIndex=false`: read commands never refresh
//!   and rewrite the index, so dv never changes the user's index (PLAN §2.4) and never holds
//!   `index.lock` while an agent runs `git add`. `GIT_OPTIONAL_LOCKS` covers `git status`; the
//!   worktree forms of `git diff` (`git diff`, `git diff HEAD`) ignore it and rewrite the index
//!   after finding stat-dirty files unchanged, which only `diff.autoRefreshIndex=false` stops.
//!   The price: those diffs list a stat-dirty but unchanged file in `--name-status` (as `M`),
//!   while `--numstat` and the patch leave it out, so the file list keeps only entries that
//!   `--numstat` reports (PLAN §6).
//! - `-c core.quotepath=off`: paths print as UTF-8, not octal escapes.
//! - `-c color.ui=never -c color.diff=never`: no escape codes. `color.diff` needs its own
//!   override because a configured `color.diff=always` wins over `color.ui`.
//! - `-c diff.algorithm=myers`: the algorithm GitHub uses (PLAN D4), whatever `diff.algorithm`
//!   the user prefers. A diff driver's `diff.<driver>.algorithm` beats this key, so every
//!   `git diff` dv runs also passes `--diff-algorithm=myers`, which beats both; that and the
//!   other diff presentation settings (`diff.indentHeuristic`, `diff.submodule`,
//!   `diff.ignoreSubmodules`, textconv) are pinned by flags on the diff commands themselves
//!   (`diff::DIFF_PINS`).
//! - The inherited variables that point git at another repository, index or configuration, or
//!   that change how pathspecs and diffs are interpreted ([`DROPPED_ENV`]), are removed, because
//!   dv may be started from a git hook or an alias that exports them.
//! - On Windows the process is created with `CREATE_NO_WINDOW`, so no console flashes up.
//!
//! The user's own configuration otherwise applies (autocrlf, attributes, excludes): that is the
//! repo dv is reviewing. Tests replace it with the fixture's through an environment overlay
//! (`LocalBackendOptions::env`), applied before the policy so the policy always wins.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::backend::{BackendError, GitOpts, GitOutput};

/// `-c` settings every git call gets (module docs).
pub const POLICY_CONFIG: &[(&str, &str)] = &[
    ("core.quotepath", "off"),
    ("color.ui", "never"),
    ("color.diff", "never"),
    ("diff.algorithm", "myers"),
    ("diff.autoRefreshIndex", "false"),
];

/// Environment variables every git call gets (module docs).
pub const POLICY_ENV: &[(&str, &str)] = &[
    ("LC_ALL", "C"),
    ("GIT_PAGER", "cat"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_OPTIONAL_LOCKS", "0"),
];

/// Inherited variables removed from every git call: they redirect git to another repository,
/// index or configuration, or change how pathspecs (dv uses `:(literal)` magic, which
/// `GIT_LITERAL_PATHSPECS` would disable) and diffs (`GIT_DIFF_OPTS` sets the context size;
/// `GIT_ATTR_SOURCE`, which `git --attr-source=<tree>` exports, reads `.gitattributes` from that
/// tree instead of the worktree and index, so `-diff`, `binary` and `eol` follow the wrong
/// rules, and fails every attribute lookup in a repo without commits when it names `HEAD`) are
/// interpreted. The test fixture's overlay (`dv_test_support::FixtureRepo::env_overlay`)
/// removes each of these too, so tests' reference git calls match; a test checks that.
pub const DROPPED_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_QUARANTINE_PATH",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_LITERAL_PATHSPECS",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
    "GIT_DIFF_OPTS",
    "GIT_EXTERNAL_DIFF",
    "GIT_ATTR_SOURCE",
];

/// An environment change: `Some` sets the variable, `None` removes it.
pub type EnvOverlay = [(OsString, Option<OsString>)];

/// A `git` command running in `cwd` with `overlay` applied and then the policy (module docs).
/// The caller adds the subcommand and its arguments.
pub fn command(cwd: &Path, overlay: &EnvOverlay) -> Command {
    command_with_index(cwd, overlay, None)
}

/// [`command`] using `index_file` as its index (`GIT_INDEX_FILE`), set after the policy removed
/// any inherited one; `None` is the worktree's own index.
fn command_with_index(cwd: &Path, overlay: &EnvOverlay, index_file: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    command.current_dir(cwd);
    apply_overlay(&mut command, overlay);
    for name in DROPPED_ENV {
        command.env_remove(name);
    }
    for (name, value) in POLICY_ENV {
        command.env(name, value);
    }
    if let Some(index_file) = index_file {
        command.env("GIT_INDEX_FILE", index_file);
    }
    command.arg("--no-pager");
    for (key, value) in POLICY_CONFIG {
        command.arg("-c").arg(format!("{key}={value}"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        /// `CREATE_NO_WINDOW` from the Win32 process creation flags.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Applies an environment overlay to `command`: sets the `Some` variables, removes the `None`
/// ones.
pub fn apply_overlay(command: &mut Command, overlay: &EnvOverlay) {
    for (name, value) in overlay {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
}

/// How long the output pipes may stay open after git exits, at the least, when the call has a
/// timeout: ample for reading what git left in them, which a reader thread has mostly read
/// already.
const DRAIN_GRACE: Duration = Duration::from_millis(100);

/// Runs `git <args>` in `cwd` (see [`command`]) and collects its output. Fails with
/// [`BackendError::Git`] when the exit code isn't one of `opts.ok_codes`, and with
/// [`BackendError::Timeout`] when `opts.timeout_ms` passes first: git is killed if it is still
/// running; if git has exited but a process it started still holds its output pipes open (an
/// alias or hook that backgrounds a command), the call stops waiting for that process at the
/// deadline (or [`DRAIN_GRACE`] after git exits, whichever is later).
///
/// `opts.index_file` is a store path only a backend can resolve; this runs with the worktree's
/// own index and refuses a call that names another ([`run_with_index`] takes the resolved path).
pub fn run(
    cwd: &Path,
    overlay: &EnvOverlay,
    args: &[&str],
    opts: &GitOpts,
) -> Result<GitOutput, BackendError> {
    if let Some(rel) = &opts.index_file {
        return Err(BackendError::InvalidPath {
            path: rel.clone(),
            reason: "an index file is resolved by a backend",
        });
    }
    run_with_index(cwd, overlay, None, args, opts)
}

/// [`run`] with `index_file`, already resolved from `opts.index_file` by the backend, as the
/// call's index (`GIT_INDEX_FILE`).
pub fn run_with_index(
    cwd: &Path,
    overlay: &EnvOverlay,
    index_file: Option<&Path>,
    args: &[&str],
    opts: &GitOpts,
) -> Result<GitOutput, BackendError> {
    let owned_args = || args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
    let deadline = opts
        .timeout_ms
        .map(|ms| (Instant::now() + Duration::from_millis(ms), ms));
    let mut command = command_with_index(cwd, overlay, index_file);
    command
        .args(args)
        .stdin(if opts.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|source| BackendError::Spawn {
        args: owned_args(),
        source,
    })?;

    // stdin, stdout and stderr each get a thread, so a large output on one pipe can't block git
    // while dv waits on another.
    if let (Some(bytes), Some(mut pipe)) = (opts.stdin.clone(), child.stdin.take()) {
        std::thread::spawn(move || {
            // git may exit without reading all of its stdin (an early error); the exit code
            // reports that, so a broken pipe here is not an error of its own.
            let _ = pipe.write_all(&bytes);
        });
    }
    let stdout = read_in_background(child.stdout.take());
    let stderr = read_in_background(child.stderr.take());

    let timed_out = |timeout_ms| BackendError::Timeout {
        args: owned_args(),
        timeout_ms,
    };
    let status = match deadline {
        None => child.wait(),
        Some((at, ms)) => match wait_until(&mut child, at) {
            Ok(Some(status)) => Ok(status),
            Ok(None) => {
                // Killing closes git's pipes, which ends the reader threads; they are left to
                // finish on their own in case a grandchild still holds a pipe open.
                let _ = child.kill();
                let _ = child.wait();
                return Err(timed_out(ms));
            }
            Err(err) => Err(err),
        },
    }
    .map_err(|source| BackendError::io("waiting for git in", cwd, source))?;

    // git has exited, so its pipes close unless a process it started holds a copy; with a
    // timeout, that process gets until the deadline, after which the reader threads are left
    // to finish on their own.
    let drain_until = deadline.map(|(at, ms)| (at.max(Instant::now() + DRAIN_GRACE), ms));
    let drain = |rx: &mpsc::Receiver<Vec<u8>>| match drain_until {
        None => Ok(rx.recv().unwrap_or_default()),
        Some((at, ms)) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
            Ok(bytes) => Ok(bytes),
            Err(RecvTimeoutError::Disconnected) => Ok(Vec::new()),
            Err(RecvTimeoutError::Timeout) => Err(timed_out(ms)),
        },
    };
    let stdout = drain(&stdout)?;
    let stderr = drain(&stderr)?;
    match status.code() {
        Some(code) if opts.ok_codes.contains(&code) => Ok(GitOutput {
            code,
            stdout,
            stderr,
        }),
        code => Err(BackendError::Git {
            args: owned_args(),
            code,
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        }),
    }
}

/// Reads a pipe to its end on a new thread; the receiver yields the bytes (empty on a read
/// error or a missing pipe).
fn read_in_background(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        let _ = tx.send(bytes);
    });
    rx
}

/// Waits for `child` to exit until `deadline`; `Ok(None)` when it is still running then. Polls
/// with a backoff from 1 ms to 25 ms, because std offers no timed wait on a child.
fn wait_until(child: &mut Child, deadline: Instant) -> std::io::Result<Option<ExitStatus>> {
    let mut pause = Duration::from_millis(1);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        std::thread::sleep(pause.min(deadline - now));
        pause = (pause * 2).min(Duration::from_millis(25));
    }
}
