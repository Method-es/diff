//! `mock_lsp` driven by dv's own LSP client through `LocalBackend::spawn_stdio`, as the app
//! drives it under `DV_LSP_TYPESCRIPT` (docs/PLAN.md §18.7): scripted answers matched by
//! params, delays and cancellation, progress, a crash that a restart doesn't repeat, a stop,
//! and a scenario that doesn't parse.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dv_core::backend::{Backend, LocalBackend, LocalBackendOptions};
use dv_lsp::{
    ClientEvent, CommandSource, HoverPart, LspClient, RequestError, ServerCommand, ServerConfig,
    ServerStatus, StartError, SymbolLocation, TextPosition, TextRange, Timeouts, UriTarget,
};
use dv_test_support::FixtureRepo;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::sync::Mutex;

/// The events a client reported.
type Events = Arc<Mutex<Vec<ClientEvent>>>;

struct Setup {
    repo: FixtureRepo,
    backend: Arc<dyn Backend>,
    dir: tempfile::TempDir,
}

impl Setup {
    fn new(scenario: Value) -> Self {
        let setup = Self::bare();
        setup.write_scenario(&scenario);
        setup
    }

    /// The fixture without a scenario yet.
    fn bare() -> Self {
        let repo = FixtureRepo::new();
        repo.write("tsconfig.json", "{}");
        repo.write(
            "src/a.ts",
            "export const answer = 42;\nconsole.log(answer);\n",
        );
        repo.commit("a");
        let backend: Arc<dyn Backend> = Arc::new(
            LocalBackend::open_with(
                repo.path(),
                LocalBackendOptions {
                    env: repo.env_overlay(),
                },
            )
            .unwrap(),
        );
        let dir = tempfile::tempdir().unwrap();
        Self { repo, backend, dir }
    }

    fn write_scenario(&self, scenario: &Value) {
        std::fs::write(self.dir.path().join("scenario.json"), scenario.to_string()).unwrap();
    }

    /// The URI the server gets for a worktree path.
    fn uri(&self, rel: &str) -> String {
        dv_lsp::WorkspacePaths::new(&self.backend.repo().toplevel).uri_text(rel)
    }

    fn log_path(&self) -> PathBuf {
        self.dir.path().join("log.jsonl")
    }

    fn command(&self) -> ServerCommand {
        let path = |p: &Path| p.to_str().unwrap().to_string();
        ServerCommand {
            spec_id: "typescript",
            program: env!("CARGO_BIN_EXE_mock_lsp").into(),
            args: vec![
                "--lsp".into(),
                "--stdio".into(),
                "--scenario".into(),
                path(&self.dir.path().join("scenario.json")),
                "--log".into(),
                path(&self.log_path()),
            ],
            root: String::new(),
            source: CommandSource::Override("DV_LSP_TYPESCRIPT".into()),
            env: Vec::new(),
        }
    }

    fn start(&self) -> Result<(LspClient, Events), StartError> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let client = LspClient::start(
            self.backend.clone(),
            &self.command(),
            ServerConfig::default(),
            Arc::new(move |event| log.lock().unwrap().push(event)),
            Timeouts {
                hover: Duration::from_secs(3),
                ..Timeouts::default()
            },
        )?;
        Ok((client, seen))
    }

    fn open(&self, client: &LspClient) {
        let text = std::fs::read_to_string(self.repo.path().join("src/a.ts")).unwrap();
        client
            .open_document("src/a.ts", "typescript", text)
            .unwrap();
    }

    /// The log's entries.
    fn log(&self) -> Vec<Value> {
        std::fs::read_to_string(self.log_path())
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// Waits until the log has an entry passing `test`.
    fn wait_for_log(&self, what: &str, test: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(entry) = self.log().into_iter().find(|entry| test(entry)) {
                return entry;
            }
            assert!(
                Instant::now() < deadline,
                "mock_lsp never logged {what}: {:#?}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn received(entry: &Value, method: &str) -> bool {
    entry["received"]["method"] == method
}

#[test]
fn hover_definition_and_references_from_the_scenario() {
    let setup = Setup::bare();
    let uri = setup.uri("src/a.ts");
    setup.write_scenario(&json!({
        "requests": [
            {"method": "textDocument/hover",
             "params": {"textDocument": {"uri": "*/src/a.ts"}, "position": {"line": 1}},
             "result": {"contents": {"kind": "markdown", "value": "```typescript\nconst answer: 42\n```"},
                        "range": {"start": {"line": 1, "character": 12}, "end": {"line": 1, "character": 18}}},
             "delay_ms": 50},
            {"method": "textDocument/definition",
             "result": [{"uri": "${root}/src/a.ts",
                         "range": {"start": {"line": 0, "character": 13}, "end": {"line": 0, "character": 19}}}]},
            {"method": "textDocument/references",
             "params": {"context": {"includeDeclaration": false}},
             "result": []}
        ]
    }));
    let (client, _) = setup.start().unwrap();
    setup.open(&client);
    let hover = client
        .hover("src/a.ts", TextPosition::new(1, 14))
        .wait()
        .unwrap()
        .unwrap();
    assert_eq!(
        hover.parts,
        [HoverPart::Markdown(
            "```typescript\nconst answer: 42\n```".into()
        )]
    );
    // A hover on another line matches nothing: `null`.
    assert_eq!(
        client
            .hover("src/a.ts", TextPosition::new(0, 14))
            .wait()
            .unwrap(),
        None
    );
    assert_eq!(
        client
            .definition("src/a.ts", TextPosition::new(1, 14))
            .wait()
            .unwrap(),
        [SymbolLocation {
            target: UriTarget::Worktree("src/a.ts".into()),
            range: TextRange {
                start: TextPosition::new(0, 13),
                end: TextPosition::new(0, 19)
            }
        }]
    );
    assert_eq!(
        client
            .references("src/a.ts", TextPosition::new(1, 14), false)
            .wait()
            .unwrap(),
        []
    );
    let log = setup.log();
    let matched: Vec<(String, Value)> = log
        .iter()
        .filter(|entry| entry.get("matched").is_some())
        .map(|entry| {
            (
                entry["received"]["method"].as_str().unwrap().to_string(),
                entry["matched"].clone(),
            )
        })
        .collect();
    assert_eq!(
        matched,
        [
            ("textDocument/hover".to_string(), json!(0)),
            ("textDocument/hover".into(), Value::Null),
            ("textDocument/definition".into(), json!(1)),
            ("textDocument/references".into(), json!(2)),
        ]
    );
    let open = log
        .iter()
        .find(|entry| received(entry, "textDocument/didOpen"))
        .unwrap();
    assert_eq!(
        open["received"]["params"]["textDocument"]["uri"],
        json!(uri)
    );
    // A clean stop: `shutdown` and `exit` without params, so the mock (strict like `tsc`)
    // exits 0 at once, nothing killed.
    let started = Instant::now();
    assert_eq!(client.stop(), ServerStatus::Stopped(Some(0)));
    assert!(started.elapsed() < Duration::from_secs(1));
    for method in ["shutdown", "exit"] {
        let entry = setup.wait_for_log(method, |entry| received(entry, method));
        assert!(entry["received"].get("params").is_none(), "{entry}");
    }
}

#[test]
fn shutdown_and_exit_with_params_are_refused_like_tsc() {
    let setup = Setup::new(json!({}));
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_mock_lsp"))
        .args(["--log", setup.log_path().to_str().unwrap()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut send = |message: Value| {
        use std::io::Write as _;
        let body = message.to_string();
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        stdin.flush().unwrap();
    };
    let mut answer = || -> Value {
        use std::io::{BufRead as _, Read as _};
        let mut length = 0;
        loop {
            let mut line = String::new();
            stdout.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length: ") {
                length = value.parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        stdout.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    };
    send(json!({"jsonrpc": "2.0", "id": 1, "method": "shutdown", "params": null}));
    assert_eq!(answer()["error"]["code"], -32602);
    send(json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}));
    assert_eq!(answer()["result"], Value::Null);
    // Ignored: it still answers afterwards.
    send(json!({"jsonrpc": "2.0", "method": "exit", "params": {}}));
    send(json!({"jsonrpc": "2.0", "id": 3, "method": "textDocument/hover", "params": {}}));
    assert_eq!(answer()["id"], 3);
    send(json!({"jsonrpc": "2.0", "method": "exit"}));
    assert_eq!(child.wait().unwrap().code(), Some(0));
}

#[test]
fn a_slow_initialize_and_a_crash_at_start() {
    let setup = Setup::new(json!({
        "initialize_delay_ms": 300,
        "crash": {"after_requests": 0, "exit": 7, "times": 1}
    }));
    match setup.start() {
        Err(StartError::Exited { code, .. }) => assert_eq!(code, Some(7)),
        other => panic!("expected a crash at start, got {:?}", other.map(|_| ())),
    }
    // The restart doesn't crash again; its initialize takes the delay.
    let started = Instant::now();
    let (client, _) = setup.start().unwrap();
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert_eq!(client.status(), ServerStatus::Ready);
}

#[test]
fn a_dropped_hover_is_cancelled_and_the_server_says_so() {
    let setup = Setup::new(json!({
        "requests": [{"method": "textDocument/hover", "result": {"contents": "slow"}, "delay_ms": 1500}]
    }));
    let (client, _) = setup.start().unwrap();
    setup.open(&client);
    let pending = client.hover("src/a.ts", TextPosition::new(0, 0));
    let id = pending.id().unwrap();
    setup.wait_for_log("the hover", |entry| received(entry, "textDocument/hover"));
    drop(pending);
    let cancel = setup.wait_for_log("the cancel", |entry| received(entry, "$/cancelRequest"));
    assert_eq!(cancel["received"]["params"]["id"], json!(id));
}

#[test]
fn progress_from_on_initialized() {
    let setup = Setup::new(json!({
        "on_initialized": [
            {"request": {"method": "window/workDoneProgress/create", "params": {"token": "load"}}},
            {"notify": {"method": "$/progress", "params": {"token": "load",
                "value": {"kind": "begin", "title": "Loading project", "percentage": 10}}}},
            {"delay_ms": 300},
            {"notify": {"method": "$/progress", "params": {"token": "load", "value": {"kind": "end"}}}}
        ]
    }));
    let (client, seen) = setup.start().unwrap();
    // The client answered the server's own request.
    let answer = setup.wait_for_log("the answer to create", |entry| {
        entry["received"]["id"] == "mock-1" && entry["received"].get("method").is_none()
    });
    assert_eq!(answer["received"]["result"], Value::Null);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let progress: Vec<ClientEvent> = seen
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, ClientEvent::Progress(_)))
            .cloned()
            .collect();
        if progress.len() == 2 {
            let ClientEvent::Progress(first) = &progress[0] else {
                unreachable!()
            };
            assert_eq!(first[0].title, "Loading project");
            assert_eq!(first[0].percentage, Some(10));
            assert_eq!(progress[1], ClientEvent::Progress(vec![]));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no progress: {:#?}",
            seen.lock().unwrap()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(client.progress().is_empty());
}

#[test]
fn a_crash_that_the_restart_doesnt_repeat() {
    let setup = Setup::new(json!({
        "stderr": ["mock_lsp: up"],
        "requests": [{"method": "textDocument/hover", "result": {"contents": "after the restart"}}],
        "crash": {"after_requests": 1, "exit": 3, "times": 1}
    }));
    let (client, _) = setup.start().unwrap();
    setup.open(&client);
    assert_eq!(
        client
            .hover("src/a.ts", TextPosition::new(0, 0))
            .wait()
            .unwrap_err(),
        RequestError::Exited
    );
    assert_eq!(client.status(), ServerStatus::Exited(Some(3)));
    assert!(client.stderr_tail().contains(&"mock_lsp: up".to_string()));

    let (again, _) = setup.start().unwrap();
    setup.open(&again);
    let hover = again
        .hover("src/a.ts", TextPosition::new(0, 0))
        .wait()
        .unwrap()
        .unwrap();
    assert_eq!(
        hover.parts,
        [HoverPart::Markdown("after the restart".into())]
    );
    let starts = setup
        .log()
        .iter()
        .filter(|entry| entry.get("started").is_some())
        .count();
    assert_eq!(starts, 2);
}

#[test]
fn a_scenario_that_doesnt_parse() {
    let setup = Setup::new(json!({"requests": [{"methd": "x"}]}));
    match setup.start() {
        Err(StartError::Exited { code, stderr }) => {
            assert_eq!(code, Some(65));
            assert!(
                stderr.iter().any(|line| line.contains("unknown field")),
                "{stderr:?}"
            );
        }
        other => panic!("expected the server to end, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn a_utf16_servers_references_convert_against_every_file_read() {
    let setup = Setup::bare();
    // Twelve files, each with a two-byte character before the symbol: UTF-16 column 26 is
    // byte 27.
    let line = "const é = 1; export const target = é;
";
    for n in 0..12 {
        setup.repo.write(format!("src/u{n}.ts"), line);
    }
    let result: Vec<Value> = (0..12)
        .map(|n| {
            json!({"uri": format!("${{root}}/src/u{n}.ts"),
                   "range": {"start": {"line": 0, "character": 26},
                             "end": {"line": 0, "character": 32}}})
        })
        .collect();
    setup.write_scenario(&json!({
        "capabilities": {"positionEncoding": "utf-16", "referencesProvider": true},
        "requests": [{"method": "textDocument/references", "result": result}]
    }));
    let (client, _) = setup.start().unwrap();
    setup.open(&client);
    let found = client
        .references("src/a.ts", TextPosition::new(1, 14), true)
        .wait()
        .unwrap();
    assert_eq!(found.len(), 12);
    for location in found {
        assert_eq!(
            location.range,
            TextRange {
                start: TextPosition::new(0, 27),
                end: TextPosition::new(0, 33)
            },
            "{:?}",
            location.target
        );
    }
    client.stop();
}
