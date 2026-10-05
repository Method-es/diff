//! dv's own files in the data dir (`settings.json`, `known_repos.json`, `review_ui.json`, PLAN
//! §13.3): how they are read and written.
//!
//! The default data dir on Windows (`%APPDATA%\dv`) also holds the first implementation's files,
//! so every file is read leniently and never written over when dv couldn't read it: a file
//! that exists but can't be read, or isn't a JSON object, makes its service *unsaveable* for
//! the session, because writing would destroy whatever it holds (another version's data, or a
//! file another process has locked for a moment). Writes go through a temporary file and a
//! rename ([`write_atomically`]), in the background, ordered by generation so the last change
//! always wins ([`Writer`], [`JsonFile`]), and whatever is unwritten when the app quits is
//! written synchronously then.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{AppContext as _, Context, Subscription, Task};
use parking_lot::Mutex;
use serde_json::{Map, Value};

/// What [`read_object`] found at a path.
#[derive(Debug, PartialEq)]
pub enum ReadObject {
    /// No file: a fresh start, saved normally.
    Missing,
    /// A JSON object.
    Object(Map<String, Value>),
    /// A file dv can't use; the text says why. Its service doesn't save this session.
    Unreadable(String),
}

/// Reads the JSON object at `path` (see the module docs).
pub fn read_object(path: &Path) -> ReadObject {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return ReadObject::Missing,
        Err(error) => {
            return ReadObject::Unreadable(format!(
                "{} couldn't be read ({error}), so changes aren't saved this session",
                path.display()
            ));
        }
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(object)) => ReadObject::Object(object),
        _ => ReadObject::Unreadable(format!(
            "{} isn't a file dv can read, so it's left as it is and changes aren't saved this \
             session",
            path.display()
        )),
    }
}

/// Writes `contents` to `path` through a temporary file beside it and a rename, so readers see
/// the old file or the new one, never part of one.
pub fn write_atomically(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let written = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// Writes snapshots of a file, newest generation wins: a write older than one already on disk
/// is skipped.
#[derive(Clone)]
pub struct Writer {
    path: PathBuf,
    written: Arc<Mutex<u64>>,
}

impl Writer {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            written: Arc::new(Mutex::new(0)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The newest generation on disk.
    pub fn written(&self) -> u64 {
        *self.written.lock()
    }

    /// Writes `contents` as generation `generation`; `Ok(false)` when a newer one is already
    /// written.
    pub fn write(&self, generation: u64, contents: &str) -> std::io::Result<bool> {
        let mut written = self.written.lock();
        if generation <= *written {
            return Ok(false);
        }
        write_atomically(&self.path, contents.as_bytes())?;
        *written = generation;
        Ok(true)
    }
}

/// One data-dir file kept up to date in the background: [`JsonFile::save`] the file's new text
/// after every change; the newest text is written, at most one write runs at a time, and the
/// last text is written synchronously when the app quits. A file that was [`Unreadable`]
/// (`unsaveable`) is never written; its saves report why instead.
///
/// [`Unreadable`]: ReadObject::Unreadable
pub struct JsonFile {
    writer: Writer,
    /// Bumped by every save.
    generation: u64,
    /// The newest generation a background write has tried (written, skipped or failed).
    attempted: u64,
    /// The newest text asked for.
    latest: Option<Arc<str>>,
    saving: Option<Task<()>>,
    last_error: Option<String>,
    unsaveable: Option<String>,
    _quit: Subscription,
}

impl JsonFile {
    pub fn new(path: PathBuf, unsaveable: Option<String>, cx: &mut Context<Self>) -> Self {
        let quit = cx.on_app_quit(|this: &mut Self, _cx| {
            this.flush();
            async {}
        });
        Self {
            writer: Writer::new(path),
            generation: 0,
            attempted: 0,
            latest: None,
            saving: None,
            last_error: None,
            unsaveable,
            _quit: quit,
        }
    }

    pub fn path(&self) -> &Path {
        self.writer.path()
    }

    /// Why the last save failed (or why this file is never saved), until one succeeds.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// No save is waiting or running (a failed one counts as done): automation's `settled`.
    pub fn is_settled(&self) -> bool {
        self.saving.is_none() && self.attempted >= self.generation
    }

    /// Asks for `contents` to be the file's text.
    pub fn save(&mut self, contents: String, cx: &mut Context<Self>) {
        self.generation += 1;
        self.latest = Some(contents.into());
        self.start(cx);
    }

    /// Starts writing the newest text unless a write is running; that one starts the next
    /// when it ends, if anything changed meanwhile.
    fn start(&mut self, cx: &mut Context<Self>) {
        if self.saving.is_some() {
            return;
        }
        if let Some(reason) = &self.unsaveable {
            self.attempted = self.generation;
            self.last_error = Some(reason.clone());
            cx.notify();
            return;
        }
        let Some(contents) = self.latest.clone() else {
            return;
        };
        let (writer, generation) = (self.writer.clone(), self.generation);
        let write = cx.background_spawn(async move { writer.write(generation, &contents) });
        self.saving = Some(cx.spawn(async move |this, cx| {
            let result = write.await;
            this.update(cx, |this, cx| {
                this.saving = None;
                this.attempted = generation;
                match result {
                    Ok(_) => this.last_error = None,
                    Err(error) => {
                        log::warn!("couldn't save {}: {error}", this.writer.path().display());
                        this.last_error = Some(error.to_string());
                    }
                }
                if this.attempted < this.generation {
                    this.start(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Writes whatever isn't written yet, on this thread (the app is quitting).
    fn flush(&mut self) {
        if self.unsaveable.is_some() || self.writer.written() >= self.generation {
            return;
        }
        if let Some(contents) = &self.latest
            && let Err(error) = self.writer.write(self.generation, contents)
        {
            log::warn!("couldn't save {}: {error}", self.writer.path().display());
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn atomic_writes_replace_the_file_and_leave_no_temporary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");
        write_atomically(&path, b"{\"v\": 1}\n").unwrap();
        write_atomically(&path, b"{\"v\": 1, \"mode\": \"dark\"}\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"v\": 1, \"mode\": \"dark\"}\n"
        );
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["settings.json"]);
    }

    #[test]
    fn the_writer_never_writes_an_older_generation_over_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let writer = Writer::new(dir.path().join("settings.json"));
        assert!(writer.write(2, "new").unwrap());
        assert!(!writer.write(1, "old").unwrap());
        assert_eq!(std::fs::read_to_string(writer.path()).unwrap(), "new");
    }

    #[test]
    fn reading_tells_missing_objects_and_everything_else_apart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.json");
        assert_eq!(read_object(&path), ReadObject::Missing);
        std::fs::write(&path, "{\"a\": 1}").unwrap();
        let ReadObject::Object(object) = read_object(&path) else {
            panic!("an object");
        };
        assert_eq!(object["a"], 1);
        for text in ["", "[1]", "not json", "\"text\""] {
            std::fs::write(&path, text).unwrap();
            assert!(
                matches!(read_object(&path), ReadObject::Unreadable(_)),
                "{text:?}"
            );
        }
        // A directory where the file should be can't be read either.
        let folder = dir.path().join("folder.json");
        std::fs::create_dir(&folder).unwrap();
        assert!(matches!(read_object(&folder), ReadObject::Unreadable(_)));
    }

    #[gpui::test]
    fn saves_land_in_order_and_settle(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known.json");
        let file = cx.new(|cx| JsonFile::new(path.clone(), None, cx));
        file.update(cx, |file, cx| {
            file.save("one".into(), cx);
            file.save("two".into(), cx);
            assert!(!file.is_settled());
        });
        cx.run_until_parked();
        file.read_with(cx, |file, _| {
            assert!(file.is_settled());
            assert_eq!(file.last_error(), None);
        });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
    }

    #[gpui::test]
    fn an_unsaveable_file_is_never_written(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known.json");
        std::fs::write(&path, "kept").unwrap();
        let file = cx.new(|cx| JsonFile::new(path.clone(), Some("unreadable".into()), cx));
        file.update(cx, |file, cx| file.save("new".into(), cx));
        cx.run_until_parked();
        file.read_with(cx, |file, _| {
            assert!(file.is_settled());
            assert_eq!(file.last_error(), Some("unreadable"));
        });
        cx.update(|cx| cx.quit());
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "kept");
    }
}
