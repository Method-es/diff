//! What the reviewer chose per review (DESIGN §2: split or unified, "remembered per review"),
//! kept in `<data dir>/review_ui.json` as `{"v": 1, "reviews": {"<review id>": {"view_mode":
//! "split" | "unified"}}}`.
//!
//! Read leniently like every data-dir file ([`super::persist`]): unknown keys, at the top and in
//! each review's entry, are kept and written back; a review's entry this version can't read is
//! kept as it was; a file that exists but isn't a JSON object this version can read is never
//! written over (the service is *unsaveable* for the session and says why).

use std::collections::BTreeMap;
use std::path::Path;

use dv_core::review::ReviewId;
use gpui::{App, AppContext as _, Context, Entity};
use serde_json::{Map, Value};

use super::persist::{JsonFile, ReadObject, read_object};

/// The file in the data dir.
pub const REVIEW_UI_FILE: &str = "review_ui.json";
/// The `"v"` this version writes.
pub const REVIEW_UI_VERSION: u64 = 1;

/// How the stream lays out a file's lines (DESIGN §2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ViewMode {
    /// Old and new side by side, the default.
    #[default]
    Split,
    /// One column, removed lines above the added ones.
    Unified,
}

impl ViewMode {
    /// `"split"` or `"unified"`, as the file and automation state spell it.
    pub fn key(self) -> &'static str {
        match self {
            Self::Split => "split",
            Self::Unified => "unified",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "split" => Some(Self::Split),
            "unified" => Some(Self::Unified),
            _ => None,
        }
    }

    /// The other mode (`S`).
    pub fn toggled(self) -> Self {
        match self {
            Self::Split => Self::Unified,
            Self::Unified => Self::Split,
        }
    }
}

/// The file's contents: each review's entry as read (unknown keys and all), and the top-level
/// keys this version doesn't know.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReviewUiFile {
    reviews: BTreeMap<String, Value>,
    extra: Map<String, Value>,
}

impl ReviewUiFile {
    /// Reads the file's object leniently: the entries, and a line for each thing that couldn't
    /// be read. `Err` when `reviews` isn't an object: not a file this version can read, so it
    /// is never written over.
    pub fn from_object(mut object: Map<String, Value>) -> Result<(Self, Vec<String>), String> {
        let mut problems = Vec::new();
        if let Some(v) = object.remove("v") {
            match v.as_u64() {
                Some(v) if v > REVIEW_UI_VERSION => problems.push(format!(
                    "written by a newer dv (v {v}); reading the entries this version knows"
                )),
                Some(_) => {}
                None => problems.push(format!("\"v\" is {v}, not a version number")),
            }
        }
        let reviews = match object.remove("reviews") {
            Some(Value::Object(reviews)) => reviews.into_iter().collect(),
            None => BTreeMap::new(),
            Some(other) => return Err(format!("\"reviews\" is {other}, not an object")),
        };
        Ok((
            Self {
                reviews,
                extra: object,
            },
            problems,
        ))
    }

    /// The view mode stored for `review`; `None` when there is none this version can read.
    pub fn view_mode(&self, review: &ReviewId) -> Option<ViewMode> {
        self.reviews
            .get(review.as_str())?
            .get("view_mode")?
            .as_str()
            .and_then(ViewMode::from_key)
    }

    /// Stores `mode` for `review`, keeping the entry's other keys; `false` when it was stored
    /// already. An entry that isn't an object is replaced.
    pub fn set_view_mode(&mut self, review: &ReviewId, mode: ViewMode) -> bool {
        if self.view_mode(review) == Some(mode) {
            return false;
        }
        let entry = self
            .reviews
            .entry(review.as_str().to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        if let Value::Object(entry) = entry {
            entry.insert("view_mode".into(), mode.key().into());
        }
        true
    }

    /// The file's text: `v` and `reviews` first, then the kept keys; pretty JSON with a trailing
    /// newline.
    pub fn to_json(&self) -> String {
        let mut object = Map::new();
        object.insert("v".into(), REVIEW_UI_VERSION.into());
        object.insert(
            "reviews".into(),
            Value::Object(self.reviews.clone().into_iter().collect()),
        );
        for (key, value) in &self.extra {
            object.insert(key.clone(), value.clone());
        }
        let mut text = serde_json::to_string_pretty(&Value::Object(object))
            .unwrap_or_else(|error| unreachable!("JSON values serialise: {error}"));
        text.push('\n');
        text
    }
}

/// The service (see the module docs).
pub struct ReviewUi {
    contents: ReviewUiFile,
    file: Entity<JsonFile>,
}

impl ReviewUi {
    /// Reads `<data_dir>/review_ui.json` synchronously (it is small, and the first review shown
    /// needs it).
    pub fn new(data_dir: &Path, cx: &mut Context<Self>) -> Self {
        let path = data_dir.join(REVIEW_UI_FILE);
        let (contents, unsaveable) = match read_object(&path) {
            ReadObject::Missing => (ReviewUiFile::default(), None),
            ReadObject::Object(object) => match ReviewUiFile::from_object(object) {
                Ok((contents, problems)) => {
                    for problem in problems {
                        log::warn!("{}: {problem}", path.display());
                    }
                    (contents, None)
                }
                Err(problem) => {
                    let reason = format!(
                        "{} can't be read ({problem}), so it's left as it is and view modes \
                         aren't saved this session",
                        path.display()
                    );
                    log::warn!("{reason}");
                    (ReviewUiFile::default(), Some(reason))
                }
            },
            ReadObject::Unreadable(reason) => {
                log::warn!("{reason}");
                (ReviewUiFile::default(), Some(reason))
            }
        };
        let file = cx.new(|cx| JsonFile::new(path, unsaveable, cx));
        Self { contents, file }
    }

    /// The view mode for `review`: the one stored, else split.
    pub fn view_mode(&self, review: &ReviewId) -> ViewMode {
        self.contents.view_mode(review).unwrap_or_default()
    }

    /// Remembers `mode` for `review` and saves the file in the background.
    pub fn set_view_mode(&mut self, review: &ReviewId, mode: ViewMode, cx: &mut Context<Self>) {
        if self.contents.set_view_mode(review, mode) {
            let text = self.contents.to_json();
            self.file.update(cx, |file, cx| file.save(text, cx));
            cx.notify();
        }
    }

    /// Why the last save failed (or why the file is never saved), until one succeeds.
    pub fn last_error<'a>(&self, cx: &'a App) -> Option<&'a str> {
        self.file.read(cx).last_error()
    }

    /// No save is waiting or running: automation's `settled`.
    pub fn is_settled(&self, cx: &App) -> bool {
        self.file.read(cx).is_settled()
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    fn id(n: u8) -> ReviewId {
        ReviewId::parse(&format!("r-01j9{n:0>22}")).expect("a review id")
    }

    fn object(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(object) => object,
            other => panic!("{other} isn't an object"),
        }
    }

    #[test]
    fn unknown_keys_survive_a_change() {
        let (mut file, problems) = ReviewUiFile::from_object(object(json!({
            "v": 1,
            "theme": "kept",
            "reviews": {
                id(1).as_str(): {"view_mode": "unified", "zoom": 2},
                id(2).as_str(): "another version's shape",
            },
        })))
        .unwrap();
        assert!(problems.is_empty());
        assert_eq!(file.view_mode(&id(1)), Some(ViewMode::Unified));
        assert_eq!(file.view_mode(&id(2)), None);
        assert_eq!(file.view_mode(&id(3)), None);
        assert!(file.set_view_mode(&id(1), ViewMode::Split));
        assert!(!file.set_view_mode(&id(1), ViewMode::Split));
        assert!(file.set_view_mode(&id(3), ViewMode::Unified));
        let written: Value = serde_json::from_str(&file.to_json()).unwrap();
        assert_eq!(
            written,
            json!({
                "v": 1,
                "theme": "kept",
                "reviews": {
                    id(1).as_str(): {"view_mode": "split", "zoom": 2},
                    id(2).as_str(): "another version's shape",
                    id(3).as_str(): {"view_mode": "unified"},
                },
            })
        );
    }

    #[test]
    fn a_newer_version_is_read_with_a_warning_and_a_wrong_shape_refused() {
        let (_, problems) = ReviewUiFile::from_object(object(json!({"v": 7}))).unwrap();
        assert_eq!(problems.len(), 1);
        assert!(ReviewUiFile::from_object(object(json!({"reviews": [1]}))).is_err());
    }

    #[gpui::test]
    fn modes_persist_and_an_unreadable_file_is_left_alone(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let ui = cx.new(|cx| ReviewUi::new(dir.path(), cx));
        ui.read_with(cx, |ui, _| {
            assert_eq!(ui.view_mode(&id(1)), ViewMode::Split)
        });
        ui.update(cx, |ui, cx| ui.set_view_mode(&id(1), ViewMode::Unified, cx));
        cx.run_until_parked();
        ui.read_with(cx, |ui, cx| assert!(ui.is_settled(cx)));
        let again = cx.new(|cx| ReviewUi::new(dir.path(), cx));
        again.read_with(cx, |ui, _| {
            assert_eq!(ui.view_mode(&id(1)), ViewMode::Unified)
        });

        let path = dir.path().join(REVIEW_UI_FILE);
        std::fs::write(&path, "[\"not ours\"]").unwrap();
        let unreadable = cx.new(|cx| ReviewUi::new(dir.path(), cx));
        unreadable.update(cx, |ui, cx| ui.set_view_mode(&id(2), ViewMode::Unified, cx));
        cx.run_until_parked();
        unreadable.read_with(cx, |ui, cx| {
            assert!(ui.is_settled(cx));
            assert!(ui.last_error(cx).is_some());
            // The choice still holds for the session.
            assert_eq!(ui.view_mode(&id(2)), ViewMode::Unified);
        });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[\"not ours\"]");
    }
}
