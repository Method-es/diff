//! A test helper, never shipped: the process the conformance suite's process cases spawn
//! (`tests/conformance/process.rs`, docs/PLAN.md §14.2, §18.3), and a child-process loopback
//! server for them. No shell: each argument is a step, run in order.
//!
//! ```text
//! dv_test_child serve              `dv_backend::server::serve` on stdin and stdout, as
//!                                  `dv backend --stdio` does
//! dv_test_child <step>…            out <text>     print <text> and a newline on stdout
//!                                  err <text>     the same on stderr
//!                                  echo           copy stdin to stdout until it ends
//!                                  cwd            print the working directory
//!                                  env <name>     print the variable, or <unset>
//!                                  pid            print `pid <its pid>`
//!                                  child          start `dv_test_child sleep 600000` and
//!                                                 print `child <its pid>`
//!                                  pidfile <path> write its pid to <path>
//!                                  flood <path>   print 64 KiB at a time on stdout until
//!                                                 killed, the total so far in <path>
//!                                  sleep <ms>     wait
//!                                  exit <code>    exit with <code>
//! ```
//!
//! It exits 0 after its last step; a step it doesn't know exits 101.

use std::io::{self, Read as _, Write as _};
use std::process::{Command, Stdio};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("serve") {
        if let Err(error) = dv_backend::server::serve(io::stdin(), io::stdout()) {
            eprintln!("serve: {error}");
            std::process::exit(1);
        }
        return;
    }
    let mut steps = args.iter();
    let mut out = io::stdout();
    while let Some(step) = steps.next() {
        let mut value = || steps.next().cloned().unwrap_or_default();
        match step.as_str() {
            "out" => {
                let _ = writeln!(out, "{}", value());
                let _ = out.flush();
            }
            "err" => eprintln!("{}", value()),
            "echo" => echo(&mut out),
            "cwd" => {
                let cwd = std::env::current_dir().map(|dir| dir.display().to_string());
                let _ = writeln!(out, "{}", cwd.unwrap_or_else(|error| error.to_string()));
                let _ = out.flush();
            }
            "env" => {
                let name = value();
                let shown = std::env::var(&name).unwrap_or_else(|_| "<unset>".into());
                let _ = writeln!(out, "{shown}");
                let _ = out.flush();
            }
            "pid" => {
                let _ = writeln!(out, "pid {}", std::process::id());
                let _ = out.flush();
            }
            "child" => {
                let child = std::env::current_exe().and_then(|exe| {
                    Command::new(exe)
                        .args(["sleep", "600000"])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                });
                match child {
                    Ok(child) => {
                        let _ = writeln!(out, "child {}", child.id());
                        let _ = out.flush();
                    }
                    Err(error) => {
                        eprintln!("child: {error}");
                        std::process::exit(1);
                    }
                }
            }
            "pidfile" => {
                let _ = std::fs::write(value(), std::process::id().to_string());
            }
            "flood" => flood(&mut out, &value()),
            "sleep" => {
                let ms = value().parse().unwrap_or(0);
                std::thread::sleep(Duration::from_millis(ms));
            }
            "exit" => std::process::exit(value().parse().unwrap_or(1)),
            other => {
                eprintln!("dv_test_child: no step {other:?}");
                std::process::exit(101);
            }
        }
    }
}

/// Copies stdin to stdout, flushing each chunk, until stdin ends.
fn echo(out: &mut io::Stdout) {
    let mut stdin = io::stdin();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match stdin.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if out
                    .write_all(&buffer[..read])
                    .and_then(|()| out.flush())
                    .is_err()
                {
                    return;
                }
            }
        }
    }
}

/// Prints 64 KiB at a time until stdout breaks, writing the total so far to `path` after each.
fn flood(out: &mut io::Stdout, path: &str) {
    let chunk = vec![b'y'; 64 * 1024];
    let mut total = 0usize;
    while out.write_all(&chunk).and_then(|()| out.flush()).is_ok() {
        total += chunk.len();
        let _ = std::fs::write(path, total.to_string());
    }
}
