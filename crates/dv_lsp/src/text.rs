//! Positions in a document, as dv speaks them and as a server does (docs/PLAN.md §18.4).
//!
//! dv's views place a caret at a line and a UTF-8 byte offset into it ([`TextPosition`]). A
//! server counts columns in the encoding it chose at `initialize` ([`PositionEncoding`]): UTF-8
//! is dv's own and needs nothing; UTF-16, the protocol's default for a server that chooses
//! nothing, is converted with the line's text.

use serde::{Deserialize, Serialize};

/// A place in a document: a 0-based line and a byte offset into the line's UTF-8 text.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct TextPosition {
    pub line: u32,
    pub column: u32,
}

impl TextPosition {
    pub fn new(line: u32, column: u32) -> Self {
        Self { line, column }
    }
}

/// A span of a document, `end` exclusive.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct TextRange {
    pub start: TextPosition,
    pub end: TextPosition,
}

/// How a server counts columns, from its `capabilities.positionEncoding`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PositionEncoding {
    Utf8,
    /// Also what a server that names no encoding, or one dv didn't offer, uses.
    Utf16,
}

impl PositionEncoding {
    /// The encoding a server chose; absent, or anything dv didn't offer, is UTF-16.
    pub fn from_capability(kind: Option<&str>) -> Self {
        match kind {
            Some("utf-8") => Self::Utf8,
            _ => Self::Utf16,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Utf16 => "utf-16",
        }
    }

    /// A byte offset into `line` as this encoding's column. An offset inside a character counts
    /// as that character's start; one past the end as the end.
    pub fn column_to_server(self, line: &str, byte: u32) -> u32 {
        match self {
            Self::Utf8 => byte,
            Self::Utf16 => {
                let mut end = (byte as usize).min(line.len());
                while !line.is_char_boundary(end) {
                    end -= 1;
                }
                let units: usize = line[..end].chars().map(char::len_utf16).sum();
                u32::try_from(units).unwrap_or(u32::MAX)
            }
        }
    }

    /// This encoding's column in `line` as a byte offset. A column inside a character (half a
    /// surrogate pair) counts as that character's start; one past the end as the end.
    pub fn column_from_server(self, line: &str, column: u32) -> u32 {
        match self {
            Self::Utf8 => column,
            Self::Utf16 => {
                let mut units = 0u32;
                for (offset, ch) in line.char_indices() {
                    let next = units + ch.len_utf16() as u32;
                    if next > column {
                        return offset as u32;
                    }
                    units = next;
                }
                line.len() as u32
            }
        }
    }
}

/// Where each line of a text is, split as LSP splits lines: at `\r\n`, `\n` or a lone `\r`.
/// Built once per text, so converting many positions in one file reads it once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineIndex {
    /// Each line's bytes, without its ending.
    spans: Vec<std::ops::Range<usize>>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let bytes = text.as_bytes();
        let mut spans = Vec::new();
        let mut start = 0;
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => {
                    spans.push(start..i);
                    start = i + 1;
                }
                b'\r' => {
                    spans.push(start..i);
                    if bytes.get(i + 1) == Some(&b'\n') {
                        i += 1;
                    }
                    start = i + 1;
                }
                _ => {}
            }
            i += 1;
        }
        spans.push(start..bytes.len());
        Self { spans }
    }

    /// Line `n` (0-based) of `text`, the text this index was built from, without its ending;
    /// `None` past the last line.
    pub fn line<'t>(&self, text: &'t str, n: u32) -> Option<&'t str> {
        text.get(self.spans.get(n as usize)?.clone())
    }

    /// How many lines there are (a text ending in a line ending has an empty last line).
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

/// Line `n` (0-based) of `text`, without its ending ([`LineIndex`]'s lines); `None` past the
/// last line.
pub fn line_of(text: &str, n: u32) -> Option<&str> {
    LineIndex::new(text).line(text, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_columns_pass_through() {
        let enc = PositionEncoding::Utf8;
        assert_eq!(enc.column_to_server("h\u{e9}llo", 3), 3);
        assert_eq!(enc.column_from_server("h\u{e9}llo", 3), 3);
    }

    #[test]
    fn utf16_columns_count_code_units() {
        let enc = PositionEncoding::Utf16;
        // "a" 1 byte / 1 unit, "é" 2 / 1, "😀" 4 / 2, "中" 3 / 1.
        let line = "a\u{e9}\u{1f600}\u{4e2d}x";
        let bytes = [0, 1, 3, 7, 10, 11];
        let units = [0, 1, 2, 4, 5, 6];
        for (byte, unit) in bytes.iter().zip(units) {
            assert_eq!(enc.column_to_server(line, *byte), unit, "byte {byte}");
            assert_eq!(enc.column_from_server(line, unit), *byte, "unit {unit}");
        }
    }

    #[test]
    fn utf16_columns_inside_a_character_or_past_the_end() {
        let enc = PositionEncoding::Utf16;
        let line = "\u{1f600}z";
        // Byte 2 is inside the emoji: its start.
        assert_eq!(enc.column_to_server(line, 2), 0);
        // Unit 1 is the emoji's low surrogate: its start.
        assert_eq!(enc.column_from_server(line, 1), 0);
        assert_eq!(enc.column_to_server(line, 99), 3);
        assert_eq!(enc.column_from_server(line, 99), 5);
    }

    #[test]
    fn the_capability_names_the_encoding() {
        assert_eq!(
            PositionEncoding::from_capability(Some("utf-8")),
            PositionEncoding::Utf8
        );
        assert_eq!(
            PositionEncoding::from_capability(Some("utf-16")),
            PositionEncoding::Utf16
        );
        assert_eq!(
            PositionEncoding::from_capability(None),
            PositionEncoding::Utf16
        );
        assert_eq!(
            PositionEncoding::from_capability(Some("utf-32")),
            PositionEncoding::Utf16
        );
    }

    #[test]
    fn lines_end_at_crlf_lf_and_a_lone_cr() {
        let text = "one\r\ntwo\nthree\rfour\r\r\n";
        let index = LineIndex::new(text);
        let lines: Vec<&str> = (0..index.len() as u32)
            .map(|n| index.line(text, n).unwrap())
            .collect();
        assert_eq!(lines, ["one", "two", "three", "four", "", ""]);
        assert_eq!(index.line(text, 6), None);
        assert_eq!(line_of("x", 0), Some("x"));
        assert_eq!(line_of("", 0), Some(""));
        assert_eq!(line_of("\u{e9}\r", 1), Some(""));
    }
}
