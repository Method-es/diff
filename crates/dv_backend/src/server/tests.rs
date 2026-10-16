//! The server spoken to in raw lines over in-memory pipes.

use std::io::PipeWriter;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::Duration;

use dv_core::backend::WatchEvent;
use dv_test_support::FixtureRepo;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::*;

const ANSWER_WITHIN: Duration = Duration::from_secs(20);

/// A server on a thread, and the client's ends of its pipes.
struct Raw {
    input: Option<PipeWriter>,
    lines: Receiver<Value>,
    server: Option<JoinHandle<io::Result<()>>>,
}

impl Raw {
    fn start() -> Self {
        let (server_in, input) = io::pipe().unwrap();
        let (output, server_out) = io::pipe().unwrap();
        let server = std::thread::spawn(move || serve(server_in, server_out));
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let line = line.unwrap();
                let value = serde_json::from_str(&line)
                    .unwrap_or_else(|error| panic!("not JSON ({error}): {line}"));
                if tx.send(value).is_err() {
                    return;
                }
            }
        });
        Self {
            input: Some(input),
            lines,
            server: Some(server),
        }
    }

    fn send(&mut self, line: &str) {
        let input = self.input.as_mut().unwrap();
        input.write_all(line.as_bytes()).unwrap();
        input.write_all(b"\n").unwrap();
    }

    fn recv(&self) -> Value {
        self.lines
            .recv_timeout(ANSWER_WITHIN)
            .expect("the server answers")
    }

    /// Sends `request` and returns the reply with its id (skipping watch batches).
    fn call(&mut self, request: Value) -> Value {
        let id = request["id"].clone();
        self.send(&request.to_string());
        loop {
            let line = self.recv();
            if line["id"] == id {
                return line;
            }
            assert!(line.get("watch").is_some(), "unexpected line {line}");
        }
    }

    /// Closes the server's input and waits for `serve` to return.
    fn finish(mut self) -> io::Result<()> {
        drop(self.input.take());
        self.server.take().unwrap().join().unwrap()
    }
}

#[test]
fn hello_says_who_the_server_is() {
    let mut raw = Raw::start();
    let reply = raw.call(json!({"id": 1, "op": "hello", "proto": PROTO}));
    assert_eq!(reply["ok"]["proto"], PROTO);
    assert_eq!(
        reply["ok"]["build_ok"],
        Value::Null,
        "no build sent, none checked"
    );
    assert_eq!(reply["ok"]["pid"], std::process::id());
    assert_eq!(reply["ok"]["os"], std::env::consts::OS);
    assert_eq!(reply["ok"]["arch"], std::env::consts::ARCH);
    assert_eq!(reply["ok"]["version"], env!("CARGO_PKG_VERSION"));

    // The server is this test executable: its own hash matches, any other doesn't.
    let own = crate::build::own_hash().unwrap();
    let reply = raw.call(json!({"id": 2, "op": "hello", "proto": PROTO, "build": own}));
    assert_eq!(reply["ok"]["build_ok"], true);
    let reply = raw.call(json!({
        "id": 3, "op": "hello", "proto": PROTO, "build": own.to_uppercase(),
    }));
    assert_eq!(reply["ok"]["build_ok"], true, "hex case doesn't matter");
    let reply = raw.call(json!({"id": 4, "op": "hello", "proto": PROTO, "build": "0".repeat(64)}));
    assert_eq!(reply["ok"]["build_ok"], false);
    raw.finish().unwrap();
}

#[test]
fn a_line_that_isnt_a_request_is_answered_and_serving_goes_on() {
    let mut raw = Raw::start();
    raw.send("not json");
    let reply = raw.recv();
    assert_eq!(reply["id"], 0);
    assert_eq!(reply["err"]["kind"], "protocol");
    assert!(
        reply["err"]["message"]
            .as_str()
            .unwrap()
            .starts_with("not a request"),
        "{reply}"
    );

    // An id that can be read is answered under it.
    let reply = raw.call(json!({"id": 5, "op": "frobnicate"}));
    assert_eq!(reply["err"]["kind"], "protocol", "{reply}");
    let reply = raw.call(json!({"id": 6, "op": "close"}));
    assert_eq!(
        reply["err"]["kind"], "protocol",
        "a missing argument: {reply}"
    );

    // Blank lines and CRLF endings are tolerated.
    raw.send("");
    raw.send("   ");
    raw.send(&format!(
        "{}\r",
        json!({"id": 7, "op": "hello", "proto": PROTO})
    ));
    let reply = raw.recv();
    assert_eq!(reply["id"], 7);
    assert_eq!(reply["ok"]["proto"], PROTO);
    raw.finish().unwrap();
}

#[test]
fn repositories_are_named_by_handle() {
    let (repo, env) = committed();
    let mut raw = Raw::start();

    let reply = raw.call(json!({"id": 1, "op": "open", "path": repo.path(), "env": env}));
    let handle = reply["ok"]["repo"].as_u64().expect("a handle");
    assert_eq!(
        reply["ok"]["info"]["object_format"], "sha1",
        "the RepoInfo: {reply}"
    );
    let reply = raw.call(json!({
        "id": 2, "op": "git", "repo": handle, "args": ["ls-files"],
    }));
    assert_eq!(reply["ok"]["code"], 0);
    assert_eq!(reply["ok"]["stdout"], "YS50eHQK", "base64 of `a.txt\\n`");
    let reply = raw.call(json!({"id": 3, "op": "worktree_read", "repo": handle, "rel": "a.txt"}));
    assert_eq!(reply["ok"], "YQo=");
    let reply = raw.call(json!({"id": 4, "op": "worktree_read", "repo": handle, "rel": "no"}));
    assert_eq!(reply, json!({"id": 4, "ok": null}));

    // Errors carry their variant.
    let reply = raw.call(json!({"id": 5, "op": "worktree_read", "repo": handle, "rel": "../x"}));
    assert_eq!(reply["err"]["kind"], "invalid_path", "{reply}");
    let missing = repo.path().join("missing");
    let reply = raw.call(json!({"id": 6, "op": "open", "path": missing, "env": env}));
    assert_eq!(reply["err"]["kind"], "path_not_found", "{reply}");

    // Closed, the handle is gone; closing again is fine.
    let reply = raw.call(json!({"id": 7, "op": "close", "repo": handle}));
    assert_eq!(reply, json!({"id": 7, "ok": null}));
    let reply = raw.call(json!({"id": 8, "op": "store_list", "repo": handle, "dir": ""}));
    assert_eq!(reply["err"]["kind"], "protocol", "{reply}");
    let reply = raw.call(json!({"id": 9, "op": "close", "repo": handle}));
    assert_eq!(reply, json!({"id": 9, "ok": null}));
    let reply = raw.call(json!({"id": 10, "op": "unwatch", "watch": 42}));
    assert_eq!(reply, json!({"id": 10, "ok": null}), "an unknown watch");
    raw.finish().unwrap();
}

#[test]
fn calls_run_concurrently_and_answer_in_the_order_they_finish() {
    let (repo, env) = committed();
    let mut raw = Raw::start();
    let reply = raw.call(json!({"id": 1, "op": "open", "path": repo.path(), "env": env}));
    let handle = reply["ok"]["repo"].clone();

    raw.send(
        &json!({
            "id": 2, "op": "git", "repo": handle, "args": ["-c", "alias.slow=!sleep 2", "slow"],
        })
        .to_string(),
    );
    raw.send(&json!({"id": 3, "op": "worktree_len", "repo": handle, "rel": "a.txt"}).to_string());
    let first = raw.recv();
    assert_eq!(first, json!({"id": 3, "ok": 2}), "the quick call overtakes");
    let second = raw.recv();
    assert_eq!(second["id"], 2);
    assert_eq!(second["ok"]["code"], 0);
    raw.finish().unwrap();
}

#[test]
fn the_end_of_the_input_ends_the_server_whatever_still_runs() {
    let (repo, env) = committed();
    let mut raw = Raw::start();
    let reply = raw.call(json!({"id": 1, "op": "open", "path": repo.path(), "env": env}));
    let handle = reply["ok"]["repo"].clone();
    let reply = raw.call(json!({
        "id": 2, "op": "watch", "repo": handle,
        "spec": {"worktree": true, "store": true, "git": true},
    }));
    assert!(reply["ok"]["watch"].as_u64().is_some(), "{reply}");
    raw.send(
        &json!({
            "id": 3, "op": "git", "repo": handle, "args": ["-c", "alias.slow=!sleep 5", "slow"],
        })
        .to_string(),
    );
    let started = std::time::Instant::now();
    raw.finish().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "serve returned without waiting for git: {:?}",
        started.elapsed()
    );
}

#[test]
fn a_watch_holds_its_batches_until_its_answer_is_queued() {
    let (tx, rx) = mpsc::channel();
    let gate = Gate::default();
    let event = |path: &str| WatchEvent {
        worktree_paths: vec![path.into()],
        ..WatchEvent::default()
    };
    gate.deliver(proto::watch_line(3, &event("early")), None, &tx);
    assert!(rx.try_recv().is_err(), "held while closed");
    tx.send(Out::Line(b"the answer\n".to_vec(), None)).unwrap();
    gate.open(&tx);
    gate.deliver(proto::watch_line(3, &event("late")), None, &tx);
    let lines: Vec<String> = rx
        .try_iter()
        .map(|out| match out {
            Out::Line(line, _) => String::from_utf8(line).unwrap(),
            Out::Stop(_) => panic!("no stop"),
        })
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "the answer\n");
    assert!(lines[1].contains("\"early\"") && lines[1].contains("\"watch\":3"));
    assert!(lines[2].contains("\"late\""));
}

#[test]
fn lines_are_trimmed_of_their_ending() {
    assert_eq!(trim_line(b"{}\n"), b"{}");
    assert_eq!(trim_line(b"{}\r\n"), b"{}");
    assert_eq!(trim_line(b"{}"), b"{}");
    assert_eq!(trim_line(b"  \r\n"), b"");
    assert_eq!(request_id(br#"{"id": 12, "op": 5}"#), 12);
    assert_eq!(request_id(br#"{"id": "x"}"#), 0);
    assert_eq!(request_id(b"garbage"), 0);
}

/// A repo with one commit, and its fixture environment as `open` sends it.
fn committed() -> (FixtureRepo, Vec<(String, Option<String>)>) {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    let text = |text: std::ffi::OsString| text.into_string().unwrap();
    let env = repo
        .env_overlay()
        .into_iter()
        .map(|(name, value)| (text(name), value.map(text)))
        .collect();
    (repo, env)
}

#[test]
fn a_processs_lines_wait_within_its_budget_until_written() {
    use self::process::{BUDGET, Budget};

    let budget = Arc::new(Budget::default());
    // A line bigger than the budget still goes when nothing waits.
    let big = budget.take(BUDGET * 2).expect("open");
    assert_eq!(budget.queued(), BUDGET * 2);
    let (tx, rx) = mpsc::channel();
    let waiter = std::thread::spawn({
        let budget = Arc::clone(&budget);
        move || {
            let credit = budget.take(10);
            tx.send(credit.is_some()).unwrap();
            credit
        }
    });
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "waits while the budget is spent"
    );
    drop(big);
    assert_eq!(rx.recv_timeout(ANSWER_WITHIN), Ok(true));
    let small = waiter.join().unwrap();
    assert_eq!(budget.queued(), 10);

    // The writer gives the bytes back once it has written the line.
    let (out, lines) = mpsc::channel();
    out.send(Out::Line(b"x\n".to_vec(), small)).unwrap();
    out.send(Out::Stop(mpsc::channel().0)).unwrap();
    let mut written = Vec::new();
    write_lines(&mut written, lines);
    assert_eq!(written, b"x\n");
    assert_eq!(budget.queued(), 0);
}
