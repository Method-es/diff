//! `dv backend --stdio` in-process: the command serves the protocol on its context's streams
//! until stdin ends (docs/PLAN.md §5.1).

use std::io::Write as _;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Buffer, Fixture};
use crate::{Context, Env, Exit};

#[test]
fn it_answers_on_stdout_until_stdin_ends() {
    let fixture = Fixture::new();
    let (stdin, mut requests) = std::io::pipe().unwrap();
    let (stdout, stderr) = (Buffer::default(), Buffer::default());
    let mut ctx = Context {
        args: Vec::new(),
        cwd: fixture.path().to_path_buf(),
        env: Env::empty(),
        stdin: Box::new(stdin),
        stdout: Box::new(stdout.clone()),
        stderr: Box::new(stderr.clone()),
        stdout_is_terminal: false,
        stderr_is_terminal: false,
        stdin_is_terminal: false,
        process_stdio: false,
        git_env: Vec::new(),
    };
    let server =
        std::thread::spawn(move || crate::main_with(["dv", "backend", "--stdio"], &mut ctx));

    let request = json!({"id": 1, "op": "hello", "proto": dv_backend::proto::PROTO});
    requests
        .write_all(format!("{request}\n").as_bytes())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !stdout.text().ends_with('\n') {
        assert!(Instant::now() < deadline, "no answer on stdout");
        std::thread::sleep(Duration::from_millis(10));
    }
    let reply: Value = serde_json::from_str(stdout.text().trim_end()).unwrap();
    assert_eq!(reply["id"], 1);
    assert_eq!(reply["ok"]["proto"], dv_backend::proto::PROTO);
    assert_eq!(reply["ok"]["pid"], std::process::id());

    drop(requests);
    assert_eq!(server.join().unwrap(), Exit::Ok);
    assert_eq!(
        stdout.text().lines().count(),
        1,
        "nothing but the protocol on stdout"
    );
    assert_eq!(stderr.text(), "");
}

#[test]
fn without_stdio_it_is_a_usage_error() {
    let fixture = Fixture::new();
    let ran = fixture.run(&["backend"]);
    assert_eq!(ran.exit, Exit::Usage);
    assert!(
        ran.stderr
            .contains("the app starts it as `dv backend --stdio`"),
        "{}",
        ran.stderr
    );
}
