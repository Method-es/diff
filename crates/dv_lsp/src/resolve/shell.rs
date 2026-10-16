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

use std::sync::mpsc;
use std::time::{Duration, Instant};

use dv_core::backend::{Backend, PathProbe, ProcessEvent, ProcessSpec};

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

/// The machine of `backend`, a POSIX one (a WSL distro), as its user's shell sees it (module
/// docs). Never fails: what can't be read is left out, the reason logged.
pub fn read(backend: &dyn Backend) -> HostEnv {
    read_with(backend, "/usr/bin/env", SHELL_TIMEOUT)
}

/// [`read`] with another `env` program and timeout (tests).
pub fn read_with(backend: &dyn Backend, env_program: &str, timeout: Duration) -> HostEnv {
    // `-0` first; an `env` without it (BusyBox's) prints lines.
    let mut env = Vec::new();
    for args in [&["-0"][..], &[]] {
        let spec = ProcessSpec::new(env_program).args(args.iter().copied());
        match run(backend, spec, ENV_TIMEOUT, |_| false) {
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
        .or_else(|| passwd_shell(backend, var("USER").or_else(|| var("LOGNAME"))?));
    let home = var("HOME");
    if let Some(shell) = shell {
        for flags in TRIES {
            match shell_path(backend, &shell, flags, timeout) {
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
    backend: &dyn Backend,
    shell: &str,
    flags: &[&str],
    timeout: Duration,
) -> Result<Printed, String> {
    let nonce = nonce();
    let spec = ProcessSpec::new("/bin/sh")
        .args(["-c", "exec \"$0\" \"$@\" </dev/null", shell])
        .args(flags.iter().copied())
        .args(["-c", &print_script()])
        .env("DV_PATH_NONCE", Some(&nonce));
    let ran = run(backend, spec, timeout, |stdout| {
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
fn passwd_shell(backend: &dyn Backend, user: &str) -> Option<String> {
    let PathProbe::File {
        contents: Some(bytes),
        ..
    } = backend.probe_path("/etc/passwd").ok()?
    else {
        return None;
    };
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

/// Runs `spec` through `backend` until it ends, or until `enough` says the stdout so far holds
/// what is wanted, killing it then (dropping the handle kills it and everything it started) or
/// after `timeout`.
fn run(
    backend: &dyn Backend,
    spec: ProcessSpec,
    timeout: Duration,
    enough: impl Fn(&[u8]) -> bool,
) -> Result<Ran, String> {
    let (sender, events) = mpsc::channel();
    let handle = backend
        .spawn_stdio(
            &spec,
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
