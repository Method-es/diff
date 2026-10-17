//! `mock_lsp`: a scripted language server for dv's tests and e2e scenarios (docs/PLAN.md §18.7).
//! dv runs whatever `DV_LSP_<ID>` names in place of a spec's server, and tests point it here,
//! so hover, definition, references, progress, a crash and its restart are covered with no
//! bun, Node or TypeScript on the machine.
//!
//! gpui: **never**; `script/check` fails when gpui appears in its dependency tree (PLAN §13.2).
//!
//! It speaks LSP over stdin and stdout (Content-Length framing):
//! 1. the scenario is `--scenario <path>`, else the file `DV_MOCK_LSP` names (`scenario.rs`);
//!    without either it plays the default: every request answered `null`. Other arguments
//!    (`--lsp --stdio`, a spec's own) are ignored;
//! 2. every message it reads is appended to the log, `--log <path>` else `DV_MOCK_LSP_LOG`, if
//!    set, as one JSON line (`log.rs`) before it is answered, and so are its start and a crash;
//! 3. it prints the scenario's `stderr` lines, answers `initialize` with the scenario's
//!    capabilities, runs `on_initialized` when `initialized` arrives, answers each request from
//!    the scenario on a thread of its own (so a delayed answer doesn't hold up the next, and a
//!    `$/cancelRequest` that arrives during the delay is answered `RequestCancelled`), and
//!    exits on `exit` (0 after `shutdown`, else 1) or when stdin ends (likewise).

mod log;
mod scenario;

use std::collections::HashSet;
use std::io::{BufReader, Write as _};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::log::Log;
use crate::scenario::{Action, Scenario};

/// The scenario file, when `--scenario` doesn't name one.
const SCENARIO_VAR: &str = "DV_MOCK_LSP";
/// Where every message is appended, when `--log` doesn't name a file.
const LOG_VAR: &str = "DV_MOCK_LSP_LOG";

/// Exit code when the scenario can't be read or is invalid (`EX_DATAERR`).
const BAD_SCENARIO: i32 = 65;
/// JSON-RPC's `RequestCancelled`.
const REQUEST_CANCELLED: i64 = -32800;
/// JSON-RPC's `InvalidParams`.
const INVALID_PARAMS: i64 = -32602;

/// What the threads share.
struct Server {
    scenario: Scenario,
    log: Option<Log>,
    stdout: Mutex<std::io::Stdout>,
    /// Requests the client cancelled while their answer waited.
    cancelled: Mutex<HashSet<String>>,
    /// Answers given by each scenario entry.
    used: Mutex<Vec<u32>>,
    next_id: AtomicU64,
    /// `initialize`'s `rootUri`, which `${root}` in the scenario's answers stands for.
    root: Mutex<Option<String>>,
}

impl Server {
    fn write(&self, message: &Value) {
        let body = message.to_string();
        let mut stdout = self.stdout.lock().unwrap_or_else(|err| err.into_inner());
        // A client that stopped reading is gone; there is nobody to tell.
        let _ = write!(stdout, "Content-Length: {}\r\n\r\n{body}", body.len());
        let _ = stdout.flush();
    }

    /// `value` with `${root}` in every string replaced by `initialize`'s `rootUri`.
    fn templated(&self, value: &Value) -> Value {
        let root = self.root.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match root {
            Some(root) => template(value, &root),
            None => value.clone(),
        }
    }

    fn run(&self, actions: &[Action]) {
        for action in actions {
            match action {
                Action::Notify(message) => self.write(&json!({
                    "jsonrpc": "2.0", "method": message.method,
                    "params": self.templated(&message.params)
                })),
                Action::Request(message) => {
                    let id = format!("mock-{}", self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
                    self.write(&json!({
                        "jsonrpc": "2.0", "id": id, "method": message.method,
                        "params": self.templated(&message.params)
                    }));
                }
                Action::DelayMs(ms) => std::thread::sleep(Duration::from_millis(*ms)),
            }
        }
    }

    fn record(&self, entry: Value) {
        if let Some(log) = &self.log
            && let Err(err) = log.append(&entry)
        {
            eprintln!("mock_lsp: {err:#}");
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scenario_path = flag(&args, "--scenario").or_else(|| env_path(SCENARIO_VAR));
    let log = flag(&args, "--log")
        .or_else(|| env_path(LOG_VAR))
        .map(Log::new);
    let scenario = match &scenario_path {
        Some(path) => Scenario::load(path),
        None => Ok(Scenario::default()),
    };
    let scenario = match scenario {
        Ok(scenario) => scenario,
        Err(err) => {
            eprintln!("mock_lsp: {err:#}");
            std::process::exit(BAD_SCENARIO);
        }
    };
    let used = vec![0; scenario.requests.len()];
    let server = Arc::new(Server {
        scenario,
        log,
        stdout: Mutex::new(std::io::stdout()),
        cancelled: Mutex::new(HashSet::new()),
        used: Mutex::new(used),
        next_id: AtomicU64::new(0),
        root: Mutex::new(None),
    });
    server.record(json!({"pid": std::process::id(), "started": args}));
    for line in &server.scenario.stderr {
        eprintln!("{line}");
    }
    // `after_requests: 0`: a server that dies as it starts, before reading anything.
    if let Some(crash) = &server.scenario.crash
        && crash.after_requests == 0
        && crashes_left(&server, crash.times)
    {
        server.record(json!({"pid": std::process::id(), "crashed": crash.exit}));
        eprintln!("mock_lsp: crashing at start as the scenario says");
        std::process::exit(crash.exit);
    }
    let code = serve(&server);
    std::process::exit(code);
}

/// Reads and answers messages until `exit` or the end of stdin; the exit code.
fn serve(server: &Arc<Server>) -> i32 {
    let mut stdin = BufReader::new(std::io::stdin().lock());
    let mut shutdown = false;
    let mut requests = 0u32;
    loop {
        let message = match read_message(&mut stdin) {
            Ok(Some(message)) => message,
            Ok(None) => return if shutdown { 0 } else { 1 },
            Err(err) => {
                eprintln!("mock_lsp: {err:#}");
                return 1;
            }
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string);
        let id = message.get("id").cloned().filter(|id| !id.is_null());
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let pid = std::process::id();
        let (Some(method), Some(id)) = (method.clone(), id) else {
            server.record(json!({"pid": pid, "received": message}));
            match method.as_deref() {
                // Like TypeScript's `tsc`: an `exit` that carries params (even `null`) is
                // ignored, so a client that sends them never stops a server cleanly.
                Some("exit") if message.get("params").is_some() => {
                    eprintln!("mock_lsp: ignoring exit with params");
                }
                Some("exit") => return if shutdown { 0 } else { 1 },
                Some("initialized") => {
                    let server = server.clone();
                    std::thread::spawn(move || {
                        let actions = server.scenario.on_initialized.clone();
                        server.run(&actions);
                    });
                }
                Some("$/cancelRequest") => {
                    if let Some(id) = params.get("id") {
                        let mut cancelled =
                            server.cancelled.lock().unwrap_or_else(|e| e.into_inner());
                        cancelled.insert(id.to_string());
                    }
                }
                _ => {}
            }
            continue;
        };
        match method.as_str() {
            "initialize" => {
                server.record(json!({"pid": pid, "received": message}));
                if let Some(root) = params.get("rootUri").and_then(Value::as_str) {
                    *server.root.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(root.trim_end_matches('/').to_string());
                }
                std::thread::sleep(Duration::from_millis(server.scenario.initialize_delay_ms));
                server.write(&json!({"jsonrpc": "2.0", "id": id, "result": {
                    "capabilities": server.scenario.capabilities,
                    "serverInfo": {"name": "mock_lsp", "version": env!("CARGO_PKG_VERSION")}
                }}));
                continue;
            }
            // Like `tsc`: `shutdown` takes no params, and `null` ones are refused.
            "shutdown" if message.get("params").is_some() => {
                server.record(json!({"pid": pid, "received": message}));
                server.write(&json!({"jsonrpc": "2.0", "id": id, "error": {
                    "code": INVALID_PARAMS, "message": "expected no params, got null"
                }}));
                continue;
            }
            "shutdown" => {
                server.record(json!({"pid": pid, "received": message}));
                shutdown = true;
                server.write(&json!({"jsonrpc": "2.0", "id": id, "result": null}));
                continue;
            }
            _ => {}
        }
        requests += 1;
        if let Some(crash) = &server.scenario.crash
            && requests == crash.after_requests
            && crashes_left(server, crash.times)
        {
            server.record(json!({"pid": pid, "received": message, "crashed": crash.exit}));
            eprintln!("mock_lsp: crashing on {method} as the scenario says");
            return crash.exit;
        }
        let picked = {
            let mut used = server.used.lock().unwrap_or_else(|e| e.into_inner());
            let picked = server.scenario.pick(&method, &params, &used);
            if let Some(index) = picked {
                used[index] += 1;
            }
            picked
        };
        server.record(json!({"pid": pid, "received": message, "matched": picked}));
        let server = server.clone();
        std::thread::spawn(move || answer(&server, id, picked));
    }
}

/// Answers request `id` from entry `picked` (or `null`), unless it was cancelled meanwhile.
fn answer(server: &Server, id: Value, picked: Option<usize>) {
    let entry = picked.and_then(|index| server.scenario.requests.get(index));
    if let Some(entry) = entry {
        std::thread::sleep(Duration::from_millis(entry.delay_ms));
        server.run(&entry.send);
    }
    let cancelled = server
        .cancelled
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id.to_string());
    let reply = if cancelled {
        json!({"jsonrpc": "2.0", "id": id,
            "error": {"code": REQUEST_CANCELLED, "message": "cancelled"}})
    } else {
        match entry.and_then(|entry| entry.error.as_ref()) {
            Some(error) => json!({"jsonrpc": "2.0", "id": id, "error": server.templated(error)}),
            None => {
                let result = entry
                    .and_then(|entry| entry.result.as_ref())
                    .map(|result| server.templated(result));
                json!({"jsonrpc": "2.0", "id": id, "result": result})
            }
        }
    };
    server.write(&reply);
}

/// Whether the scenario's crash may happen again: `times` counts crashes over the whole log
/// (every process that wrote to it); without a log, or a `times`, it always may.
fn crashes_left(server: &Server, times: Option<u32>) -> bool {
    let (Some(times), Some(log)) = (times, &server.log) else {
        return times != Some(0);
    };
    match log.count(|entry| entry.get("crashed").is_some()) {
        Ok(count) => count < times as usize,
        Err(err) => {
            eprintln!("mock_lsp: {err:#}");
            false
        }
    }
}

/// `value` with `${root}` in every string (keys included) replaced by `root`, and `${parent}`
/// by the folder holding it (a file outside the repository, beside it).
fn template(value: &Value, root: &str) -> Value {
    let parent = root.rsplit_once('/').map_or(root, |(parent, _)| parent);
    let fill = |text: &str| text.replace("${root}", root).replace("${parent}", parent);
    match value {
        Value::String(text) => Value::String(fill(text)),
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| template(item, root)).collect())
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (fill(key), template(item, root)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The next message from `input`, `None` at its end.
fn read_message(input: &mut impl std::io::BufRead) -> Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = Some(value.trim().parse::<usize>().context("Content-Length")?);
        }
    }
    let Some(length) = length else {
        bail!("a header without Content-Length");
    };
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    Ok(Some(
        serde_json::from_slice(&body).context("a message that isn't JSON")?,
    ))
}

/// The value after `name` in the arguments.
fn flag(args: &[String], name: &str) -> Option<PathBuf> {
    let at = args.iter().position(|arg| arg == name)?;
    args.get(at + 1).map(PathBuf::from)
}

/// A non-empty path from the environment.
fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_read_from_their_frames() {
        let mut input: &[u8] =
            b"Content-Length: 8\r\n\r\n{\"a\": 1}content-length: 2\r\nContent-Type: x\r\n\r\n{}";
        let mut reader = BufReader::new(&mut input);
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({"a": 1})));
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({})));
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

    #[test]
    fn root_is_templated_into_answers() {
        let answer = json!({"uri": "${root}/src/a.ts", "n": [1, "${root}"], "${root}": true});
        assert_eq!(
            template(&answer, "file:///w"),
            json!({"uri": "file:///w/src/a.ts", "n": [1, "file:///w"], "file:///w": true})
        );
        assert_eq!(
            template(&json!("${parent}/lib/x.d.ts"), "file:///d/repos/web"),
            json!("file:///d/repos/lib/x.d.ts")
        );
    }

    #[test]
    fn flags_name_files() {
        let args: Vec<String> = ["--lsp", "--stdio", "--scenario", "s.json"]
            .map(String::from)
            .to_vec();
        assert_eq!(flag(&args, "--scenario"), Some(PathBuf::from("s.json")));
        assert_eq!(flag(&args, "--log"), None);
    }
}
