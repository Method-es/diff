//! End-to-end proofs against the example harness. Ignored by default because
//! they open windows on the desktop:
//!
//! ```text
//! cargo test -p gpui_automation --test harness_e2e -- --ignored
//! ```
//!
//! Each test first builds what it launches (the harness example, and
//! `mock_gh` for the `gh` scenario) into this test's own target dir and
//! profile, so it never runs a stale binary: `cargo test` itself builds the
//! `e2e` runner but not examples or other packages' binaries.
//!
//! The scenarios write to output dirs of their own,
//! `<target>/e2e-harness-<pid>` and `<target>/e2e-harness-gh-<pid>`, so
//! concurrent runs of these tests in one checkout (or an `e2e` run on the
//! default `<target>/e2e`) never clear each other's artifacts. Each is
//! removed when its scenarios pass and kept for post-mortems when they fail.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Where cargo put this test's binaries: `<target>/<profile>`.
fn profile_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_e2e"))
        .parent()
        .expect("the runner lives in a target directory")
        .to_path_buf()
}

/// The target dir this test was built into.
fn target_dir() -> PathBuf {
    profile_dir()
        .parent()
        .expect("the profile directory sits in the target directory")
        .to_path_buf()
}

/// `cargo build <args>` into the same target dir and profile as this test.
fn cargo_build(args: &[&str]) {
    let profile_dir = profile_dir();
    let profile = match profile_dir.file_name().and_then(|name| name.to_str()) {
        Some("debug") => "dev",
        Some(name) => name,
        None => panic!("unexpected profile directory {}", profile_dir.display()),
    };
    let status = Command::new(env!("CARGO"))
        .arg("build")
        .args(["--profile", profile])
        .arg("--target-dir")
        .arg(target_dir())
        .args(args)
        .status()
        .expect("running cargo");
    assert!(status.success(), "cargo build {args:?} failed: {status}");
}

fn exe(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    assert!(path.is_file(), "{} was not built", path.display());
    path
}

fn build_harness() -> PathBuf {
    cargo_build(&["-p", "gpui_automation", "--example", "harness"]);
    exe(&profile_dir().join("examples"), "harness")
}

fn tests_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// Runs `scenarios` through the `e2e` runner against `harness` in a background run (the
/// runner's default), with `extra` runner options, into `<target>/<out_name>-<pid>`.
fn run_scenarios(harness: &Path, extra: &[&std::ffi::OsStr], out_name: &str, scenarios: &[&str]) {
    // Inside the target dir, so it stays on the build's drive.
    let out = target_dir().join(format!("{out_name}-{}", std::process::id()));
    let status = Command::new(env!("CARGO_BIN_EXE_e2e"))
        .arg("--app")
        .arg(harness)
        .args(extra)
        .arg("--out")
        .arg(&out)
        .args(scenarios.iter().map(|name| tests_dir().join(name)))
        .status()
        .expect("running the e2e runner");
    assert!(
        status.success(),
        "harness scenarios failed: {status}; logs and screenshots in {}",
        out.display()
    );
    if let Err(err) = std::fs::remove_dir_all(&out) {
        eprintln!("could not remove {}: {err}", out.display());
    }
}

#[test]
#[ignore = "opens windows on the real desktop"]
fn harness_scenarios_pass() {
    let harness = build_harness();
    // `harness_background.jsonl` checks what only background mode does.
    run_scenarios(
        &harness,
        &[],
        "e2e-harness",
        &["harness.jsonl", "harness_background.jsonl"],
    );
}

#[test]
#[ignore = "opens windows on the real desktop"]
fn harness_gh_scenario_passes() {
    let harness = build_harness();
    cargo_build(&["-p", "mock_gh", "--bin", "mock_gh"]);
    let mock_gh = exe(&profile_dir(), "mock_gh");
    run_scenarios(
        &harness,
        &["--mock-gh".as_ref(), mock_gh.as_os_str()],
        "e2e-harness-gh",
        &["harness_gh.jsonl"],
    );
}

#[cfg(windows)]
mod pipe {
    use std::fs::{File, OpenOptions};
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};

    /// `ERROR_PIPE_BUSY`: another client holds the pipe.
    const PIPE_BUSY: i32 = 231;

    /// Kills the app if the test fails before it quits.
    struct App(Child);

    impl Drop for App {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

    struct Client {
        writer: File,
        reader: BufReader<File>,
    }

    impl Client {
        /// Connect, waiting for the app to create the pipe or for the
        /// previous client's slot to free up.
        fn connect(path: &str) -> Self {
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                match OpenOptions::new().read(true).write(true).open(path) {
                    Ok(writer) => {
                        let reader = BufReader::new(writer.try_clone().expect("cloning the pipe"));
                        return Self { writer, reader };
                    }
                    Err(err) if Instant::now() < deadline => {
                        assert!(
                            err.kind() == std::io::ErrorKind::NotFound
                                || err.raw_os_error() == Some(PIPE_BUSY),
                            "connecting to {path}: {err}"
                        );
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(err) => panic!("connecting to {path}: {err}"),
                }
            }
        }

        fn read(&mut self) -> Value {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("reading a reply");
            serde_json::from_str(&line).unwrap_or_else(|err| panic!("{line:?}: {err}"))
        }

        /// Send a request and return its result, requiring success.
        fn call(&mut self, request: Value) -> Value {
            let id = request["id"].clone();
            writeln!(self.writer, "{request}").expect("sending a request");
            let reply = self.read();
            assert_eq!(reply["id"], id, "{reply}");
            assert_eq!(reply["ok"], json!(true), "{reply}");
            reply["result"].clone()
        }
    }

    #[test]
    #[ignore = "opens a window on the real desktop"]
    fn a_driver_attaches_detaches_and_reattaches_over_the_named_pipe() {
        let harness = super::build_harness();
        let name = format!("gpui-automation-harness-{}", std::process::id());
        let path = format!(r"\\.\pipe\{name}");
        let child = Command::new(&harness)
            .args(["--automation-pipe", &name, "--window-size", "800x600"])
            .args(["--automation-background", "--counter", "3"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("starting the harness");
        let mut app = App(child);

        let mut first = Client::connect(&path);
        let ready = first.read();
        assert_eq!(ready["event"], "ready", "{ready}");
        assert_eq!(ready["pid"], json!(app.0.id()), "{ready}");
        assert_eq!(ready["background"], json!(true), "{ready}");
        let state = first.call(json!({"id": 1, "cmd": "state"}));
        assert_eq!(state["counter"], json!(3), "{state}");
        first.call(json!({"id": 2, "cmd": "set_counter", "value": 42}));
        let busy = OpenOptions::new().read(true).write(true).open(&path);
        assert_eq!(
            busy.err().and_then(|err| err.raw_os_error()),
            Some(PIPE_BUSY),
            "one client at a time"
        );
        drop(first);

        // The app kept running; a new driver picks up where the last one left.
        let mut second = Client::connect(&path);
        assert_eq!(second.read()["event"], "ready");
        let state = second.call(json!({"id": "again", "cmd": "state"}));
        assert_eq!(state["counter"], json!(42), "{state}");
        second.call(json!({"id": 3, "cmd": "quit"}));

        let deadline = Instant::now() + Duration::from_secs(15);
        let status = loop {
            if let Some(status) = app.0.try_wait().expect("polling the app") {
                break status;
            }
            assert!(Instant::now() < deadline, "the app did not exit after quit");
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(status.success(), "the app exited with {status}");
        let mut stdout = String::new();
        app.0
            .stdout
            .take()
            .expect("stdout was piped")
            .read_to_string(&mut stdout)
            .expect("reading stdout");
        assert_eq!(stdout, "", "pipe mode keeps stdout silent");
    }
}
