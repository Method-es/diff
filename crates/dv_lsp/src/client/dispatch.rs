//! What a server sends the client, handled on the server process's own thread (docs/PLAN.md
//! §18.4): answers to the client's requests, the server's own requests (answered at once), and
//! its notifications.

use std::sync::Arc;

use dv_core::backend::ProcessSink;
use serde_json::{Value, json};

use super::pending::ResponseError;
use super::watchers::Watcher;
use super::{ClientEvent, Progress, Shared, on_process_event};
use crate::framing::Decoder;

/// JSON-RPC's "method not found".
const METHOD_NOT_FOUND: i64 = -32601;

/// The process sink of a client: its stdout decoded into messages, its stderr kept, its exit
/// recorded.
pub(super) fn sink(shared: Arc<Shared>) -> ProcessSink {
    let mut decoder = Decoder::new();
    Box::new(move |event| on_process_event(&shared, &mut decoder, event))
}

/// One message body from the server.
pub(super) fn message(shared: &Shared, body: &[u8]) {
    let message: Value = match serde_json::from_slice(body) {
        Ok(message) => message,
        Err(err) => {
            log::warn!("a language server sent a message that isn't JSON: {err}");
            return;
        }
    };
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id").filter(|id| !id.is_null());
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match (method, id) {
        (Some(method), Some(id)) => server_request(shared, id.clone(), method, &params),
        (Some(method), None) => notification(shared, method, &params),
        (None, Some(id)) => response(shared, id, &message),
        (None, None) => log::debug!("a language server sent a message with neither id nor method"),
    }
}

/// An answer to one of the client's requests: handed to the request waiting for it, if any
/// still is.
fn response(shared: &Shared, id: &Value, message: &Value) {
    let Some(id) = id.as_i64() else {
        return;
    };
    let Some(waiting) = shared.state.lock().pending.remove(&id) else {
        // Cancelled, timed out or dropped: nobody wants it.
        return;
    };
    let reply = match message.get("error") {
        Some(error) => Err(ResponseError {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        }),
        None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
    };
    let _ = waiting.send(reply);
}

/// A request from the server, answered at once.
fn server_request(shared: &Shared, id: Value, method: &str, params: &Value) {
    let result = match method {
        "workspace/configuration" => {
            let items = params.get("items").and_then(Value::as_array);
            let answers: Vec<Value> = items
                .into_iter()
                .flatten()
                .map(|item| shared.configuration(item.get("section").and_then(Value::as_str)))
                .collect();
            Ok(Value::Array(answers))
        }
        "client/registerCapability" => {
            register(shared, params);
            Ok(Value::Null)
        }
        "client/unregisterCapability" => {
            // The protocol spells the field `unregisterations`.
            let ids: Vec<&str> = params
                .get("unregisterations")
                .or_else(|| params.get("unregistrations"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .collect();
            let mut state = shared.state.lock();
            state
                .watchers
                .retain(|watcher| !ids.contains(&watcher.registration.as_str()));
            state
                .registrations
                .retain(|(id, _)| !ids.contains(&id.as_str()));
            Ok(Value::Null)
        }
        // The token's progress arrives as `$/progress`; creating it needs nothing more.
        "window/workDoneProgress/create" => Ok(Value::Null),
        "workspace/workspaceFolders" => {
            let root = &shared.config.root;
            let absolute = shared.paths.absolute(root);
            Ok(json!([{
                "uri": shared.paths.uri_text(root),
                "name": super::root_name(&absolute),
            }]))
        }
        _ => {
            Err(json!({"code": METHOD_NOT_FOUND, "message": format!("dv doesn't handle {method}")}))
        }
    };
    let answer = match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    };
    shared.send(&answer);
}

/// Records `client/registerCapability`'s registrations, and the file watchers among them.
fn register(shared: &Shared, params: &Value) {
    let registrations = params.get("registrations").and_then(Value::as_array);
    let mut state = shared.state.lock();
    for registration in registrations.into_iter().flatten() {
        let (Some(id), Some(method)) = (
            registration.get("id").and_then(Value::as_str),
            registration.get("method").and_then(Value::as_str),
        ) else {
            continue;
        };
        state
            .registrations
            .push((id.to_string(), method.to_string()));
        if method == "workspace/didChangeWatchedFiles"
            && let Some(options) = registration.get("registerOptions")
        {
            let watchers = Watcher::from_options(id, options, &shared.paths);
            state.watchers.extend(watchers);
        }
    }
}

/// A notification from the server.
fn notification(shared: &Shared, method: &str, params: &Value) {
    match method {
        "$/progress" => progress(shared, params),
        "window/logMessage" | "window/showMessage" => {
            let text = params.get("message").and_then(Value::as_str).unwrap_or("");
            log::debug!("language server: {text}");
        }
        _ => {}
    }
}

/// `$/progress` with a work-done `begin`, `report` or `end`.
fn progress(shared: &Shared, params: &Value) {
    let token = match params.get("token") {
        Some(Value::String(token)) => token.clone(),
        Some(Value::Number(token)) => token.to_string(),
        _ => return,
    };
    let Some(value) = params.get("value") else {
        return;
    };
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    let percentage = value
        .get("percentage")
        .and_then(Value::as_u64)
        .map(|p| p.min(100) as u32);
    let now = {
        let mut state = shared.state.lock();
        let progress = &mut state.progress;
        match value.get("kind").and_then(Value::as_str) {
            Some("begin") => {
                progress.retain(|item| item.token != token);
                progress.push(Progress {
                    token,
                    title: text("title").unwrap_or_default(),
                    message: text("message"),
                    percentage,
                });
            }
            Some("report") => {
                let Some(item) = progress.iter_mut().find(|item| item.token == token) else {
                    return;
                };
                if let Some(message) = text("message") {
                    item.message = Some(message);
                }
                if percentage.is_some() {
                    item.percentage = percentage;
                }
            }
            Some("end") => progress.retain(|item| item.token != token),
            _ => return,
        }
        progress.clone()
    };
    shared.emit(ClientEvent::Progress(now));
}
