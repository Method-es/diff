//! The scenario `mock_lsp` plays (docs/PLAN.md §18.7): what `initialize` answers, the requests
//! it expects and their answers, what it sends by itself, and when it crashes.
//!
//! ```json
//! {
//!   "capabilities": {"positionEncoding": "utf-8", "hoverProvider": true},
//!   "initialize_delay_ms": 500,
//!   "stderr": ["mock_lsp starting"],
//!   "on_initialized": [
//!     {"request": {"method": "window/workDoneProgress/create", "params": {"token": "load"}}},
//!     {"notify": {"method": "$/progress", "params": {"token": "load", "value": {"kind": "begin", "title": "Loading"}}}},
//!     {"delay_ms": 300},
//!     {"notify": {"method": "$/progress", "params": {"token": "load", "value": {"kind": "end"}}}}
//!   ],
//!   "requests": [
//!     {"method": "textDocument/hover",
//!      "params": {"textDocument": {"uri": "*/src/a.ts"}, "position": {"line": 3}},
//!      "result": {"contents": {"kind": "markdown", "value": "```ts\nconst a: number\n```"}},
//!      "delay_ms": 100, "times": 1},
//!     {"method": "textDocument/definition",
//!      "result": [{"uri": "${root}/src/b.ts", "range": {"start": {"line": 0, "character": 13}, "end": {"line": 0, "character": 19}}}]},
//!     {"method": "textDocument/references", "error": {"code": -32603, "message": "no project"}}
//!   ],
//!   "crash": {"after_requests": 2, "exit": 3, "times": 1}
//! }
//! ```
//!
//! A request is answered by the first entry whose `method` is its method, whose `params` are a
//! subset of its params (objects key by key, arrays and numbers exactly, strings with `*` as
//! any run of characters) and that has answers left (`times`, unlimited when absent, counted
//! per process). It answers `result` (`null` when absent) or `error`, after `delay_ms`, after
//! sending its `send` actions; a request no entry matches is answered `null`. `${root}` in a
//! result, an error or a sent message's params is `initialize`'s `rootUri` (no trailing `/`),
//! so a scenario names the fixture's files. `initialize` answers `capabilities` (the default
//! below when absent) after `initialize_delay_ms`; `shutdown` answers `null`, and like
//! TypeScript's `tsc` refuses params (even `null`, `InvalidParams`) and ignores an `exit` that
//! carries any. `crash` exits with `exit` (default 1) on the `after_requests`-th request other
//! than those two, without answering it (`0`: at start, before reading anything), at most
//! `times` times over the whole log (every time without a log).

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    #[serde(default = "default_capabilities")]
    pub capabilities: Value,
    /// How long `initialize` takes to answer.
    #[serde(default)]
    pub initialize_delay_ms: u64,
    #[serde(default)]
    pub stderr: Vec<String>,
    #[serde(default)]
    pub on_initialized: Vec<Action>,
    #[serde(default)]
    pub requests: Vec<Entry>,
    pub crash: Option<Crash>,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            capabilities: default_capabilities(),
            initialize_delay_ms: 0,
            stderr: Vec::new(),
            on_initialized: Vec::new(),
            requests: Vec::new(),
            crash: None,
        }
    }
}

/// What `initialize` answers without a scenario's own: UTF-8 positions, hover, definition and
/// references.
fn default_capabilities() -> Value {
    json!({
        "positionEncoding": "utf-8",
        "textDocumentSync": 1,
        "hoverProvider": true,
        "definitionProvider": true,
        "referencesProvider": true
    })
}

/// One scripted answer.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<Value>,
    #[serde(default)]
    pub delay_ms: u64,
    pub times: Option<u32>,
    /// Sent before the answer (after the delay).
    #[serde(default)]
    pub send: Vec<Action>,
}

/// Something the server sends by itself.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum Action {
    /// A notification: `{"method", "params"}`.
    Notify(Message),
    /// A request of the server's own (ids `mock-1`, `mock-2`, …); the client's answer is
    /// logged.
    Request(Message),
    /// A pause.
    DelayMs(u64),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crash {
    pub after_requests: u32,
    #[serde(default = "one")]
    pub exit: i32,
    pub times: Option<u32>,
}

fn one() -> i32 {
    1
}

impl Scenario {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the scenario {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("in the scenario {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let scenario: Scenario = serde_json::from_str(text)?;
        for (index, entry) in scenario.requests.iter().enumerate() {
            if entry.result.is_some() && entry.error.is_some() {
                bail!("requests[{index}] has both a result and an error");
            }
        }
        Ok(scenario)
    }

    /// The entry answering `method` with `params`, given how many times each has answered.
    pub fn pick(&self, method: &str, params: &Value, used: &[u32]) -> Option<usize> {
        self.requests.iter().enumerate().position(|(index, entry)| {
            entry.method == method
                && entry
                    .params
                    .as_ref()
                    .is_none_or(|pattern| matches(pattern, params))
                && entry
                    .times
                    .is_none_or(|times| used.get(index).copied().unwrap_or(0) < times)
        })
    }
}

/// Whether `actual` has everything `pattern` has (module docs).
pub fn matches(pattern: &Value, actual: &Value) -> bool {
    match (pattern, actual) {
        (Value::Object(pattern), Value::Object(actual)) => pattern
            .iter()
            .all(|(key, value)| actual.get(key).is_some_and(|got| matches(value, got))),
        (Value::String(pattern), Value::String(actual)) => glob(pattern, actual),
        (pattern, actual) => pattern == actual,
    }
}

/// `*` in `pattern` matches any run of characters; everything else itself.
fn glob(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsets_and_globs() {
        let params = json!({
            "textDocument": {"uri": "file:///w/repo/src/a.ts"},
            "position": {"line": 3, "character": 7},
            "context": {"includeDeclaration": true}
        });
        assert!(matches(&json!({}), &params));
        assert!(matches(&json!({"position": {"line": 3}}), &params));
        assert!(matches(
            &json!({"textDocument": {"uri": "*/src/a.ts"}}),
            &params
        ));
        assert!(matches(
            &json!({"textDocument": {"uri": "file://*"}}),
            &params
        ));
        assert!(!matches(&json!({"position": {"line": 4}}), &params));
        assert!(!matches(
            &json!({"textDocument": {"uri": "*/b.ts"}}),
            &params
        ));
        assert!(!matches(&json!({"missing": 1}), &params));
        assert!(glob("a*b*c", "aXXbYc") && !glob("a*b*c", "aXXcYb") && glob("*", ""));
    }

    #[test]
    fn entries_are_picked_in_order_while_they_have_answers() {
        let scenario = Scenario::parse(
            r#"{"requests": [
                {"method": "textDocument/hover", "params": {"position": {"line": 1}}, "result": 1, "times": 1},
                {"method": "textDocument/hover", "result": 2}
            ]}"#,
        )
        .unwrap();
        let on_line = |line: u32| json!({"position": {"line": line}});
        assert_eq!(
            scenario.pick("textDocument/hover", &on_line(1), &[0, 0]),
            Some(0)
        );
        assert_eq!(
            scenario.pick("textDocument/hover", &on_line(1), &[1, 0]),
            Some(1)
        );
        assert_eq!(
            scenario.pick("textDocument/hover", &on_line(2), &[0, 0]),
            Some(1)
        );
        assert_eq!(
            scenario.pick("textDocument/definition", &on_line(1), &[0, 0]),
            None
        );
        assert!(
            Scenario::parse(r#"{"requests": [{"method": "x", "result": 1, "error": {}}]}"#)
                .is_err()
        );
        assert!(Scenario::parse(r#"{"request": []}"#).is_err());
    }
}
