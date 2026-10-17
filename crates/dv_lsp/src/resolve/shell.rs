//! A distro user's `PATH`, read from their shell through the backend (docs/PLAN.md §18.4,
//! resolver 3), once per connection.
//!
//! The backend runs under `wsl.exe --exec`, without the user's profile, so its own `PATH` lacks
//! what their shell's startup files add. A login shell isn't enough: bun's installer and nvm
//! edit `.zshrc` / `.bashrc`, which only an interactive shell reads (checked 2026-10-17 on the
//! owner's Ubuntu: `zsh -lc` has no `~/.bun/bin`, `zsh -lic` has). So [`read`]:
//!
//! 1. runs `/usr/bin/env -0` for the backend's environment: `SHELL`, `HOME`, `PATH`, `USER`;
//! 2. finds the user's shell: `$SHELL`, else their `/etc/passwd` entry, else none;
//! 3. runs it as `<shell> -l -i -c '<print>'` with stdin from `/dev/null` (through `/bin/sh`,
//!    the one way to close it, since a spawned process's stdin is a pipe), and takes only what
//!    is between two markers made for the call, because an interactive shell's startup files
//!    print banners and terminal titles; the shell is killed as soon as the end marker is in,
//!    or after [`SHELL_TIMEOUT`];
//! 4. failing that, `<shell> -i -c` (csh and tcsh take `-l` only alone), then `<shell> -l -c`,
//!    then the backend's own `PATH`.
//!
//! The command prints with `printf` and `printenv`, which every shell runs the same way. The
//! marker text never appears whole in it, so a shell that echoes its commands (`set -x`,
//! `setopt verbose`) can't fake one. Nothing runs through a shell but these commands, and they
//! only print.
//!
//! [`read_here`] reads this machine's user the same way, without a repository: `dv doctor`
//! inside a distro asking whether `~/.local/bin` is on the user's `PATH` (PLAN §12.2). There
//! the shell runs in a session of its own (`setsid -w`, when there is one): dv may run in a
//! terminal, which an interactive shell would otherwise try to take, and be stopped for it.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use dv_core::backend::{
    Backend, BackendError, PathProbe, ProcessEvent, ProcessHandle, ProcessSink, ProcessSpec,
};

use super::host::{HostEnv, PathFrom};
use crate::paths::PathStyle;

/// How long each shell may take to print its `PATH`.
pub const SHELL_TIMEOUT: Duration = Duration::from_secs(4);

/// How long `/usr/bin/env` may take.
const ENV_TIMEOUT: Duration = Duration::from_secs(4);

/// The shell's flags, in the order they are tried (module docs).
const TRIES: [&[&str]; 3] = [&["-l", "-i"], &["-i"], &["-l"]];

/// What a command printed before it ended (or before what was wanted was in).
#[derive(Debug)]
struct Ran {
    stdout: Vec<u8>,
    code: Option<i32>,
}

/// Where the commands run: through a repository's backend, or on this machine.
enum On<'a> {
    Backend(&'a dyn Backend),
    /// This machine, every command with these environment changes (`dv doctor`'s home).
    Here(&'a [(String, Option<String>)]),
}

impl On<'_> {
    fn spawn(&self, spec: ProcessSpec, sink: ProcessSink) -> Result<ProcessHandle, BackendError> {
        match self {
            Self::Backend(backend) => backend.spawn_stdio(&spec, sink),
            Self::Here(env) => {
                let mut spec = spec;
                spec.env.extend(env.iter().cloned());
                dv_core::backend::process::spawn_in(std::path::Path::new("/"), &spec, sink)
            }
        }
    }

    /// A small file's bytes, `None` when it can't be read.
    fn read_small(&self, path: &str) -> Option<Vec<u8>> {
        match self {
            Self::Backend(backend) => match backend
                .probe_path(path, dv_core::backend::PROBE_READ_SMALL)
                .ok()?
            {
                PathProbe::File {
                    contents: Some(bytes),
                    ..
                } => Some(bytes),
                _ => None,
            },
            Self::Here(_) => std::fs::read(path).ok(),
        }
    }
}

/// The machine of `backend`, a POSIX one (a WSL distro), as its user's shell sees it (module
/// docs). Never fails: what can't be read is left out, the reason logged.
pub fn read(backend: &dyn Backend) -> HostEnv {
    read_with(backend, "/usr/bin/env", SHELL_TIMEOUT)
}

/// This machine, a POSIX one, as its user's shell sees it, every command run with `env`'s
/// changes (`HOME`, so a test's scratch home is the one the shell reads): [`read`] without a
/// repository (module docs).
pub fn read_here(env: &[(String, Option<String>)]) -> HostEnv {
    read_on(&On::Here(env), "/usr/bin/env", SHELL_TIMEOUT)
}

/// [`read`] with another `env` program and timeout (tests).
pub fn read_with(backend: &dyn Backend, env_program: &str, timeout: Duration) -> HostEnv {
    read_on(&On::Backend(backend), env_program, timeout)
}

fn read_on(on: &On<'_>, env_program: &str, timeout: Duration) -> HostEnv {
    // `-0` first; an `env` without it (BusyBox's) prints lines.
    let mut env = Vec::new();
    for args in [&["-0"][..], &[]] {
        let spec = ProcessSpec::new(env_program).args(args.iter().copied());
        match run(on, spec, ENV_TIMEOUT, |_| false) {
            Ok(ran) => env = parse_env(&String::from_utf8_lossy(&ran.stdout)),
            Err(error) => log::info!("reading the backend's environment: {error}"),
        }
        if !env.is_empty() {
            break;
        }
    }
    let var = |name: &str| {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty())
    };
    let shell = var("SHELL")
        .filter(|shell| shell.starts_with('/'))
        .map(str::to_string)
        .or_else(|| passwd_shell(on, var("USER").or_else(|| var("LOGNAME"))?));
    let home = var("HOME");
    if let Some(shell) = shell {
        for flags in TRIES {
            match shell_path(on, &shell, flags, timeout) {
                Ok(printed) => {
                    return HostEnv::new(
                        PathStyle::Posix,
                        Some(&printed.path),
                        home,
                        printed.bun_install.as_deref().or(var("BUN_INSTALL")),
                        PathFrom::Shell {
                            shell,
                            flags: flags.join(" "),
                        },
                    );
                }
                Err(error) => {
                    log::info!("reading PATH from {shell} {}: {error}", flags.join(" "));
                }
            }
        }
    }
    let from = if var("PATH").is_some() {
        PathFrom::Backend
    } else {
        PathFrom::Unknown
    };
    HostEnv::new(
        PathStyle::Posix,
        var("PATH"),
        home,
        var("BUN_INSTALL"),
        from,
    )
}

/// What the shell printed between the markers.
#[derive(Debug, PartialEq, Eq)]
struct Printed {
    path: String,
    bun_install: Option<String>,
}

/// `shell`'s `PATH` (and `BUN_INSTALL`), as `<shell> <flags> -c` prints it.
fn shell_path(
    on: &On<'_>,
    shell: &str,
    flags: &[&str],
    timeout: Duration,
) -> Result<Printed, String> {
    let nonce = nonce();
    // On this machine dv may run in a terminal (`dv doctor`): an interactive shell in a process
    // group of its own there would take the terminal and be stopped (SIGTTOU), so it runs in a
    // session of its own, without one. A backend's process has no terminal to take.
    let start = match on {
        On::Here(_) => {
            "if command -v setsid >/dev/null 2>&1; then exec setsid -w \"$0\" \"$@\" </dev/null; \
             fi; exec \"$0\" \"$@\" </dev/null"
        }
        On::Backend(_) => "exec \"$0\" \"$@\" </dev/null",
    };
    let spec = ProcessSpec::new("/bin/sh")
        .args(["-c", start, shell])
        .args(flags.iter().copied())
        .args(["-c", &print_script()])
        .env("DV_PATH_NONCE", Some(&nonce));
    let ran = run(on, spec, timeout, |stdout| {
        between_markers(&String::from_utf8_lossy(stdout), &nonce).is_some()
    })?;
    let stdout = String::from_utf8_lossy(&ran.stdout);
    between_markers(&stdout, &nonce).ok_or_else(|| match ran.code {
        Some(0) => "it printed no PATH".to_string(),
        Some(code) => format!("it exited with code {code} without printing a PATH"),
        None => "it was killed without printing a PATH".to_string(),
    })
}

/// The command the shell runs: the begin marker, `PATH`, `BUN_INSTALL` (nothing when it is
/// unset) and the end marker, a line each. Each marker is printed in two parts around the
/// nonce, which the environment carries, so the command's own text holds neither marker.
fn print_script() -> String {
    "printf '%s%s\\n' __dv_path_begin_ \"$DV_PATH_NONCE\"; printenv PATH; printenv BUN_INSTALL; \
     printf '%s%s\\n' __dv_path_end_ \"$DV_PATH_NONCE\""
        .to_string()
}

/// The `PATH` and `BUN_INSTALL` lines between this call's markers in `stdout`.
fn between_markers(stdout: &str, nonce: &str) -> Option<Printed> {
    let stdout = stdout.replace("\r\n", "\n");
    let begin = format!("__dv_path_begin_{nonce}\n");
    let end = format!("\n__dv_path_end_{nonce}");
    let start = stdout.find(&begin)? + begin.len();
    let length = stdout[start..].find(&end)?;
    let inside = &stdout[start..start + length];
    let mut lines = inside.split('\n');
    let path = lines.next()?.to_string();
    let bun_install = lines
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    (!path.trim().is_empty()).then_some(Printed { path, bun_install })
}

/// `NAME=value` entries of `env -0`'s output, each ended by a NUL (an `env` without `-0`
/// prints lines, and a value spanning lines then keeps only its first).
fn parse_env(output: &str) -> Vec<(String, String)> {
    let entries: Vec<&str> = if output.contains('\0') {
        output.split('\0').collect()
    } else {
        output.lines().collect()
    };
    entries
        .into_iter()
        .filter_map(|entry| {
            let (name, value) = entry.split_once('=')?;
            let plain = !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
            plain.then(|| (name.to_string(), value.to_string()))
        })
        .collect()
}

/// `user`'s login shell from `/etc/passwd`.
fn passwd_shell(on: &On<'_>, user: &str) -> Option<String> {
    let bytes = on.read_small("/etc/passwd")?;
    String::from_utf8_lossy(&bytes).lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        (fields.len() >= 7 && fields[0] == user && fields[6].starts_with('/'))
            .then(|| fields[6].to_string())
    })
}

/// A marker part no other call uses.
fn nonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!(
        "{:x}{:x}{:x}",
        std::process::id(),
        time & 0xffff_ffff,
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Runs `spec` where `on` says until it ends, or until `enough` says the stdout so far holds
/// what is wanted, killing it then (dropping the handle kills it and everything it started) or
/// after `timeout`.
fn run(
    on: &On<'_>,
    spec: ProcessSpec,
    timeout: Duration,
    enough: impl Fn(&[u8]) -> bool,
) -> Result<Ran, String> {
    let (sender, events) = mpsc::channel();
    let handle = on
        .spawn(
            spec,
            Box::new(move |event| {
                let _ = sender.send(event);
            }),
        )
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + timeout;
    let mut stdout = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(ProcessEvent::Stdout(chunk)) => {
                stdout.extend_from_slice(&chunk);
                if enough(&stdout) {
                    drop(handle);
                    return Ok(Ran { stdout, code: None });
                }
            }
            Ok(ProcessEvent::Stderr(_)) => {}
            Ok(ProcessEvent::Exit(code)) => return Ok(Ran { stdout, code }),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                drop(handle);
                return Err(format!("it didn't finish within {} s", timeout.as_secs()));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(Ran { stdout, code: None }),
        }
    }
}

#[cfg(test)]
mod tests;
