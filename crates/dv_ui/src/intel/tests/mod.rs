//! Code intelligence over real fixture repos and an in-process language server (PLAN §18.5,
//! §18.6): servers start once the file list shows, hover asks the server about the text the
//! stream shows and a hover the pointer left is cancelled, go to definition goes to the stream
//! or the file view, references list with the review's changed rows, a crash restarts, a stop
//! doesn't, a missing server says why, and changed files reach the server.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use dv_core::backend::{BackendError, ProcessControl, ProcessEvent, ProcessHandle, ProcessSink};
use dv_core::review::AnchorSide;
use dv_core::{Location, Target};
use dv_lsp::framing::{Decoder, encode};
use dv_lsp::{LspClient, Timeouts, WorkspacePaths};
use dv_test_support::FixtureRepo;
use gpui::{Entity, Focusable as _, TestAppContext, VisualTestContext};
use parking_lot::Mutex;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::*;
use crate::services::ActiveServices as _;
use crate::services::reviews::OpenRequest;
use crate::shell::Shell;
use crate::test_support::{add_repo, open_shell, settle};

mod nav;
mod stacked;

/// What the fake server answers: a message in, `Some(result)` to answer a request with.
type Answer = Arc<dyn Fn(&Value) -> Option<Value> + Send + Sync>;

/// The fake servers a test's starter made: every message each read, and a way to end them.
#[derive(Clone, Default)]
struct Fakes {
    received: Arc<Mutex<Vec<Value>>>,
    /// Ends the running server as a crash would.
    crash: Arc<Mutex<Option<Sender<Input>>>>,
    starts: Arc<Mutex<usize>>,
    /// What `initialize` answers as its capabilities, when not hover, definition and
    /// references.
    capabilities: Arc<Mutex<Option<Value>>>,
}

impl Fakes {
    fn with_method(&self, method: &str) -> Vec<Value> {
        self.received
            .lock()
            .iter()
            .filter(|message| message["method"] == method)
            .cloned()
            .collect()
    }

    fn starts(&self) -> usize {
        *self.starts.lock()
    }

    fn crash(&self) {
        if let Some(input) = self.crash.lock().as_ref() {
            let _ = input.send(Input::Crash);
        }
    }
}

enum Input {
    Bytes(Vec<u8>),
    Kill,
    Crash,
}

struct FakeControl {
    input: Sender<Input>,
}

impl ProcessControl for FakeControl {
    fn write(&self, bytes: &[u8]) -> Result<(), BackendError> {
        self.input
            .send(Input::Bytes(bytes.to_vec()))
            .map_err(|_| dv_core::backend::process::ended("fake"))
    }

    fn kill(&self) {
        let _ = self.input.send(Input::Kill);
    }
}

/// A server on a thread of its own: answers `initialize` (UTF-8 positions), `shutdown`, exits
/// on `exit`, and every other request through `answer` (`null` when it says nothing).
fn spawn_fake(sink: ProcessSink, answer: Answer, fakes: &Fakes) -> ProcessHandle {
    let (input, inputs): (Sender<Input>, Receiver<Input>) = mpsc::channel();
    *fakes.crash.lock() = Some(input.clone());
    *fakes.starts.lock() += 1;
    let received = fakes.received.clone();
    let capabilities = fakes.capabilities.lock().clone().unwrap_or_else(|| {
        json!({"positionEncoding": "utf-8", "hoverProvider": true,
            "definitionProvider": true, "referencesProvider": true})
    });
    std::thread::spawn(move || {
        let mut sink = sink;
        let mut decoder = Decoder::new();
        let send = |sink: &mut ProcessSink, message: Value| {
            let body = serde_json::to_vec(&message).unwrap();
            sink(ProcessEvent::Stdout(encode(&body)));
        };
        for input in inputs {
            match input {
                Input::Bytes(bytes) => {
                    decoder.push(&bytes);
                    while let Some(Ok(body)) = decoder.next_message() {
                        let message: Value = serde_json::from_slice(&body).unwrap();
                        received.lock().push(message.clone());
                        let id = message.get("id").cloned();
                        match message["method"].as_str() {
                            Some("initialize") => send(
                                &mut sink,
                                json!({"jsonrpc": "2.0", "id": id, "result": {
                                    "capabilities": capabilities.clone()}}),
                            ),
                            Some("shutdown") => send(
                                &mut sink,
                                json!({"jsonrpc": "2.0", "id": id, "result": null}),
                            ),
                            Some("exit") => {
                                sink(ProcessEvent::Exit(Some(0)));
                                return;
                            }
                            _ if id.is_some() && message.get("method").is_some() => {
                                let result = answer(&message).unwrap_or(Value::Null);
                                send(
                                    &mut sink,
                                    json!({"jsonrpc": "2.0", "id": id, "result": result}),
                                );
                            }
                            _ => {}
                        }
                    }
                }
                Input::Kill => {
                    sink(ProcessEvent::Exit(None));
                    return;
                }
                Input::Crash => {
                    sink(ProcessEvent::Exit(Some(3)));
                    return;
                }
            }
        }
    });
    ProcessHandle::new(7, FakeControl { input })
}

/// Has the review's language servers be the fake (`DV_LSP_TYPESCRIPT` names it), or with
/// `answer` `None` leaves TypeScript to the resolvers (none installed: missing).
fn use_fake(answer: Option<Answer>, cx: &mut VisualTestContext) -> Fakes {
    let fakes = Fakes::default();
    let servers = cx.update(|_, cx| cx.services().language_servers.clone());
    let made = fakes.clone();
    servers.update(cx, |servers, _| {
        let found = answer.is_some();
        servers.set_env(Arc::new(move |name| {
            (found && name == "DV_LSP_TYPESCRIPT").then(|| "/fake/tsc".to_string())
        }));
        let answer = answer.unwrap_or_else(|| Arc::new(|_| None));
        servers.set_starter(Arc::new(move |backend, _, config, events| {
            let paths = WorkspacePaths::new(&backend.repo().toplevel);
            let answer = answer.clone();
            let made = made.clone();
            LspClient::start_with(
                move |sink| Ok(spawn_fake(sink, answer, &made)),
                paths,
                Some(backend),
                config,
                events,
                Timeouts::default(),
            )
        }));
    });
    fakes
}

/// A TypeScript project: `src/a.ts` changes (its line 2 calls `helper`), `src/b.ts` defines
/// `helper` and doesn't change.
fn project() -> FixtureRepo {
    let repo = FixtureRepo::new();
    repo.write("package.json", "{\"name\": \"web\"}\n");
    repo.write(
        "src/a.ts",
        "import { helper } from './b';\nexport const one = 1;\n",
    );
    repo.write(
        "src/b.ts",
        "export function helper(x: number): number {\n  return x + 1;\n}\n",
    );
    repo.commit("base");
    repo.write(
        "src/a.ts",
        "import { helper } from './b';\nexport const one = helper(0);\n",
    );
    repo
}

fn open_review<'a>(
    repo: &FixtureRepo,
    answer: Option<Answer>,
    cx: &'a mut TestAppContext,
) -> (
    tempfile::TempDir,
    Entity<Shell>,
    &'a mut VisualTestContext,
    Fakes,
) {
    open_target(repo, answer, Target::Working, cx)
}

fn open_target<'a>(
    repo: &FixtureRepo,
    answer: Option<Answer>,
    target: Target,
    cx: &'a mut TestAppContext,
) -> (
    tempfile::TempDir,
    Entity<Shell>,
    &'a mut VisualTestContext,
    Fakes,
) {
    open_with(repo, target, cx, move |cx| use_fake(answer, cx))
}

/// Opens `target` of `repo` in a shell whose language servers `make` sets up first; what it
/// made.
fn open_with<'a, F>(
    repo: &FixtureRepo,
    target: Target,
    cx: &'a mut TestAppContext,
    make: impl FnOnce(&mut VisualTestContext) -> F,
) -> (
    tempfile::TempDir,
    Entity<Shell>,
    &'a mut VisualTestContext,
    F,
) {
    let (dir, shell, cx) = open_shell(cx);
    let fakes = make(cx);
    add_repo(repo, &shell, cx);
    let task = cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open(
                Location::Local(repo.path().to_path_buf()),
                OpenRequest::Target(target),
                window,
                cx,
            )
        })
    });
    cx.run_until_parked();
    settle(cx, &shell, "the review", |shell, cx| {
        shell
            .session()
            .is_some_and(|session| !session.read(cx).is_first_load())
    });
    cx.foreground_executor()
        .block_test(task)
        .expect("the review opens");
    (dir, shell, cx, fakes)
}

fn intel(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<CodeIntel> {
    shell.read_with(cx, |shell, _| {
        shell.intel().cloned().expect("a review's intel")
    })
}

fn servers(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<(String, String)> {
    let intel = intel(shell, cx);
    intel.read_with(cx, |intel, cx| {
        intel
            .servers(cx)
            .into_iter()
            .map(|server| (server.spec_id.to_string(), server.state.key().to_string()))
            .collect()
    })
}

/// `line` of `path`'s new side as the stream shows it, and its diff.
fn symbol(
    shell: &Entity<Shell>,
    path: &str,
    line: u32,
    name: &str,
    cx: &mut VisualTestContext,
) -> (SymbolAt, Arc<dv_core::diff::FileDiff>) {
    shell.read_with(cx, |shell, cx| {
        let session = shell.session().expect("a session").read(cx);
        let diff = session.shown_diff(path).expect("the file's diff").clone();
        let text = diff
            .as_text()
            .and_then(|text| text.lines.iter().find(|l| l.new_no == Some(line)))
            .map(|l| l.text.clone())
            .expect("the line");
        let start = text.find(name).expect("the symbol on the line");
        (
            SymbolAt {
                path: path.to_string(),
                side: AnchorSide::New,
                line,
                range: start..start + name.len(),
                name: name.to_string(),
                offset: start,
                worktree: false,
            },
            diff,
        )
    })
}

fn wait(
    cx: &mut VisualTestContext,
    shell: &Entity<Shell>,
    what: &str,
    done: impl Fn(&App) -> bool,
) {
    settle(cx, shell, what, |_, cx| done(cx));
}

fn location(path: &str, line: u32, column: u32) -> Value {
    json!({"uri": format!("${{root}}/{path}"), "range": {
        "start": {"line": line, "character": column},
        "end": {"line": line, "character": column + 6}}})
}

/// `location` with `${root}` made the fixture's URI.
fn at(root: &str, value: Value) -> Value {
    serde_json::from_str(&value.to_string().replace("${root}", root)).unwrap()
}

#[gpui::test]
fn servers_start_once_the_list_shows_and_hover_reads_the_streams_text(cx: &mut TestAppContext) {
    let repo = project();
    let answer: Answer = Arc::new(|message| {
        (message["method"] == "textDocument/hover").then(|| {
            json!({"contents": {"kind": "markdown",
                "value": "```typescript\nfunction helper(x: number): number\n```\nAdds one."}})
        })
    });
    let (_dir, shell, cx, fakes) = open_review(&repo, Some(answer), cx);
    wait(cx, &shell, "the server", |_| true);
    assert_eq!(servers(&shell, cx), [("typescript".into(), "ready".into())]);
    assert_eq!(fakes.starts(), 1);
    let intel = intel(&shell, cx);
    let (at, diff) = symbol(&shell, "src/a.ts", 2, "helper", cx);
    intel.update(cx, |intel, cx| {
        intel.hover(at.clone(), Pane::Stream, Default::default(), &diff, cx)
    });
    let card = intel.clone();
    wait(cx, &shell, "the hover", move |cx| {
        card.read(cx).hover_card().is_some()
    });
    let card = intel.read_with(cx, |intel, _| intel.hover_card().cloned().unwrap());
    assert_eq!(card.at, at);
    assert!(card.text().contains("function helper(x: number): number"));
    assert!(
        !card.against_worktree,
        "a working review's new side is the disk"
    );
    // The server read the stream's text at the file's path, and was asked at its position.
    let opened = fakes.with_method("textDocument/didOpen");
    assert_eq!(opened.len(), 1);
    assert_eq!(
        opened[0]["params"]["textDocument"]["text"],
        "import { helper } from './b';\nexport const one = helper(0);\n"
    );
    let hover = &fakes.with_method("textDocument/hover")[0];
    assert_eq!(
        hover["params"]["position"],
        json!({"line": 1, "character": 19})
    );
    // The same symbol again asks nothing more; closing takes the card away.
    intel.update(cx, |intel, cx| {
        intel.hover(at, Pane::Stream, Default::default(), &diff, cx)
    });
    wait(cx, &shell, "nothing", |_| true);
    assert_eq!(fakes.with_method("textDocument/hover").len(), 1);
    intel.update(cx, |intel, cx| intel.close_hover(cx));
    assert!(intel.read_with(cx, |intel, _| intel.hover_card().is_none()));
}

#[test]
fn a_request_the_ui_abandons_is_cancelled_while_its_thread_waits() {
    // The fake holds each hover until the test lets that one go (then answers it with null),
    // each after its request was given up: the first after the wait ended, so only the cancel
    // can end that wait before the hover's timeout; the second after its drop, so it's still
    // unanswered, and so cancelled, when it goes.
    let fakes = Fakes::default();
    let made = fakes.clone();
    let (release, gate) = mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let answer: Answer = Arc::new(move |message| {
        if message["method"] == "textDocument/hover" {
            let _ = gate.lock().recv();
        }
        None
    });
    let client = LspClient::start_with(
        move |sink| Ok(spawn_fake(sink, answer, &made)),
        WorkspacePaths::new("/w/repo"),
        None,
        dv_lsp::ServerConfig::default(),
        Arc::new(|_| {}),
        Timeouts::default(),
    )
    .expect("the fake starts");
    client
        .open_document("a.ts", "typescript", "helper")
        .unwrap();
    let (in_flight, arming) = InFlight::detached();
    let pending = client.hover("a.ts", dv_lsp::TextPosition::new(0, 0));
    assert!(arming.arm(&pending));
    let waiting = std::thread::spawn(move || pending.wait());
    drop(in_flight);
    assert_eq!(
        waiting.join().unwrap().unwrap_err(),
        RequestError::Exited,
        "the cancel ends the wait, not the hover's timeout"
    );
    release.send(()).unwrap();
    // A request armed after the UI let go is cancelled by its own drop.
    let (in_flight, arming) = InFlight::detached();
    drop(in_flight);
    let late = client.hover("a.ts", dv_lsp::TextPosition::new(0, 1));
    assert!(!arming.arm(&late));
    assert!(arming.is_abandoned());
    drop(late);
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while fakes.with_method("$/cancelRequest").len() < 2 {
        assert!(Instant::now() < deadline, "the hovers were never cancelled");
        std::thread::sleep(Duration::from_millis(5));
    }
    let hovers: Vec<Value> = fakes
        .with_method("textDocument/hover")
        .iter()
        .map(|hover| hover["id"].clone())
        .collect();
    let cancelled: Vec<Value> = fakes
        .with_method("$/cancelRequest")
        .iter()
        .map(|cancel| cancel["params"]["id"].clone())
        .collect();
    assert_eq!(cancelled, hovers);
}

#[gpui::test]
fn go_to_definition_opens_the_file_view_on_an_unchanged_file_and_back_returns(
    cx: &mut TestAppContext,
) {
    let repo = project();
    let root = WorkspacePaths::new(&repo.path().display().to_string()).uri_text("");
    let answer: Answer = Arc::new(move |message| {
        (message["method"] == "textDocument/definition")
            .then(|| at(&root, json!([location("src/b.ts", 0, 16)])))
    });
    let (_dir, shell, cx, _fakes) = open_review(&repo, Some(answer), cx);
    wait(cx, &shell, "the server", |_| true);
    let intel = intel(&shell, cx);
    let (at, diff) = symbol(&shell, "src/a.ts", 2, "helper", cx);
    intel.update(cx, |intel, cx| {
        intel.definition(at, Pane::Stream, &diff, cx)
    });
    let view = shell.read_with(cx, |shell, _| shell.file_view().clone());
    let shown = view.clone();
    wait(cx, &shell, "the file view", move |cx| {
        shown.read(cx).file_view_path() == Some("src/b.ts")
            && shown
                .read(cx)
                .file_view_summary(cx)
                .is_some_and(|summary| !summary.loading)
    });
    assert!(shell.read_with(cx, |shell, _| shell.is_file_view_open()));
    assert_eq!(
        intel.read_with(cx, |intel, _| intel.last_definitions()),
        Some(1)
    );
    let (cursor, caret) = view.read_with(cx, |view, _| (view.cursor(), view.caret()));
    assert_eq!(
        cursor.map(|c| (c.path, c.line)),
        Some(("src/b.ts".into(), 1))
    );
    assert_eq!(
        caret.map(|(path, place)| (path, place.column)),
        Some(("src/b.ts".into(), 17))
    );
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.history().counts()),
        (1, 0)
    );
    // Alt ← comes back to the stream.
    cx.simulate_keystrokes("alt-left");
    cx.run_until_parked();
    assert!(!shell.read_with(cx, |shell, _| shell.is_file_view_open()));
}

#[gpui::test]
fn a_definition_the_stream_shows_scrolls_the_stream_and_one_outside_opens_read_only(
    cx: &mut TestAppContext,
) {
    let repo = project();
    let root = WorkspacePaths::new(&repo.path().display().to_string()).uri_text("");
    // A library's declarations outside the repository.
    let outside = repo.outside_dir("lib");
    let lib: String = (1..=12)
        .map(|n| format!("declare const x{n}: number;\n"))
        .collect();
    std::fs::write(outside.join("lib.d.ts"), &lib).unwrap();
    let lib_uri = WorkspacePaths::new(&outside.display().to_string()).uri_text("lib.d.ts");
    let calls = Arc::new(Mutex::new(0));
    let answer: Answer = Arc::new(move |message| {
        if message["method"] != "textDocument/definition" {
            return None;
        }
        let mut calls = calls.lock();
        *calls += 1;
        let range =
            json!({"start": {"line": 9, "character": 14}, "end": {"line": 9, "character": 17}});
        Some(match *calls {
            1 => at(&root, json!([location("src/a.ts", 1, 13)])),
            2 => json!([{"uri": lib_uri, "range": range}]),
            _ => json!([{"uri": "untitled:Untitled-1", "range": range}]),
        })
    });
    let (_dir, shell, cx, _fakes) = open_review(&repo, Some(answer), cx);
    wait(cx, &shell, "the server", |_| true);
    let intel = intel(&shell, cx);
    let (at, diff) = symbol(&shell, "src/a.ts", 1, "helper", cx);
    intel.update(cx, |intel, cx| {
        intel.definition(at.clone(), Pane::Stream, &diff, cx)
    });
    let stream = shell.read_with(cx, |shell, _| shell.diff_pane().clone());
    let shown = stream.clone();
    wait(cx, &shell, "the stream's caret", move |cx| {
        shown.read(cx).caret().is_some()
    });
    assert!(!shell.read_with(cx, |shell, _| shell.is_file_view_open()));
    let caret = stream.read_with(cx, |pane, _| pane.caret());
    assert_eq!(
        caret.map(|(path, place)| (path, place.line, place.column)),
        Some(("src/a.ts".into(), 2, 14))
    );
    // A file outside the repository: the file view shows it read-only, at the place.
    intel.update(cx, |intel, cx| {
        intel.definition(at.clone(), Pane::Stream, &diff, cx)
    });
    let view = shell.read_with(cx, |shell, _| shell.file_view().clone());
    let shown = view.clone();
    wait(cx, &shell, "the outside file", move |cx| {
        shown
            .read(cx)
            .file_view_summary(cx)
            .is_some_and(|summary| !summary.loading)
    });
    let summary = view
        .read_with(cx, |view, cx| view.file_view_summary(cx))
        .unwrap();
    assert!(summary.outside && !summary.worktree, "{summary:?}");
    assert!(summary.path.ends_with("lib.d.ts"), "{summary:?}");
    assert_eq!(summary.line_count, 12);
    assert_eq!(
        view.read_with(cx, |view, _| view.caret())
            .map(|(_, place)| (place.line, place.column)),
        Some((10, 15))
    );
    assert!(intel.read_with(cx, |intel, _| intel.notice().is_none()));
    // Read-only: no comment there.
    view.update_in(cx, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx)
    });
    cx.simulate_keystrokes("c");
    cx.run_until_parked();
    assert!(view.read_with(cx, |view, _| view.composer().is_none()));
    // A place that isn't a file: the quiet notice says where, and `Esc` takes it away.
    let (at_view, view_diff) = view.read_with(cx, |view, _| {
        view.symbol_at_caret_for_tests()
            .expect("the caret's symbol")
    });
    assert_eq!(at_view.path, summary.path);
    assert!(
        at_view.worktree,
        "a server reads an outside file from the disk"
    );
    intel.update(cx, |intel, cx| {
        intel.definition(at_view, Pane::FileView, &view_diff, cx)
    });
    let noticed = intel.clone();
    wait(cx, &shell, "the notice", move |cx| {
        noticed.read(cx).notice().is_some()
    });
    let notice = intel.read_with(cx, |intel, _| intel.notice().cloned().unwrap());
    assert_eq!(
        notice.message,
        "Defined outside the repository: untitled:Untitled-1:10"
    );
    view.update_in(cx, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx)
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(intel.read_with(cx, |intel, _| intel.notice().is_none()));
}

#[gpui::test]
fn references_list_by_file_with_the_rows_the_review_changed(cx: &mut TestAppContext) {
    let repo = project();
    let root = WorkspacePaths::new(&repo.path().display().to_string()).uri_text("");
    let answer: Answer = Arc::new(move |message| {
        (message["method"] == "textDocument/references").then(|| {
            at(
                &root,
                json!([
                    location("src/b.ts", 0, 16),
                    location("src/a.ts", 1, 19),
                    location("src/a.ts", 0, 9),
                ]),
            )
        })
    });
    let (_dir, shell, cx, _fakes) = open_review(&repo, Some(answer), cx);
    wait(cx, &shell, "the server", |_| true);
    let intel = intel(&shell, cx);
    let (at, diff) = symbol(&shell, "src/a.ts", 2, "helper", cx);
    intel.update(cx, |intel, cx| {
        intel.references_at(at, Pane::Stream, &diff, cx)
    });
    assert_eq!(
        shell.read_with(cx, |shell, cx| shell.panel_tab(cx)),
        crate::shell::intel::PanelTab::References
    );
    let listed = intel.clone();
    wait(cx, &shell, "the references", move |cx| {
        listed
            .read(cx)
            .references()
            .is_some_and(|list| !list.is_loading() && !list.is_reading())
    });
    let rows = |intel: &Entity<CodeIntel>, cx: &mut VisualTestContext| {
        intel.read_with(cx, |intel, _| {
            let list = intel.references().unwrap();
            list.shown()
                .into_iter()
                .flat_map(|(file, rows)| {
                    rows.into_iter().map(move |(row, changed)| {
                        (
                            file.label().to_string(),
                            row.line,
                            row.text.clone().unwrap_or_default(),
                            changed,
                        )
                    })
                })
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(
        rows(&intel, cx),
        [
            (
                "src/a.ts".to_string(),
                1,
                "import { helper } from './b';".to_string(),
                false
            ),
            (
                "src/a.ts".into(),
                2,
                "export const one = helper(0);".into(),
                true
            ),
            (
                "src/b.ts".into(),
                1,
                "export function helper(x: number): number {".into(),
                false
            ),
        ],
        "grouped by file in path order, the changed line marked"
    );
    intel.update(cx, |intel, cx| intel.set_in_review(true, cx));
    assert_eq!(
        rows(&intel, cx),
        [(
            "src/a.ts".to_string(),
            2,
            "export const one = helper(0);".to_string(),
            true
        )]
    );
    intel.update(cx, |intel, cx| intel.close_references(cx));
    assert_eq!(
        shell.read_with(cx, |shell, cx| shell.panel_tab(cx)),
        crate::shell::intel::PanelTab::Review
    );
}

#[gpui::test]
fn a_crash_restarts_and_too_many_fail_until_retry(cx: &mut TestAppContext) {
    let repo = project();
    let (_dir, shell, cx, fakes) = open_review(&repo, Some(Arc::new(|_| None)), cx);
    wait(cx, &shell, "the server", |_| true);
    assert_eq!(fakes.starts(), 1);
    for (crash, starts) in [(1, 2), (2, 3), (3, 4)] {
        fakes.crash();
        let made = fakes.clone();
        wait(cx, &shell, "the restart", move |_| made.starts() == starts);
        wait(cx, &shell, "it ready again", |_| true);
        assert_eq!(
            servers(&shell, cx),
            [("typescript".into(), "ready".into())],
            "crash {crash}"
        );
    }
    // The fourth in a row: failed, until Retry.
    fakes.crash();
    let shown = intel(&shell, cx);
    wait(cx, &shell, "failed", move |cx| {
        shown
            .read(cx)
            .servers(cx)
            .iter()
            .any(|server| matches!(server.state, ServerState::Failed(_)))
    });
    assert_eq!(fakes.starts(), 4);
    let intel = intel(&shell, cx);
    let id = intel.read_with(cx, |intel, cx| intel.servers(cx)[0].id);
    intel.update(cx, |intel, cx| intel.retry(id, cx));
    let made = fakes.clone();
    wait(cx, &shell, "the retry", move |_| made.starts() == 5);
    assert_eq!(servers(&shell, cx), [("typescript".into(), "ready".into())]);
}

#[gpui::test]
fn without_its_override_a_server_is_missing_in_a_test(cx: &mut TestAppContext) {
    // Tests (and runs under automation) find servers only through DV_LSP_<ID>.
    let repo = project();
    let (_dir, shell, cx, fakes) = open_review(&repo, None, cx);
    wait(cx, &shell, "the resolution", |_| true);
    assert!(servers(&shell, cx).is_empty());
    assert_eq!(fakes.starts(), 0);
    let intel = intel(&shell, cx);
    let missing = intel.read_with(cx, |intel, cx| intel.missing(cx));
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].spec_id, "typescript");
    assert_eq!(
        missing[0].reason,
        "DV_LSP_TYPESCRIPT isn't set, and automated runs find language servers only through it"
    );
    // An install (a package.json changing) resolves again, and it is still missing.
    let servers = cx.update(|_, cx| cx.services().language_servers.clone());
    let worktree = intel.read_with(cx, |intel, _| intel.worktree().clone());
    servers.update(cx, |servers, cx| {
        servers.files_changed(&worktree, &["package.json".to_string()], false, cx)
    });
    wait(cx, &shell, "the resolution again", |_| true);
    assert_eq!(intel.read_with(cx, |intel, cx| intel.missing(cx)).len(), 1);
}

#[gpui::test]
fn the_servers_stop_after_the_linger_once_no_review_uses_them(cx: &mut TestAppContext) {
    let repo = project();
    let (_dir, shell, cx, fakes) = open_review(&repo, Some(Arc::new(|_| None)), cx);
    wait(cx, &shell, "the server", |_| true);
    let servers = cx.update(|_, cx| cx.services().language_servers.clone());
    let worktree = intel(&shell, cx).read_with(cx, |intel, _| intel.worktree().clone());
    servers.update(cx, |servers, _| {
        servers.set_linger(Duration::from_millis(500))
    });
    // The review goes: its intel with it.
    shell.update(cx, |shell, cx| shell.close_review_for_tests(cx));
    cx.run_until_parked();
    assert!(servers.read_with(cx, |servers, _| servers.has_worktree(&worktree)));
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert!(!servers.read_with(cx, |servers, _| servers.has_worktree(&worktree)));
    // Stopped, not crashed: shutdown and exit, and no restart.
    let deadline = Instant::now() + Duration::from_secs(5);
    while fakes.with_method("exit").is_empty() {
        assert!(Instant::now() < deadline, "the server was never stopped");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(fakes.with_method("shutdown").len(), 1);
    assert_eq!(fakes.starts(), 1);
}

#[test]
fn identifiers_are_found_at_and_after_a_word() {
    let text = "let $el = foo_bar(42) + café;";
    assert_eq!(identifier_at(text, 4), Some(4..7), "a `$` is part of it");
    assert_eq!(identifier_at(text, 7), Some(4..7), "just after it");
    assert_eq!(identifier_at(text, 12), Some(10..17));
    assert_eq!(identifier_at(text, 18), None, "a number isn't a symbol");
    assert_eq!(identifier_at(text, 9), None, "between words");
    let cafe = text.find("café").unwrap();
    assert_eq!(
        identifier_at(text, cafe + 3),
        Some(cafe..cafe + "café".len())
    );
    assert_eq!(identifier_at("", 0), None);
}

#[gpui::test]
fn the_keys_at_the_caret_ask_about_its_symbol(cx: &mut TestAppContext) {
    let repo = project();
    let root = WorkspacePaths::new(&repo.path().display().to_string()).uri_text("");
    let answer: Answer = Arc::new(move |message| match message["method"].as_str() {
        Some("textDocument/hover") => Some(json!({"contents": "helper"})),
        Some("textDocument/references") => Some(at(&root, json!([location("src/b.ts", 0, 16)]))),
        _ => None,
    });
    let (_dir, shell, cx, fakes) = open_review(&repo, Some(answer), cx);
    wait(cx, &shell, "the server", |_| true);
    let stream = shell.read_with(cx, |shell, _| shell.diff_pane().clone());
    // The caret after `helper`'s last letter on line 2 still names it.
    stream.update(cx, |pane, cx| {
        pane.show_place("src/a.ts", AnchorSide::New, 2, 25, cx)
    });
    cx.simulate_keystrokes("secondary-i");
    let intel = intel(&shell, cx);
    let card = intel.clone();
    wait(cx, &shell, "the hover", move |cx| {
        card.read(cx).hover_card().is_some()
    });
    let symbol = intel.read_with(cx, |intel, _| intel.hover_card().unwrap().at.clone());
    assert_eq!(
        (symbol.name.as_str(), symbol.line, symbol.range),
        ("helper", 2, 19..25)
    );
    // Esc closes the card before anything else in the stream's chain.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(intel.read_with(cx, |intel, _| intel.hover_card().is_none()));
    assert!(
        stream.read_with(cx, |pane, _| pane.caret().is_some()),
        "the caret stays"
    );
    cx.simulate_keystrokes("shift-f12");
    let listed = intel.clone();
    wait(cx, &shell, "the references", move |cx| {
        listed
            .read(cx)
            .references()
            .is_some_and(|list| !list.is_loading())
    });
    assert_eq!(
        intel.read_with(cx, |intel, _| intel.references().unwrap().count()),
        1
    );
    assert_eq!(fakes.with_method("textDocument/references").len(), 1);
    assert_eq!(
        fakes.with_method("textDocument/references")[0]["params"]["context"],
        json!({"includeDeclaration": true})
    );
    // A caret on no symbol asks nothing.
    stream.update(cx, |pane, cx| {
        pane.show_place("src/a.ts", AnchorSide::New, 2, 17, cx)
    });
    cx.simulate_keystrokes("f12");
    wait(cx, &shell, "nothing", |_| true);
    assert!(fakes.with_method("textDocument/definition").is_empty());
}

#[gpui::test]
fn another_sides_text_is_closed_once_answered_and_its_definition_stays_on_that_side(
    cx: &mut TestAppContext,
) {
    let repo = project();
    let root = WorkspacePaths::new(&repo.path().display().to_string()).uri_text("");
    let answer: Answer = Arc::new(move |message| match message["method"].as_str() {
        Some("textDocument/hover") => Some(json!({"contents": "helper"})),
        // Line 1 of the file asked about, which is the old side's text there.
        Some("textDocument/definition") => Some(at(&root, json!([location("src/a.ts", 1, 13)]))),
        _ => None,
    });
    let (_dir, shell, cx, fakes) = open_review(&repo, Some(answer), cx);
    wait(cx, &shell, "the server", |_| true);
    let intel = intel(&shell, cx);
    let (at_new, diff) = symbol(&shell, "src/a.ts", 1, "helper", cx);
    let at_old = SymbolAt {
        side: AnchorSide::Old,
        ..at_new.clone()
    };
    intel.update(cx, |intel, cx| {
        intel.hover(at_old.clone(), Pane::Stream, Default::default(), &diff, cx)
    });
    let card = intel.clone();
    wait(cx, &shell, "the hover", move |cx| {
        card.read(cx).hover_card().is_some()
    });
    assert!(intel.read_with(cx, |intel, _| intel.hover_card().unwrap().against_worktree));
    // The old side's text was opened at the path, then closed once answered.
    let opened = fakes.with_method("textDocument/didOpen");
    assert_eq!(
        opened[0]["params"]["textDocument"]["text"],
        "import { helper } from './b';\nexport const one = 1;\n"
    );
    assert_eq!(fakes.with_method("textDocument/didClose").len(), 1);
    // Its definition in the same file is on the old side, where it was asked.
    intel.update(cx, |intel, cx| {
        intel.definition(at_old, Pane::Stream, &diff, cx)
    });
    let stream = shell.read_with(cx, |shell, _| shell.diff_pane().clone());
    let shown = stream.clone();
    wait(cx, &shell, "the caret", move |cx| {
        shown.read(cx).caret().is_some()
    });
    let caret = stream.read_with(cx, |pane, _| pane.caret()).unwrap();
    assert_eq!(
        (caret.1.side, caret.1.line, caret.1.column),
        (AnchorSide::Old, 2, 14)
    );
    assert!(!shell.read_with(cx, |shell, _| shell.is_file_view_open()));
    // The new side's text (the working tree's) stays open: its file reads from disk alike.
    intel.update(cx, |intel, cx| {
        intel.hover(at_new, Pane::Stream, Default::default(), &diff, cx)
    });
    wait(cx, &shell, "the second hover", |_| true);
    assert_eq!(fakes.with_method("textDocument/didClose").len(), 2);
    let closes = fakes.with_method("textDocument/didClose").len();
    // The agent edits it: the open document is closed, so the next request reads it afresh.
    let servers = cx.update(|_, cx| cx.services().language_servers.clone());
    let worktree = intel.read_with(cx, |intel, _| intel.worktree().clone());
    servers.update(cx, |servers, cx| {
        servers.files_changed(&worktree, &["src/a.ts".to_string()], false, cx)
    });
    wait(cx, &shell, "the change", |_| true);
    let deadline = Instant::now() + Duration::from_secs(5);
    while fakes.with_method("textDocument/didClose").len() == closes {
        assert!(
            Instant::now() < deadline,
            "the edited document was never closed"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui::test]
fn a_staged_reviews_definition_opens_the_working_trees_file(cx: &mut TestAppContext) {
    let repo = project();
    repo.git(&["add", "src/a.ts"]);
    let root = WorkspacePaths::new(&repo.path().display().to_string()).uri_text("");
    let answer: Answer = Arc::new(move |message| {
        (message["method"] == "textDocument/definition")
            .then(|| at(&root, json!([location("src/a.ts", 1, 13)])))
    });
    let (_dir, shell, cx, _fakes) = open_target(&repo, Some(answer), Target::Staged, cx);
    wait(cx, &shell, "the server", |_| true);
    let intel = intel(&shell, cx);
    // From the stream's new side (the index): another text than the disk's, so the server
    // read it as opened; a place in the same file is on that side.
    let (at, diff) = symbol(&shell, "src/a.ts", 2, "helper", cx);
    assert!(!intel.read_with(cx, |intel, cx| intel.reads_worktree(&at, cx)));
    intel.update(cx, |intel, cx| {
        intel.definition(at, Pane::Stream, &diff, cx)
    });
    wait(cx, &shell, "the place", |_| true);
    assert!(!shell.read_with(cx, |shell, _| shell.is_file_view_open()));
    // A place in another file is the working tree's, as it stands: the file view reads it so.
    let location = dv_lsp::SymbolLocation {
        target: dv_lsp::UriTarget::Worktree("src/b.ts".into()),
        range: dv_lsp::TextRange {
            start: dv_lsp::TextPosition::new(0, 16),
            end: dv_lsp::TextPosition::new(0, 22),
        },
    };
    intel.update(cx, |intel, cx| intel.go_to(location, None, cx));
    let view = shell.read_with(cx, |shell, _| shell.file_view().clone());
    let shown = view.clone();
    wait(cx, &shell, "the file view", move |cx| {
        shown
            .read(cx)
            .file_view_summary(cx)
            .is_some_and(|summary| !summary.loading)
    });
    let summary = view
        .read_with(cx, |view, cx| view.file_view_summary(cx))
        .unwrap();
    assert_eq!(
        (summary.path.as_str(), summary.worktree),
        ("src/b.ts", true)
    );
    assert_eq!(summary.line_count, 3);
}

#[gpui::test]
fn a_definition_answered_after_the_reviewer_moved_on_is_dropped(cx: &mut TestAppContext) {
    let repo = project();
    let root = WorkspacePaths::new(&repo.path().display().to_string()).uri_text("");
    let answer: Answer = Arc::new(move |message| {
        (message["method"] == "textDocument/definition")
            .then(|| at(&root, json!([location("src/b.ts", 0, 16)])))
    });
    let (_dir, shell, cx, _fakes) = open_review(&repo, Some(answer), cx);
    wait(cx, &shell, "the server", |_| true);
    let intel = intel(&shell, cx);
    let stream = shell.read_with(cx, |shell, _| shell.diff_pane().clone());
    stream.update(cx, |pane, cx| {
        pane.show_place("src/a.ts", AnchorSide::New, 2, 19, cx)
    });
    let (at, diff) = symbol(&shell, "src/a.ts", 2, "helper", cx);
    intel.update(cx, |intel, cx| {
        intel.definition(at, Pane::Stream, &diff, cx)
    });
    // Before it answers, the reviewer goes to another line.
    stream.update(cx, |pane, cx| {
        pane.show_place("src/a.ts", AnchorSide::New, 1, 0, cx)
    });
    wait(cx, &shell, "the answer", |_| true);
    assert_eq!(
        intel.read_with(cx, |intel, _| intel.last_definitions()),
        Some(1)
    );
    assert!(!shell.read_with(cx, |shell, _| shell.is_file_view_open()));
}
