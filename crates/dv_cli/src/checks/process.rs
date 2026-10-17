//! Running the small programs the checks ask (git, `gh`): to their end or a timeout, stdin
//! empty, output collected, no console window (the app runs detached, and a console child of it
//! would flash one), dying with dv (`dv_core::job`).

use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a program printed and how it ended.
#[derive(Debug)]
pub(crate) struct Ran {
    /// `None` when a signal ended it.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// Runs `command` to its end, killing it after `timeout` (an [`io::ErrorKind::TimedOut`]
/// error). A program that isn't there is [`io::ErrorKind::NotFound`].
pub(crate) fn run(mut command: Command, timeout: Duration) -> io::Result<Ran> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    no_window(&mut command);
    let mut child = command.spawn()?;
    dv_core::job::tie_to_this_process(&child);
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let stdout = read(child.stdout.take().map(|pipe| Box::new(pipe) as _));
    let stderr = read(child.stderr.take().map(|pipe| Box::new(pipe) as _));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("it didn't finish within {} s", timeout.as_secs()),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Ok(Ran {
        code: status.code(),
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// Applies `env` (`Some` sets, `None` removes) to `command`.
pub(crate) fn apply_env(
    command: &mut Command,
    env: &[(std::ffi::OsString, Option<std::ffi::OsString>)],
) {
    for (name, value) in env {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
}

#[cfg(windows)]
fn no_window(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    /// `CREATE_NO_WINDOW` from the Win32 process creation flags.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn no_window(_command: &mut Command) {}
