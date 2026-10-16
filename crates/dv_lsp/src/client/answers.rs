//! What the client's requests answer, in dv's terms: UTF-8 columns ([`crate::TextPosition`]),
//! worktree paths ([`UriTarget`]), and hover contents as Markdown or code (docs/PLAN.md §18.1).

use lsp_types::{HoverContents, MarkedString, MarkupKind, Range};
use serde::Deserialize;
use serde_json::Value;

use crate::paths::UriTarget;
use crate::text::TextRange;

/// A hover's answer: its parts in the server's order, and the span of the symbol it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    pub parts: Vec<HoverPart>,
    pub range: Option<TextRange>,
}

/// One part of a hover: TypeScript's is one Markdown part, a fenced signature then the docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoverPart {
    Markdown(String),
    /// Code in a named language (a signature).
    Code {
        language: String,
        text: String,
    },
    Plain(String),
}

impl HoverPart {
    /// Whether the part shows nothing.
    pub fn is_empty(&self) -> bool {
        match self {
            HoverPart::Markdown(text) | HoverPart::Plain(text) => text.trim().is_empty(),
            HoverPart::Code { text, .. } => text.trim().is_empty(),
        }
    }
}

/// A place a definition or a reference is: the file, and the span of the symbol there.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolLocation {
    pub target: UriTarget,
    pub range: TextRange,
}

/// A hover's contents as parts, the empty ones dropped.
pub(crate) fn hover_parts(contents: HoverContents) -> Vec<HoverPart> {
    let marked = |marked: MarkedString| match marked {
        MarkedString::String(text) => HoverPart::Markdown(text),
        MarkedString::LanguageString(code) => HoverPart::Code {
            language: code.language,
            text: code.value,
        },
    };
    let parts = match contents {
        HoverContents::Scalar(one) => vec![marked(one)],
        HoverContents::Array(many) => many.into_iter().map(marked).collect(),
        HoverContents::Markup(markup) => vec![match markup.kind {
            MarkupKind::Markdown => HoverPart::Markdown(markup.value),
            MarkupKind::PlainText => HoverPart::Plain(markup.value),
        }],
    };
    parts.into_iter().filter(|part| !part.is_empty()).collect()
}

/// A `Location` with its URI as text: one URI a server spelled badly (an unencoded space or
/// backslash, which `lsp_types::Uri` refuses) loses only its own entry.
#[derive(Deserialize)]
struct RawLocation {
    uri: String,
    range: Range,
}

/// A `LocationLink`, its URI as text likewise.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLink {
    target_uri: String,
    target_selection_range: Range,
}

/// A definition's or references' answer as `(uri, range)` pairs: `null`, one location, or a
/// list of locations or links (a link as its target's selection, the symbol's name rather than
/// its whole declaration). An entry that doesn't parse is dropped, and logged.
pub(crate) fn location_entries(method: &str, value: Value) -> Vec<(String, Range)> {
    let entries = match value {
        Value::Null => return Vec::new(),
        Value::Array(entries) => entries,
        one => vec![one],
    };
    entries
        .into_iter()
        .filter_map(|entry| {
            let parsed = if entry.get("targetUri").is_some() {
                serde_json::from_value::<RawLink>(entry.clone())
                    .map(|link| (link.target_uri, link.target_selection_range))
            } else {
                serde_json::from_value::<RawLocation>(entry.clone())
                    .map(|location| (location.uri, location.range))
            };
            match parsed {
                Ok(pair) => Some(pair),
                Err(err) => {
                    log::warn!("{method} answered a location dv can't read ({err}): {entry}");
                    None
                }
            }
        })
        .collect()
}
