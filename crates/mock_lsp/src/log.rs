//! `mock_lsp`'s log: one JSON line per message it read (`{"pid", "received", "matched"}`), per
//! start (`{"pid", "started": [args]}`) and per crash (`"crashed": <exit code>`), so a test sees
//! what dv sent each server it started, and how many times one started.
//!
//! Several processes append to one log (a server that crashed and its restart), so each line is
//! one `write` to a file opened for appending, which the OS keeps whole.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use serde_json::Value;

pub struct Log {
    path: PathBuf,
}

impl Log {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn append(&self, entry: &Value) -> Result<()> {
        let mut line = entry.to_string();
        line.push('\n');
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| file.write_all(line.as_bytes()))
            .with_context(|| format!("appending to the log {}", self.path.display()))
    }

    /// How many entries pass `test`.
    pub fn count(&self, test: impl Fn(&Value) -> bool) -> Result<usize> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("reading the log {}", self.path.display()));
            }
        };
        Ok(text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|entry| test(entry))
            .count())
    }
}
