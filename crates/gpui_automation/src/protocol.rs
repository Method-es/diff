//! Wire format: one JSON object per line in, exactly one JSON line out per
//! request.
//!
//! Request: `{"id": <any>, "cmd": "<name>", "window": <name|index>, ...args}`.
//! `id` is echoed back verbatim (`null` when absent) and `window` optionally
//! addresses a window other than the selected one. Everything else is the
//! command's arguments.
//!
//! Reply: `{"id": .., "ok": true, "result": ..}` or
//! `{"id": .., "ok": false, "error": ".."}`. Besides replies the app emits
//! unsolicited events, currently only `{"event": "ready", ..}` at startup.

use std::fmt;

use serde_json::{Map, Value, json};

/// A decoded request line.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// Echoed in the reply; `Value::Null` when the request had none.
    pub id: Value,
    pub cmd: String,
    pub window: Option<WindowSelector>,
    /// The command's arguments: every field except `id`, `cmd` and `window`.
    pub args: Map<String, Value>,
}

/// Which window a command addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowSelector {
    /// Position in the window list (see the `windows` command).
    Index(usize),
    /// A name registered with [`crate::register_window`].
    Name(String),
}

impl WindowSelector {
    pub fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::String(name) if !name.is_empty() => Ok(Self::Name(name.clone())),
            Value::Number(n) => n
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .map(Self::Index)
                .ok_or_else(|| format!("window index must be a non-negative integer, got {n}")),
            other => Err(format!(
                "window must be a name or an index, got {}",
                crate::matcher::compact(other)
            )),
        }
    }
}

impl fmt::Display for WindowSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Index(index) => write!(f, "#{index}"),
            Self::Name(name) => write!(f, "{name:?}"),
        }
    }
}

/// A request that could not be decoded, with whatever `id` could be salvaged
/// so the driver can still correlate the error reply.
#[derive(Debug, Clone, PartialEq)]
pub struct BadRequest {
    pub id: Value,
    pub message: String,
}

pub fn parse_request(line: &str) -> Result<Request, BadRequest> {
    let value: Value = serde_json::from_str(line).map_err(|err| BadRequest {
        id: Value::Null,
        message: format!("invalid JSON: {err}"),
    })?;
    let Value::Object(mut args) = value else {
        return Err(BadRequest {
            id: Value::Null,
            message: "a request must be a JSON object".into(),
        });
    };
    let id = args.remove("id").unwrap_or(Value::Null);
    let bad = |message: String| BadRequest {
        id: id.clone(),
        message,
    };
    let cmd = match args.remove("cmd") {
        Some(Value::String(cmd)) if !cmd.is_empty() => cmd,
        Some(other) => {
            return Err(bad(format!(
                "cmd must be a non-empty string, got {}",
                crate::matcher::compact(&other)
            )));
        }
        None => return Err(bad("missing \"cmd\"".into())),
    };
    let window = match args.remove("window") {
        None | Some(Value::Null) => None,
        Some(value) => Some(WindowSelector::from_value(&value).map_err(bad)?),
    };
    Ok(Request {
        id,
        cmd,
        window,
        args,
    })
}

pub fn ok_reply(id: Value, result: Value) -> Value {
    json!({"id": id, "ok": true, "result": result})
}

pub fn error_reply(id: Value, error: &anyhow::Error) -> Value {
    json!({"id": id, "ok": false, "error": format!("{error:#}")})
}

pub fn event(name: &str, mut fields: Map<String, Value>) -> Value {
    fields.insert("event".into(), Value::String(name.into()));
    Value::Object(fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_id_cmd_window_and_args() {
        let request =
            parse_request(r#"{"id": 7, "cmd": "click", "window": "reader", "x": 1, "y": 2}"#)
                .unwrap();
        assert_eq!(request.id, json!(7));
        assert_eq!(request.cmd, "click");
        assert_eq!(request.window, Some(WindowSelector::Name("reader".into())));
        assert_eq!(Value::Object(request.args), json!({"x": 1, "y": 2}));
    }

    #[test]
    fn window_index_and_missing_id() {
        let request = parse_request(r#"{"cmd": "state", "window": 1}"#).unwrap();
        assert_eq!(request.id, Value::Null);
        assert_eq!(request.window, Some(WindowSelector::Index(1)));
        assert!(request.args.is_empty());
    }

    #[test]
    fn errors_keep_the_id_when_possible() {
        let err = parse_request(r#"{"id": "a"}"#).unwrap_err();
        assert_eq!(err.id, json!("a"));
        assert!(err.message.contains("missing \"cmd\""));

        let err = parse_request(r#"{"id": 3, "cmd": "state", "window": -1}"#).unwrap_err();
        assert_eq!(err.id, json!(3));
        assert!(err.message.contains("non-negative"), "{}", err.message);

        let err = parse_request(r#"{"id": 3, "cmd": 5}"#).unwrap_err();
        assert!(err.message.contains("cmd must be"));

        let err = parse_request("not json").unwrap_err();
        assert_eq!(err.id, Value::Null);
        assert!(err.message.starts_with("invalid JSON"));

        let err = parse_request("[1]").unwrap_err();
        assert!(err.message.contains("JSON object"));
    }

    #[test]
    fn replies_have_the_documented_shape() {
        assert_eq!(
            ok_reply(json!(1), json!({"a": 1})),
            json!({"id": 1, "ok": true, "result": {"a": 1}})
        );
        let error = anyhow::anyhow!("inner").context("outer");
        assert_eq!(
            error_reply(Value::Null, &error),
            json!({"id": null, "ok": false, "error": "outer: inner"})
        );
        assert_eq!(event("ready", Map::new()), json!({"event": "ready"}));
    }
}
