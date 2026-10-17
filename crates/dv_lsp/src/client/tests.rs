//! The client against an in-process fake server (docs/PLAN.md §18.7): framing, requests and
//! their cancellation, the encoding conversion, the server's own requests, progress, the
//! server's end and the client's stop.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use dv_core::backend::{
    BackendError, LocalBackend, LocalBackendOptions, ProcessControl, ProcessEvent, ProcessHandle,
    ProcessSink,
};
use dv_test_support::FixtureRepo;
use parking_lot::Mutex;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::*;
use crate::framing::{Decoder, encode};

/// What the fake does with each message the client sends.
type Handler = Box<dyn FnMut(&Value, &mut Out) + Send>;

/// The fake server's side of the pipes: what it sends the client.
struct Out {
    sink: ProcessSink,
    /// Each frame is split into chunks of this many bytes (0: one chunk per frame).
    chunk: usize,
    ended: bool,
}

impl Out {
    fn send(&mut self, message: Value) {
        let frame = encode(&serde_json::to_vec(&message).unwrap());
        self.raw(&frame);
    }

    fn raw(&mut self, bytes: &[u8]) {
        if self.ended {
            return;
        }
        if self.chunk == 0 {
            (self.sink)(ProcessEvent::Stdout(bytes.to_vec()));
        } else {
            for piece in bytes.chunks(self.chunk) {
                (self.sink)(ProcessEvent::Stdout(piece.to_vec()));
            }
        }
    }

    fn reply(&mut self, request: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": request["id"], "result": result}));
    }

    fn stderr(&mut self, line: &str) {
        (self.sink)(ProcessEvent::Stderr(line.to_string()));
    }

    fn exit(&mut self, code: Option<i32>) {
        if !self.ended {
            self.ended = true;
            (self.sink)(ProcessEvent::Exit(code));
        }
    }
}

/// What the fake's thread is told.
enum Input {
    Bytes(Vec<u8>),
    Kill,
}

struct FakeControl {
    input: Sender<Input>,
    ended: Arc<Mutex<bool>>,
}

impl ProcessControl for FakeControl {
    fn write(&self, bytes: &[u8]) -> Result<(), BackendError> {
        if *self.ended.lock() || self.input.send(Input::Bytes(bytes.to_vec())).is_err() {
            return Err(dv_core::backend::process::ended("fake"));
        }
        Ok(())
    }

    fn kill(&self) {
        let _ = self.input.send(Input::Kill);
    }
}

/// A fake server: every message the client sent, in order.
#[derive(Clone, Default)]
struct Received(Arc<Mutex<Vec<Value>>>);

impl Received {
    fn all(&self) -> Vec<Value> {
        self.0.lock().clone()
    }

    fn methods(&self) -> Vec<String> {
        self.all()
            .iter()
            .filter_map(|message| message["method"].as_str().map(str::to_string))
            .collect()
    }

    fn with_method(&self, method: &str) -> Vec<Value> {
        self.all()
            .into_iter()
            .filter(|message| message["method"] == method)
            .collect()
    }

    /// Waits until `count` messages of `method` have arrived.
    fn wait_for_count(&self, method: &str, count: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while self.with_method(method).len() < count {
            assert!(
                std::time::Instant::now() < deadline,
                "the fake never got {count} {method}; it got {:#?}",
                self.all()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Waits until a message matching `test` has arrived.
    fn wait_for(&self, what: &str, test: impl Fn(&Value) -> bool) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(found) = self.all().into_iter().find(|message| test(message)) {
                return found;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the fake never got {what}; it got {:#?}",
                self.all()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// A spawn function for [`LspClient::start_with`] running `handler` on a thread of its own.
fn fake(
    chunk: usize,
    mut handler: impl FnMut(&Value, &mut Out) + Send + 'static,
) -> (
    impl FnOnce(ProcessSink) -> Result<ProcessHandle, BackendError>,
    Received,
) {
    let received = Received::default();
    let log = received.clone();
    let spawn = move |sink: ProcessSink| {
        let (input, inputs): (Sender<Input>, Receiver<Input>) = mpsc::channel();
        let ended = Arc::new(Mutex::new(false));
        let thread_ended = ended.clone();
        std::thread::spawn(move || {
            let mut out = Out {
                sink,
                chunk,
                ended: false,
            };
            let mut decoder = Decoder::new();
            for input in inputs {
                match input {
                    Input::Bytes(bytes) => {
                        decoder.push(&bytes);
                        while let Some(Ok(body)) = decoder.next_message() {
                            let message: Value = serde_json::from_slice(&body).unwrap();
                            log.0.lock().push(message.clone());
                            handler(&message, &mut out);
                        }
                    }
                    Input::Kill => out.exit(None),
                }
                if out.ended {
                    *thread_ended.lock() = true;
                    break;
                }
            }
        });
        Ok(ProcessHandle::new(4242, FakeControl { input, ended }))
    };
    (spawn, received)
}

/// Answers `initialize` with `capabilities`, `shutdown` with null, and exits on `exit`; hands
/// everything else to `rest`.
fn standard(
    capabilities: Value,
    mut rest: impl FnMut(&Value, &mut Out) + Send + 'static,
) -> Handler {
    Box::new(move |message, out| match message["method"].as_str() {
        Some("initialize") => out.reply(message, json!({"capabilities": capabilities})),
        Some("shutdown") => out.reply(message, Value::Null),
        Some("exit") => out.exit(Some(0)),
        _ => rest(message, out),
    })
}

fn utf8() -> Value {
    json!({"positionEncoding": "utf-8", "hoverProvider": true})
}

const ROOT: &str = "/w/repo";

fn events() -> (EventSink, Arc<Mutex<Vec<ClientEvent>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    (Arc::new(move |event| log.lock().push(event)), seen)
}

fn quick() -> Timeouts {
    Timeouts {
        initialize: Duration::from_secs(5),
        hover: Duration::from_millis(300),
        definition: Duration::from_millis(300),
        references: Duration::from_millis(300),
        stop_grace: Duration::from_millis(300),
    }
}

fn start(handler: Handler) -> (LspClient, Received, Arc<Mutex<Vec<ClientEvent>>>) {
    start_with_settings(handler, json!({}), 0)
}

fn start_with_settings(
    handler: Handler,
    settings: Value,
    chunk: usize,
) -> (LspClient, Received, Arc<Mutex<Vec<ClientEvent>>>) {
    let (spawn, received) = fake(chunk, handler);
    let (sink, seen) = events();
    let client = LspClient::start_with(
        spawn,
        WorkspacePaths::new(ROOT),
        None,
        ServerConfig {
            root: String::new(),
            initialization_options: json!({"x": 1}),
            settings,
        },
        sink,
        quick(),
    )
    .expect("the fake starts");
    (client, received, seen)
}

#[test]
fn initialize_offers_utf8_and_says_initialized() {
    let (client, received, seen) = start(standard(utf8(), |_, _| {}));
    assert_eq!(client.status(), ServerStatus::Ready);
    assert_eq!(client.encoding(), PositionEncoding::Utf8);
    assert_eq!(client.pid(), 4242);
    let initialize = &received.with_method("initialize")[0];
    let params = &initialize["params"];
    assert_eq!(
        params["capabilities"]["general"]["positionEncodings"],
        json!(["utf-8", "utf-16"])
    );
    assert_eq!(params["processId"], Value::Null);
    assert_eq!(params["rootUri"], "file:///w/repo");
    assert_eq!(
        params["workspaceFolders"],
        json!([{"uri": "file:///w/repo", "name": "repo"}])
    );
    assert_eq!(params["initializationOptions"], json!({"x": 1}));
    received.wait_for("didChangeConfiguration", |m| {
        m["method"] == "workspace/didChangeConfiguration"
    });
    assert_eq!(
        received.methods(),
        [
            "initialize",
            "initialized",
            "workspace/didChangeConfiguration"
        ]
    );
    assert!(
        seen.lock()
            .contains(&ClientEvent::Status(ServerStatus::Ready))
    );
}

/// A hover answer as TypeScript gives it.
fn ts_hover(message: &Value, out: &mut Out) {
    if message["method"] == "textDocument/hover" {
        out.reply(
            message,
            json!({
                "contents": {"kind": "markdown", "value": "```typescript\nfunction greet(name: string): string\n```\nSays hello."},
                "range": {"start": {"line": 0, "character": 9}, "end": {"line": 0, "character": 14}}
            }),
        );
    }
}

#[test]
fn a_hover_round_trips_in_utf8() {
    let (client, received, _) = start(standard(utf8(), ts_hover));
    client
        .open_document("src/a.ts", "typescript", "function greet() {}\n")
        .unwrap();
    let hover = client
        .hover("src/a.ts", TextPosition::new(0, 11))
        .wait()
        .unwrap()
        .expect("an answer");
    assert_eq!(
        hover.parts,
        [HoverPart::Markdown(
            "```typescript\nfunction greet(name: string): string\n```\nSays hello.".into()
        )]
    );
    assert_eq!(
        hover.range,
        Some(TextRange {
            start: TextPosition::new(0, 9),
            end: TextPosition::new(0, 14)
        })
    );
    let sent = &received.with_method("textDocument/hover")[0]["params"];
    assert_eq!(sent["textDocument"]["uri"], "file:///w/repo/src/a.ts");
    assert_eq!(sent["position"], json!({"line": 0, "character": 11}));
    let open = &received.with_method("textDocument/didOpen")[0]["params"]["textDocument"];
    assert_eq!(open["languageId"], "typescript");
    assert_eq!(open["text"], "function greet() {}\n");
}

#[test]
fn answers_arrive_byte_by_byte_and_several_to_a_chunk() {
    // One byte at a time.
    let (client, _, _) = start_with_settings(standard(utf8(), ts_hover), Value::Null, 1);
    client
        .open_document("a.ts", "typescript", "function greet() {}")
        .unwrap();
    assert!(
        client
            .hover("a.ts", TextPosition::new(0, 10))
            .wait()
            .unwrap()
            .is_some()
    );

    // Two answers in one chunk, the second request's first.
    let first = Arc::new(Mutex::new(None::<Value>));
    let (client, _, _) = start(standard(utf8(), move |message, out| {
        if message["method"] != "textDocument/hover" {
            return;
        }
        let mut held = first.lock();
        match held.take() {
            None => *held = Some(message.clone()),
            Some(earlier) => {
                let answer = |request: &Value, text: &str| {
                    encode(
                        &serde_json::to_vec(&json!({
                            "jsonrpc": "2.0", "id": request["id"],
                            "result": {"contents": text}
                        }))
                        .unwrap(),
                    )
                };
                let mut both = answer(message, "second");
                both.extend(answer(&earlier, "first"));
                out.raw(&both);
            }
        }
    }));
    client.open_document("a.ts", "typescript", "x").unwrap();
    let one = client.hover("a.ts", TextPosition::new(0, 0));
    let two = client.hover("a.ts", TextPosition::new(0, 0));
    assert_eq!(
        one.wait().unwrap().unwrap().parts,
        [HoverPart::Markdown("first".into())]
    );
    assert_eq!(
        two.wait().unwrap().unwrap().parts,
        [HoverPart::Markdown("second".into())]
    );
}

#[test]
fn a_dropped_request_is_cancelled_and_its_late_answer_ignored() {
    let held = Arc::new(Mutex::new(Vec::<Value>::new()));
    let keep = held.clone();
    let (client, received, _) = start(standard(utf8(), move |message, out| {
        match message["method"].as_str() {
            Some("textDocument/hover") => keep.lock().push(message.clone()),
            // On the cancel, answer the cancelled request anyway, then the next one.
            Some("$/cancelRequest") => {
                for request in keep.lock().drain(..) {
                    out.reply(&request, json!({"contents": "late"}));
                }
            }
            Some("textDocument/definition") => out.reply(message, Value::Null),
            _ => {}
        }
    }));
    client.open_document("a.ts", "typescript", "x").unwrap();
    let pending = client.hover("a.ts", TextPosition::new(0, 0));
    let id = pending.id().unwrap();
    received.wait_for("the hover", |m| m["method"] == "textDocument/hover");
    drop(pending);
    let cancel = received.wait_for("the cancel", |m| m["method"] == "$/cancelRequest");
    assert_eq!(cancel["params"], json!({"id": id}));
    // The late answer went nowhere; the client still works.
    assert_eq!(
        client
            .definition("a.ts", TextPosition::new(0, 0))
            .wait()
            .unwrap(),
        []
    );
    assert!(client.shared.state.lock().pending.is_empty());
}

#[test]
fn a_canceller_ends_a_wait_on_another_thread() {
    let (client, received, _) = start(standard(utf8(), |_, _| {}));
    client.open_document("a.ts", "typescript", "x").unwrap();
    let pending = client.hover("a.ts", TextPosition::new(0, 0));
    let id = pending.id().unwrap();
    let canceller = pending.canceller().unwrap();
    let waiting = std::thread::spawn(move || pending.wait());
    received.wait_for("the hover", |m| m["method"] == "textDocument/hover");
    let started = std::time::Instant::now();
    canceller.cancel();
    assert_eq!(waiting.join().unwrap().unwrap_err(), RequestError::Exited);
    // Well before the hover's own timeout.
    assert!(started.elapsed() < Duration::from_millis(250));
    let cancel = received.wait_for("the cancel", |m| m["method"] == "$/cancelRequest");
    assert_eq!(cancel["params"]["id"], id);
    assert!(client.shared.state.lock().pending.is_empty());
    // A second cancel, or one after the answer, sends nothing more.
    canceller.cancel();
}

#[test]
fn a_request_times_out_and_is_cancelled() {
    let (client, received, _) = start(standard(utf8(), |_, _| {}));
    client.open_document("a.ts", "typescript", "x").unwrap();
    let pending = client.hover("a.ts", TextPosition::new(0, 0));
    let id = pending.id().unwrap();
    let started = std::time::Instant::now();
    let error = pending.wait().unwrap_err();
    assert!(started.elapsed() >= Duration::from_millis(250));
    assert_eq!(
        error,
        RequestError::Timeout {
            method: "textDocument/hover".into(),
            after: Duration::from_millis(300)
        }
    );
    let cancel = received.wait_for("the cancel", |m| m["method"] == "$/cancelRequest");
    assert_eq!(cancel["params"]["id"], id);
}

#[test]
fn an_error_answer_and_a_request_about_a_closed_document() {
    let (client, _, _) = start(standard(utf8(), |message, out| {
        if message["method"] == "textDocument/references" {
            out.send(json!({"jsonrpc": "2.0", "id": message["id"],
                "error": {"code": -32603, "message": "no project"}}));
        }
    }));
    assert_eq!(
        client
            .hover("never.ts", TextPosition::new(0, 0))
            .wait()
            .unwrap_err(),
        RequestError::NotOpen("never.ts".into())
    );
    client.open_document("a.ts", "typescript", "x").unwrap();
    assert_eq!(
        client
            .references("a.ts", TextPosition::new(0, 0), true)
            .wait()
            .unwrap_err(),
        RequestError::Server {
            method: "textDocument/references".into(),
            code: -32603,
            message: "no project".into()
        }
    );
}

#[test]
fn utf16_positions_are_converted_both_ways() {
    let repo = FixtureRepo::new();
    // "é" is 2 bytes and 1 UTF-16 unit, "😀" 4 bytes and 2 units.
    repo.write(
        "src/other.ts",
        "export const caf\u{e9} = 1;\nconst \u{1f600}x = caf\u{e9};\n",
    );
    repo.commit("other");
    let backend: Arc<dyn Backend> = Arc::new(
        LocalBackend::open_with(
            repo.path(),
            LocalBackendOptions {
                env: repo.env_overlay(),
            },
        )
        .unwrap(),
    );
    let (spawn, received) = fake(
        0,
        standard(json!({"definitionProvider": true}), |message, out| {
            match message["method"].as_str() {
                Some("textDocument/hover") => out.reply(
                    message,
                    json!({"contents": "x",
                        "range": {"start": {"line": 0, "character": 2}, "end": {"line": 0, "character": 3}}}),
                ),
                Some("textDocument/definition") => out.reply(
                    message,
                    json!([
                        {"targetUri": "file:///w/repo/src/a.ts",
                         "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}},
                         "targetSelectionRange": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 4}}},
                        {"targetUri": "file:///w/repo/src/other.ts",
                         "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 20}},
                         "targetSelectionRange": {"start": {"line": 1, "character": 6}, "end": {"line": 1, "character": 9}}},
                        {"targetUri": "file:///elsewhere/lib.d.ts",
                         "targetRange": {"start": {"line": 3, "character": 0}, "end": {"line": 3, "character": 9}},
                         "targetSelectionRange": {"start": {"line": 3, "character": 7}, "end": {"line": 3, "character": 8}}}
                    ]),
                ),
                _ => {}
            }
        }),
    );
    let (sink, _) = events();
    let client = LspClient::start_with(
        spawn,
        WorkspacePaths::new(ROOT),
        Some(backend),
        ServerConfig::default(),
        sink,
        quick(),
    )
    .unwrap();
    assert_eq!(client.encoding(), PositionEncoding::Utf16);
    // The open document is `src/a.ts` here, though the fixture's files are at its own root:
    // `files` reads `src/other.ts`, which both have.
    client
        .open_document("src/a.ts", "typescript", "\u{e9}\u{1f600}ab\n")
        .unwrap();
    // Byte 6 is after "é😀": UTF-16 unit 3.
    let hover = client
        .hover("src/a.ts", TextPosition::new(0, 6))
        .wait()
        .unwrap()
        .unwrap();
    let sent = &received.with_method("textDocument/hover")[0]["params"];
    assert_eq!(sent["position"], json!({"line": 0, "character": 3}));
    // Units 2..3 are the emoji's low half to "a": bytes 2..6 (a half counts as its start).
    assert_eq!(
        hover.range,
        Some(TextRange {
            start: TextPosition::new(0, 2),
            end: TextPosition::new(0, 6)
        })
    );
    let definitions = client
        .definition("src/a.ts", TextPosition::new(0, 6))
        .wait()
        .unwrap();
    assert_eq!(
        definitions,
        [
            SymbolLocation {
                target: UriTarget::Worktree("src/a.ts".into()),
                range: TextRange {
                    start: TextPosition::new(0, 6),
                    end: TextPosition::new(0, 7)
                }
            },
            // "const 😀x": units 6..9 are "😀x", bytes 6..11.
            SymbolLocation {
                target: UriTarget::Worktree("src/other.ts".into()),
                range: TextRange {
                    start: TextPosition::new(1, 6),
                    end: TextPosition::new(1, 11)
                }
            },
            SymbolLocation {
                target: UriTarget::Outside {
                    uri: "file:///elsewhere/lib.d.ts".into(),
                    path: Some("/elsewhere/lib.d.ts".into())
                },
                range: TextRange {
                    start: TextPosition::new(3, 7),
                    end: TextPosition::new(3, 8)
                }
            },
        ]
    );
}

#[test]
fn past_the_bound_the_document_asked_about_least_recently_closes() {
    let (client, received, _) = start(standard(utf8(), |_, _| {}));
    for n in 0..DOCUMENTS_OPEN {
        client
            .open_document(&format!("f{n}.ts"), "typescript", "x")
            .unwrap();
    }
    // f0 asked about again: f1 is now the least recent.
    client.open_document("f0.ts", "typescript", "x").unwrap();
    client.open_document("more.ts", "typescript", "x").unwrap();
    received.wait_for_count("textDocument/didClose", 1);
    let closed: Vec<Value> = received
        .all()
        .into_iter()
        .filter(|m| m["method"] == "textDocument/didClose")
        .collect();
    assert_eq!(closed.len(), 1);
    assert!(
        closed[0]["params"]["textDocument"]["uri"]
            .as_str()
            .is_some_and(|uri| uri.ends_with("/f1.ts")),
        "{closed:?}"
    );
    assert_eq!(client.open_documents().len(), DOCUMENTS_OPEN);
    assert!(client.document_text("f0.ts").is_some() && client.document_text("f1.ts").is_none());
}

#[test]
fn one_version_of_a_path_is_open_at_a_time() {
    let (client, received, _) = start(standard(utf8(), |_, _| {}));
    client.open_document("a.ts", "typescript", "old").unwrap();
    client.open_document("a.ts", "typescript", "old").unwrap();
    client.open_document("a.ts", "typescript", "new").unwrap();
    client.close_document("a.ts");
    client.close_document("a.ts");
    received.wait_for_count("textDocument/didClose", 2);
    let documents: Vec<(String, Value)> = received
        .all()
        .into_iter()
        .filter(|m| {
            m["method"]
                .as_str()
                .is_some_and(|method| method.starts_with("textDocument/did"))
        })
        .map(|m| {
            let doc = &m["params"]["textDocument"];
            (
                m["method"].as_str().unwrap().to_string(),
                json!([doc["version"], doc["text"]]),
            )
        })
        .collect();
    assert_eq!(
        documents,
        [
            ("textDocument/didOpen".to_string(), json!([1, "old"])),
            ("textDocument/didClose".to_string(), json!([null, null])),
            ("textDocument/didOpen".to_string(), json!([2, "new"])),
            ("textDocument/didClose".to_string(), json!([null, null])),
        ]
    );
    assert_eq!(client.document_text("a.ts"), None);
}

#[test]
fn the_servers_requests_are_answered() {
    let (client, received, _) = start_with_settings(
        standard(utf8(), |message, out| {
            if message["method"] == "initialized" {
                out.send(
                    json!({"jsonrpc": "2.0", "id": 1, "method": "workspace/configuration",
                    "params": {"items": [
                        {"section": "typescript.preferences"}, {"section": "nope.x"}, {}
                    ]}}),
                );
                out.send(json!({"jsonrpc": "2.0", "id": "two", "method": "window/workDoneProgress/create",
                    "params": {"token": "load"}}));
                out.send(
                    json!({"jsonrpc": "2.0", "id": 3, "method": "workspace/workspaceFolders"}),
                );
                out.send(
                    json!({"jsonrpc": "2.0", "id": 4, "method": "window/showDocument",
                    "params": {"uri": "https://example.com"}}),
                );
            }
        }),
        json!({"typescript": {"preferences": {"quoteStyle": "single"}}}),
        0,
    );
    let answer =
        |id: Value| received.wait_for("an answer", |m| m["id"] == id && m.get("method").is_none());
    assert_eq!(
        answer(json!(1))["result"],
        json!([{"quoteStyle": "single"}, null, {"typescript": {"preferences": {"quoteStyle": "single"}}}])
    );
    assert_eq!(answer(json!("two"))["result"], Value::Null);
    assert_eq!(
        answer(json!(3))["result"],
        json!([{"uri": "file:///w/repo", "name": "repo"}])
    );
    assert_eq!(answer(json!(4))["error"]["code"], -32601);
    drop(client);
}

#[test]
fn watched_files_follow_the_registrations() {
    let (client, received, _) = start(standard(utf8(), |message, out| {
        if message["method"] == "initialized" {
            out.send(json!({"jsonrpc": "2.0", "id": 1, "method": "client/registerCapability",
                "params": {"registrations": [
                    {"id": "w1", "method": "workspace/didChangeWatchedFiles", "registerOptions": {
                        "watchers": [{"globPattern": "**/*.{ts,json}"}]}},
                    {"id": "w2", "method": "workspace/didChangeWatchedFiles", "registerOptions": {
                        "watchers": [{"globPattern": {"baseUri": "file:///w/repo", "pattern": "*.md"}, "kind": 4}]}},
                    {"id": "c1", "method": "workspace/didChangeConfiguration"}
                ]}}));
        }
    }));
    received.wait_for("the registration's answer", |m| {
        m["id"] == 1 && m.get("method").is_none()
    });
    assert_eq!(
        client.registrations(),
        [
            (
                "w1".to_string(),
                "workspace/didChangeWatchedFiles".to_string()
            ),
            ("w2".into(), "workspace/didChangeWatchedFiles".into()),
            ("c1".into(), "workspace/didChangeConfiguration".into()),
        ]
    );
    let told = client.files_changed(&[
        ("src/a.ts".into(), CHANGE),
        ("README.md".into(), CHANGE),
        ("README.md".into(), DELETE),
        ("img.png".into(), CREATE),
        ("package.json".into(), CREATE),
    ]);
    assert_eq!(told, 3);
    let sent = received.wait_for("the changes", |m| {
        m["method"] == "workspace/didChangeWatchedFiles"
    });
    assert_eq!(
        sent["params"]["changes"],
        json!([
            {"uri": "file:///w/repo/src/a.ts", "type": 2},
            {"uri": "file:///w/repo/README.md", "type": 3},
            {"uri": "file:///w/repo/package.json", "type": 1},
        ])
    );
    assert_eq!(client.files_changed(&[("x.png".into(), CHANGE)]), 0);
    // Which paths are worth looking at before telling it (any kind of change).
    assert_eq!(
        client.watched(&[
            "src/a.ts".to_string(),
            "img.png".into(),
            "README.md".into(),
            "docs/x.md".into()
        ]),
        ["src/a.ts".to_string(), "README.md".into()]
    );
}

#[test]
fn progress_begins_reports_and_ends() {
    let (client, received, seen) = start(standard(utf8(), |message, out| {
        let progress = |out: &mut Out, value: Value| {
            out.send(json!({"jsonrpc": "2.0", "method": "$/progress",
                "params": {"token": "load", "value": value}}));
        };
        match message["method"].as_str() {
            Some("initialized") => {
                progress(
                    out,
                    json!({"kind": "begin", "title": "Loading project", "percentage": 0}),
                );
                progress(
                    out,
                    json!({"kind": "report", "message": "3/4 files", "percentage": 75}),
                );
            }
            Some("textDocument/didOpen") => progress(out, json!({"kind": "end"})),
            _ => {}
        }
    }));
    let loading = Progress {
        token: "load".into(),
        title: "Loading project".into(),
        message: Some("3/4 files".into()),
        percentage: Some(75),
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while client.progress() != [loading.clone()] {
        assert!(std::time::Instant::now() < deadline, "no progress");
        std::thread::sleep(Duration::from_millis(5));
    }
    client.open_document("a.ts", "typescript", "x").unwrap();
    received.wait_for("the open", |m| m["method"] == "textDocument/didOpen");
    while !client.progress().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the progress never ended"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let progress_events: Vec<ClientEvent> = seen
        .lock()
        .iter()
        .filter(|event| matches!(event, ClientEvent::Progress(_)))
        .cloned()
        .collect();
    assert_eq!(
        progress_events,
        [
            ClientEvent::Progress(vec![Progress {
                message: None,
                percentage: Some(0),
                ..loading.clone()
            }]),
            ClientEvent::Progress(vec![loading]),
            ClientEvent::Progress(vec![]),
        ]
    );
}

#[test]
fn a_server_that_ends_fails_its_requests() {
    let (client, _, seen) = start(standard(utf8(), |message, out| {
        if message["method"] == "textDocument/hover" {
            out.stderr("panic: nil map");
            out.exit(Some(2));
        }
    }));
    client.open_document("a.ts", "typescript", "x").unwrap();
    assert_eq!(
        client
            .hover("a.ts", TextPosition::new(0, 0))
            .wait()
            .unwrap_err(),
        RequestError::Exited
    );
    assert_eq!(client.status(), ServerStatus::Exited(Some(2)));
    assert_eq!(client.stderr_tail(), ["panic: nil map"]);
    assert_eq!(
        client
            .definition("a.ts", TextPosition::new(0, 0))
            .wait()
            .unwrap_err(),
        RequestError::Exited
    );
    assert_eq!(
        client.open_document("b.ts", "typescript", "y"),
        Err(RequestError::Exited)
    );
    let seen = seen.lock();
    assert!(seen.contains(&ClientEvent::Stderr("panic: nil map".into())));
    assert_eq!(
        seen.last(),
        Some(&ClientEvent::Status(ServerStatus::Exited(Some(2))))
    );
}

#[test]
fn a_server_that_ends_while_starting() {
    let (spawn, _) = fake(0, |message: &Value, out: &mut Out| {
        if message["method"] == "initialize" {
            out.stderr("error: unknown flag --lsp");
            out.exit(Some(1));
        }
    });
    let (sink, _) = events();
    let error = LspClient::start_with(
        spawn,
        WorkspacePaths::new(ROOT),
        None,
        ServerConfig::default(),
        sink,
        quick(),
    )
    .unwrap_err();
    match error {
        StartError::Exited { code, stderr } => {
            assert_eq!(code, Some(1));
            assert_eq!(stderr, ["error: unknown flag --lsp"]);
        }
        other => panic!("expected an exit, got {other:?}"),
    }
}

#[test]
fn stop_says_shutdown_and_exit_without_params() {
    // Like `tsc`: `exit` with params is ignored, so a stop only ends the server without them.
    let (client, received, seen) = start(Box::new(|message: &Value, out: &mut Out| {
        match message["method"].as_str() {
            Some("initialize") => out.reply(message, json!({"capabilities": {}})),
            Some("shutdown") if message.get("params").is_some() => out.send(json!({
                "jsonrpc": "2.0", "id": message["id"],
                "error": {"code": -32602, "message": "expected no params, got null"}
            })),
            Some("shutdown") => out.reply(message, Value::Null),
            // `tsc` exits 1 even after a proper shutdown.
            Some("exit") if message.get("params").is_none() => out.exit(Some(1)),
            _ => {}
        }
    }));
    let started = std::time::Instant::now();
    assert_eq!(client.stop(), ServerStatus::Stopped(Some(1)));
    // Well within the grace: nothing had to be killed.
    assert!(started.elapsed() < Duration::from_millis(250));
    let ending: Vec<Value> = received
        .all()
        .into_iter()
        .filter(|m| m["method"] == "shutdown" || m["method"] == "exit")
        .collect();
    assert_eq!(
        ending
            .iter()
            .map(|m| (m["method"].as_str().unwrap(), m.get("params").is_some()))
            .collect::<Vec<_>>(),
        [("shutdown", false), ("exit", false)]
    );
    assert_eq!(
        seen.lock().last(),
        Some(&ClientEvent::Status(ServerStatus::Stopped(Some(1))))
    );
}

/// Waits until the last event is `status`.
fn wait_for_status(seen: &Mutex<Vec<ClientEvent>>, status: ServerStatus) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while seen.lock().last() != Some(&ClientEvent::Status(status)) {
        assert!(
            std::time::Instant::now() < deadline,
            "never {status:?}: {:?}",
            seen.lock()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn dropping_the_client_kills_the_server_and_ends_its_requests() {
    let (client, received, seen) = start(standard(utf8(), |_, _| {}));
    client.open_document("a.ts", "typescript", "x").unwrap();
    let client = Arc::new(Mutex::new(Some(client)));
    let pending = client
        .lock()
        .as_ref()
        .unwrap()
        .hover("a.ts", TextPosition::new(0, 0));
    received.wait_for("the hover", |m| m["method"] == "textDocument/hover");
    // Another thread waits on it; the client goes meanwhile.
    let waiter = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        (pending.wait(), started.elapsed())
    });
    std::thread::sleep(Duration::from_millis(50));
    client.lock().take();
    let (answer, waited) = waiter.join().unwrap();
    assert_eq!(answer.unwrap_err(), RequestError::Exited);
    // Not the hover's whole timeout (300 ms here).
    assert!(waited < Duration::from_millis(250), "{waited:?}");
    wait_for_status(&seen, ServerStatus::Stopped(None));
}

#[test]
fn a_server_that_ignores_shutdown_is_killed() {
    // Answers initialize and nothing else.
    let (client, _, seen) = start(Box::new(|message: &Value, out: &mut Out| {
        if message["method"] == "initialize" {
            out.reply(message, json!({"capabilities": {}}));
        }
    }));
    let started = std::time::Instant::now();
    // The fake's kill reads as a signal: no code.
    assert_eq!(client.stop(), ServerStatus::Stopped(None));
    // shutdown's wait and the exit's, each the grace.
    assert!(started.elapsed() >= Duration::from_millis(500));
    wait_for_status(&seen, ServerStatus::Stopped(None));
}

#[test]
fn a_crash_is_not_a_stop() {
    let (client, _, seen) = start(standard(utf8(), |message, out| {
        if message["method"] == "textDocument/didOpen" {
            out.exit(Some(0));
        }
    }));
    client.open_document("a.ts", "typescript", "x").unwrap();
    wait_for_status(&seen, ServerStatus::Exited(Some(0)));
    assert_eq!(client.stop(), ServerStatus::Exited(Some(0)));
}

#[test]
fn a_badly_spelled_uri_loses_only_its_entry() {
    let (client, _, _) = start(standard(utf8(), |message, out| {
        if message["method"] == "textDocument/references" {
            let range =
                json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}});
            out.reply(
                message,
                json!([
                    {"uri": "file:///w/repo/src/with space.ts", "range": range},
                    {"uri": "file:///w/repo/src/ok.ts", "range": range},
                    {"uri": 42, "range": range},
                    {"uri": "file:///w/repo/src/no-range.ts"}
                ]),
            );
        }
    }));
    client.open_document("a.ts", "typescript", "x").unwrap();
    let targets: Vec<UriTarget> = client
        .references("a.ts", TextPosition::new(0, 0), true)
        .wait()
        .unwrap()
        .into_iter()
        .map(|place| place.target)
        .collect();
    // An unencoded space is taken as the path it plainly names; the malformed ones are dropped.
    assert_eq!(
        targets,
        [
            UriTarget::Worktree("src/with space.ts".into()),
            UriTarget::Worktree("src/ok.ts".into())
        ]
    );
}
