//! The call log `DV_MOCK_GH_LOG` names: one JSON line per call, `{"argv", "stdin", "matched"}`,
//! which tests and `gh_log` scenario lines assert on (docs/PLAN.md §14.2).
//!
//! Every call is a fresh `mock_gh` process, so the log is also what `times` counts against: a
//! call takes an exclusive lock on `<log>.lock`, counts the earlier calls each entry answered,
//! appends its own line with a single `write` on an append-mode handle, and releases the lock.
//! Concurrent calls therefore never hand out an entry's last answer twice. The lock lives in a
//! separate file because a Windows file lock is mandatory: locking the log itself would make a
//! reader's `read` fail while a call is in flight.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use serde::Serialize;
use serde_json::Value;

/// One line of the log.
#[derive(Debug, Serialize)]
pub struct Record<'a> {
    pub argv: &'a [String],
    /// What the call read from stdin; `null` when its arguments don't ask gh to read stdin.
    pub stdin: Option<&'a str>,
    /// The index of the scenario entry that answered, or `null` for an unmatched call.
    pub matched: Option<usize>,
}

/// The log file and its lock.
#[derive(Debug, Clone)]
pub struct CallLog {
    path: PathBuf,
}

/// Holds the log's lock until dropped (closing the handle releases it).
#[must_use = "the lock is released when this is dropped"]
pub struct Locked {
    _file: File,
}

impl CallLog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn lock_path(&self) -> PathBuf {
        let mut name = OsString::from(self.path.as_os_str());
        name.push(".lock");
        PathBuf::from(name)
    }

    /// Blocks until this process holds the log's lock.
    pub fn lock(&self) -> Result<Locked> {
        let path = self.lock_path();
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("opening the call log lock {}", path.display()))?;
        file.lock()
            .with_context(|| format!("locking {}", path.display()))?;
        Ok(Locked { _file: file })
    }

    /// How many logged calls each entry answered, by entry index. A log that doesn't exist yet
    /// has none.
    pub fn uses(&self) -> Result<HashMap<usize, u32>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => {
                return Err(err).with_context(|| format!("reading {}", self.path.display()));
            }
        };
        let mut uses = HashMap::new();
        for (number, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let record: Value = serde_json::from_str(line).with_context(|| {
                format!("{}:{}: not a call record", self.path.display(), number + 1)
            })?;
            if let Some(index) = record.get("matched").and_then(Value::as_u64) {
                let index = usize::try_from(index).context("entry index out of range")?;
                *uses.entry(index).or_insert(0) += 1;
            }
        }
        Ok(uses)
    }

    /// Appends `record` as one line, in one `write`.
    pub fn append(&self, record: &Record<'_>) -> Result<()> {
        let mut line = serde_json::to_string(record).context("encoding the call record")?;
        line.push('\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("opening the call log {}", self.path.display()))?;
        let written = file
            .write(line.as_bytes())
            .with_context(|| format!("writing to {}", self.path.display()))?;
        if written != line.len() {
            bail!(
                "short write to {}: {written} of {} bytes",
                self.path.display(),
                line.len()
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn records_append_as_json_lines_and_count_per_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("calls.jsonl");
        let log = CallLog::new(&path);
        assert_eq!(log.uses().unwrap(), HashMap::new());

        let argv = vec!["pr".to_string(), "view".to_string()];
        for matched in [Some(0), None, Some(2), Some(0)] {
            let _lock = log.lock().unwrap();
            log.append(&Record {
                argv: &argv,
                stdin: None,
                matched,
            })
            .unwrap();
        }
        log.append(&Record {
            argv: &argv,
            stdin: Some("body"),
            matched: Some(1),
        })
        .unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5);
        assert_eq!(
            lines[0],
            r#"{"argv":["pr","view"],"stdin":null,"matched":0}"#
        );
        assert_eq!(
            lines[4],
            r#"{"argv":["pr","view"],"stdin":"body","matched":1}"#
        );
        assert_eq!(log.uses().unwrap(), HashMap::from([(0, 2), (1, 1), (2, 1)]));
        assert!(dir.path().join("calls.jsonl.lock").is_file());
    }

    #[test]
    fn a_corrupt_log_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("calls.jsonl");
        std::fs::write(&path, "{\"matched\":0}\n\nnot json\n").unwrap();
        let err = format!("{:#}", CallLog::new(&path).uses().unwrap_err());
        assert!(err.contains("calls.jsonl:3: not a call record"), "{err}");
    }
}
