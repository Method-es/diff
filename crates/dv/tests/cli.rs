//! The built `dv` as a terminal or an agent meets it (docs/PLAN.md §10, §13.1): one binary
//! whose headless commands print and exit without any window, with the documented exit codes,
//! stdout and stderr.
//!
//! Only headless commands (and `--help`, `--version`) are run here: `dv` alone or `dv open`
//! would start the app on the owner's desktop. Every child gets the fixture's git environment
//! and none of this process's `CLAUDECODE`, `DV_AUTHOR` or `DV_REVIEW` (Claude Code exports
//! `CLAUDECODE` to the commands it runs).

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use dv_test_support::FixtureRepo;
use pretty_assertions::assert_eq;
use serde_json::Value;

/// `dv <args>` in `cwd`, with the fixture's git environment and `env` on top.
fn dv(repo: &FixtureRepo, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    command(repo, cwd, args, env)
        .stdin(Stdio::null())
        .output()
        .expect("dv runs")
}

/// `dv <args>` in the work tree with `stdin` written to it through a real pipe.
fn dv_piped(repo: &FixtureRepo, args: &[&str], stdin: &str) -> Output {
    let mut child = command(repo, repo.path(), args, &[])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("dv starts");
    let mut pipe = child.stdin.take().unwrap();
    pipe.write_all(stdin.as_bytes()).unwrap();
    drop(pipe);
    child.wait_with_output().expect("dv runs")
}

fn command(repo: &FixtureRepo, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_dv"));
    command
        .args(args)
        .current_dir(cwd)
        .env_remove("CLAUDECODE")
        .env_remove("DV_AUTHOR")
        .env_remove("DV_REVIEW")
        .env_remove("RUST_LOG");
    for (name, value) in repo.env_overlay() {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    command.envs(env.iter().copied());
    command
}

fn fixture() -> FixtureRepo {
    let repo = FixtureRepo::new();
    repo.write("src/app.ts", "export const a = 1;\n");
    repo.commit("base");
    repo.write("src/app.ts", "export const a = 2;\n");
    repo
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {:?}", stdout(output)))
}

#[test]
fn help_lists_every_command_and_the_version_prints() {
    let repo = fixture();
    let help = dv(&repo, repo.path(), &["--help"], &[]);
    assert_eq!(help.status.code(), Some(0));
    let text = stdout(&help);
    assert!(text.contains("Usage: dv [OPTIONS] [COMMAND]"), "{text}");
    for command in [
        "open", "status", "review", "comment", "setup", "doctor", "backend",
    ] {
        assert!(
            text.lines()
                .any(|line| line.trim_start().starts_with(&format!("{command} "))),
            "`{command}` in dv --help:\n{text}"
        );
    }
    assert_eq!(stderr(&help), "");

    let version = dv(&repo, repo.path(), &["--version"], &[]);
    assert_eq!(version.status.code(), Some(0));
    assert_eq!(
        stdout(&version),
        format!("dv {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn a_headless_command_prints_and_exits_0() {
    let repo = fixture();
    let output = dv(&repo, repo.path(), &["status", "--json"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let status = json(&output);
    assert_eq!(status["schema"], 1);
    assert_eq!(status["author"]["kind"], "reviewer");
    assert_eq!(status["author"]["name"], "dv fixture");
    assert_eq!(status["current"], Value::Null);
    assert_eq!(stderr(&output), "", "nothing logged on a success");

    // The process's environment reaches the command: Claude Code's variable makes it an agent.
    let output = dv(&repo, repo.path(), &["status"], &[("CLAUDECODE", "1")]);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        stdout(&output).contains("acting as: agent claude-code"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn errors_go_to_stderr_and_set_the_exit_code() {
    let repo = fixture();

    // No review to show: exit 2, the error on stderr, and with --json on stdout too.
    let output = dv(&repo, repo.path(), &["review", "show", "--json"], &[]);
    assert_eq!(output.status.code(), Some(2));
    let error = json(&output);
    assert_eq!(error["schema"], 1);
    assert_eq!(error["error"]["code"], "no_review");
    assert!(stderr(&output).starts_with("dv: no open review in this repository"));

    let output = dv(&repo, repo.path(), &["review", "show"], &[]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stdout(&output), "");

    // clap's own errors honour --json too.
    let output = dv(
        &repo,
        repo.path(),
        &["review", "list", "--bogus", "--json"],
        &[],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json(&output)["error"]["code"], "usage");
    assert!(stderr(&output).contains("--bogus"));

    // A WSL repository: exit 4, nothing started.
    let output = dv(
        &repo,
        repo.path(),
        &["status", "--repo", r"\\wsl.localhost\Ubuntu\home\kyle\api"],
        &[],
    );
    assert_eq!(output.status.code(), Some(4));
    assert!(stderr(&output).starts_with("dv: Ubuntu:/home/kyle/api is inside WSL"));

    // Outside a repository: exit 1.
    let outside = repo.outside_dir("plain");
    let output = dv(&repo, &outside, &["comment", "list"], &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).starts_with("dv: not a git repository"));

    // A later version's command: exit 2 with what to do.
    let output = dv(&repo, repo.path(), &["doctor"], &[]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        stderr(&output),
        "dv: `dv doctor` arrives in a later version\n"
    );
}

#[test]
fn an_agent_answers_through_the_binary_with_a_body_on_stdin() {
    let repo = fixture();
    // The reviewer starts a review and comments; then requests changes.
    let output = dv(&repo, repo.path(), &["review", "new", "--json"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let review = json(&output)["review"]["id"].as_str().unwrap().to_string();
    let output = dv(
        &repo,
        repo.path(),
        &[
            "comment",
            "add",
            "--file",
            "src/app.ts",
            "--lines",
            "1",
            "--body",
            "Why 2?",
            "--json",
        ],
        &[],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let thread = json(&output)["thread"]["id"].as_str().unwrap().to_string();
    let output = dv(
        &repo,
        repo.path(),
        &["review", "submit", "--verdict", "request-changes"],
        &[],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    // The agent's reply comes over a real pipe, quotes and all.
    let body = "It's \"2\" now: `a = 2`.\n\nSee $HOME and `$(rm -rf /)` untouched.\n";
    let output = dv_piped(
        &repo,
        &[
            "comment",
            "reply",
            &thread,
            "--body-file",
            "-",
            "--author",
            "claude-code",
            "--json",
        ],
        body,
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let reply = json(&output);
    assert_eq!(reply["reply"]["body"], body.trim_end());
    assert_eq!(reply["reply"]["author"]["name"], "claude-code");

    // `done` needs an agent; Claude Code's variable is one.
    let output = dv(&repo, repo.path(), &["review", "done"], &[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).starts_with("dv: `dv review done` is an agent's handoff"));
    let output = dv(
        &repo,
        repo.path(),
        &["review", "done", "--note", "Fixed."],
        &[("CLAUDECODE", "1")],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    // The reviewer hasn't answered: a short wait times out with exit 3.
    let output = dv(
        &repo,
        repo.path(),
        &["review", "wait", &review, "--timeout", "0.3", "--json"],
        &[],
    );
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert_eq!(stdout(&output), "{\"schema\":1,\"outcome\":\"timeout\"}\n");
}

#[test]
fn a_snapshot_cache_that_cant_be_written_logs_nothing_on_a_success() {
    let repo = fixture();
    let output = dv(&repo, repo.path(), &["review", "new", "--json"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let review = json(&output)["review"]["id"].as_str().unwrap().to_string();
    // A directory stands where the cache goes, so every load's write of it fails, as a replace
    // does on Windows while another reader (a waiting agent, the app) holds the file.
    let cache = repo
        .path()
        .join(".git/dv/reviews")
        .join(format!("{review}.state.json"));
    if cache.exists() {
        std::fs::remove_file(&cache).unwrap();
    }
    std::fs::create_dir_all(&cache).unwrap();
    for args in [
        &["review", "show", review.as_str(), "--json"][..],
        &["status", "--json"],
        &["comment", "list", "--json"],
    ] {
        let output = dv(&repo, repo.path(), args, &[]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            stderr(&output)
        );
        assert_eq!(json(&output)["schema"], 1);
        assert_eq!(
            stderr(&output),
            "",
            "{args:?}: the cache is only an optimisation"
        );
    }
}
