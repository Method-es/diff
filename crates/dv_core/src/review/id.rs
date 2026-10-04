//! Review ids (docs/PLAN.md §7): a one-letter kind, a dash and a ULID in lowercase Crockford
//! base32 — `r-` reviews, `t-` threads, `c-` comments (replies), `e-` events.
//!
//! A ULID starts with its creation time, so ids sort by age (the review list is newest first)
//! and are unique without coordination between the GUI, the CLI and agents writing at once
//! (PLAN D5). A review id becomes a store file name (`reviews/<id>.jsonl`), so every id is
//! checked by a strict parser before anything builds a path from it: exactly the kind letter,
//! `-` and 26 characters of the lowercase Crockford alphabet, the first of them `0`–`7` (a
//! ULID is 128 bits; a larger first digit would overflow). `../x`, uppercase and a wrong kind
//! letter are all refused.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The length of a ULID in base32 digits.
const ULID_LEN: usize = 26;

/// Why a string isn't an id of the expected kind.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind} id {text:?}: {reason}")]
pub struct InvalidId {
    /// What was expected (`review`, `thread`, …).
    pub kind: &'static str,
    pub text: String,
    pub reason: &'static str,
}

/// Whether `b` is a digit of lowercase Crockford base32 (`0-9a-z` without `i`, `l`, `o`, `u`).
fn is_crockford(b: u8) -> bool {
    matches!(b, b'0'..=b'9' | b'a'..=b'h' | b'j' | b'k' | b'm' | b'n' | b'p'..=b't' | b'v'..=b'z')
}

/// Checks `text` is `<prefix>-<ulid>` (module docs).
fn check(text: &str, prefix: char, kind: &'static str) -> Result<(), InvalidId> {
    let invalid = |reason| InvalidId {
        kind,
        text: text.to_string(),
        reason,
    };
    let bytes = text.as_bytes();
    if bytes.len() != 2 + ULID_LEN {
        return Err(invalid("expected a kind letter, '-' and 26 characters"));
    }
    if char::from(bytes[0]) != prefix || bytes[1] != b'-' {
        return Err(invalid("wrong kind prefix"));
    }
    let ulid = &bytes[2..];
    if !ulid.iter().all(|&b| is_crockford(b)) {
        return Err(invalid("not a lowercase Crockford base32 ULID"));
    }
    if ulid[0] > b'7' {
        return Err(invalid("out of the ULID range"));
    }
    Ok(())
}

/// A fresh `<prefix>-<ulid>`, timestamped now.
fn generate(prefix: char) -> String {
    let ulid = ulid::Ulid::generate().to_string().to_ascii_lowercase();
    format!("{prefix}-{ulid}")
}

/// Defines one id kind: a string newtype that only holds valid ids, serialised as the string.
macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident, $prefix:literal, $kind:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// The kind letter every id of this kind starts with.
            pub const PREFIX: char = $prefix;

            /// A new, unique id.
            pub fn generate() -> Self {
                Self(generate($prefix))
            }

            /// `text` as an id of this kind, when it is one (module docs).
            pub fn parse(text: &str) -> Result<Self, InvalidId> {
                check(text, $prefix, $kind)?;
                Ok(Self(text.to_string()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = InvalidId;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                Self::parse(text)
            }
        }

        impl TryFrom<String> for $name {
            type Error = InvalidId;

            fn try_from(text: String) -> Result<Self, Self::Error> {
                check(&text, $prefix, $kind)?;
                Ok(Self(text))
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.0
            }
        }
    };
}

id_type!(
    /// A review's id, `r-<ulid>`; also its log's file name (`reviews/<id>.jsonl`, PLAN §7).
    ReviewId,
    'r',
    "review"
);
id_type!(
    /// A thread's id, `t-<ulid>` (PLAN §2.5). It also names the thread's opening comment.
    ThreadId,
    't',
    "thread"
);
id_type!(
    /// A reply's id, `c-<ulid>`.
    CommentId,
    'c',
    "comment"
);
id_type!(
    /// An event's id, `e-<ulid>`: unique per log line, so a line that lands twice is folded once
    /// (PLAN §7).
    EventId,
    'e',
    "event"
);

/// What `body_edited` and `deleted` events point at: a thread (its opening comment) or a reply.
///
/// Serialised as the id string; the kind letter tells which.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum ItemId {
    Thread(ThreadId),
    Comment(CommentId),
}

impl ItemId {
    /// `text` as a thread or comment id.
    pub fn parse(text: &str) -> Result<Self, InvalidId> {
        match text.as_bytes().first() {
            Some(b't') => ThreadId::parse(text).map(Self::Thread),
            Some(b'c') => CommentId::parse(text).map(Self::Comment),
            _ => Err(InvalidId {
                kind: "thread or comment",
                text: text.to_string(),
                reason: "expected a t- or c- id",
            }),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Thread(id) => id.as_str(),
            Self::Comment(id) => id.as_str(),
        }
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ItemId {
    type Err = InvalidId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl TryFrom<String> for ItemId {
    type Error = InvalidId;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text)
    }
}

impl From<ItemId> for String {
    fn from(id: ItemId) -> Self {
        match id {
            ItemId::Thread(id) => id.into(),
            ItemId::Comment(id) => id.into(),
        }
    }
}

impl From<ThreadId> for ItemId {
    fn from(id: ThreadId) -> Self {
        Self::Thread(id)
    }
}

impl From<CommentId> for ItemId {
    fn from(id: CommentId) -> Self {
        Self::Comment(id)
    }
}
