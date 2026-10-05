//! The repositories dv knows (PLAN §11 `Reviews`): `<data dir>/known_repos.json`,
//! `{"v": 1, "repos": [{"location", "added_at", "opened_at"}]}`, times in ms since the Unix epoch.
//!
//! Deliberately not the first implementation's `repos.json`, which sits in the same default
//! data dir with another shape. Read leniently ([`super::super::persist`]): unknown keys, at the
//! top and in each entry, are kept and written back; an entry this version can't read (no
//! location, another version's shape) is kept as it was, after a warning; a repeated location
//! counts once.

use dv_core::Location;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The file in the data dir.
pub const KNOWN_REPOS_FILE: &str = "known_repos.json";
/// The `"v"` this version writes.
pub const KNOWN_REPOS_VERSION: u64 = 1;

/// One known repository.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KnownRepo {
    /// The worktree root, as `Location` serialises (`{"local": "D:\\code\\api"}`).
    pub location: Location,
    /// When dv first saw it.
    #[serde(default)]
    pub added_at: u64,
    /// When a review of it was last opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened_at: Option<u64>,
    /// Keys this version doesn't know.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl KnownRepo {
    pub fn new(location: Location, now: u64) -> Self {
        Self {
            location,
            added_at: now,
            opened_at: None,
            extra: Map::new(),
        }
    }

    /// When it was last used: opened, else added. Lists show the most recent first.
    pub fn last_used(&self) -> u64 {
        self.opened_at.unwrap_or(self.added_at).max(self.added_at)
    }
}

/// An entry of the file, as read.
#[derive(Clone, Debug, PartialEq)]
enum Entry {
    Known(KnownRepo),
    /// An entry this version can't read, written back as it was.
    Kept(Value),
}

/// The file's contents.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KnownReposFile {
    entries: Vec<Entry>,
    /// Top-level keys this version doesn't know.
    extra: Map<String, Value>,
}

impl KnownReposFile {
    /// Reads the file's object leniently: the repos, and a line for each thing that couldn't
    /// be read. `Err` when `repos` isn't a list at all: not a file this version can read, so
    /// it is never written over.
    pub fn from_object(mut object: Map<String, Value>) -> Result<(Self, Vec<String>), String> {
        let mut problems = Vec::new();
        if let Some(v) = object.get("v") {
            match v.as_u64() {
                Some(v) if v > KNOWN_REPOS_VERSION => problems.push(format!(
                    "written by a newer dv (v {v}); reading the entries this version knows"
                )),
                Some(_) => {}
                None => problems.push(format!("\"v\" is {v}, not a version number")),
            }
        }
        let entries = match object.remove("repos") {
            Some(Value::Array(entries)) => entries,
            None => Vec::new(),
            Some(other) => return Err(format!("\"repos\" is {other}, not a list")),
        };
        object.remove("v");
        let mut file = Self::default();
        for (index, value) in entries.into_iter().enumerate() {
            match serde_json::from_value::<KnownRepo>(value.clone()) {
                Ok(repo) if file.get(&repo.location).is_none() => {
                    file.entries.push(Entry::Known(repo))
                }
                Ok(repo) => problems.push(format!(
                    "repos[{index}] repeats {}; reading it once",
                    repo.location
                )),
                Err(error) => {
                    problems.push(format!("repos[{index}] can't be read ({error}); kept"));
                    file.entries.push(Entry::Kept(value));
                }
            }
        }
        file.extra = object;
        Ok((file, problems))
    }

    /// The repos this version can read, in file order.
    pub fn repos(&self) -> impl Iterator<Item = &KnownRepo> {
        self.entries.iter().filter_map(|entry| match entry {
            Entry::Known(repo) => Some(repo),
            Entry::Kept(_) => None,
        })
    }

    pub fn get(&self, location: &Location) -> Option<&KnownRepo> {
        self.repos().find(|repo| &repo.location == location)
    }

    fn get_mut(&mut self, location: &Location) -> Option<&mut KnownRepo> {
        self.entries.iter_mut().find_map(|entry| match entry {
            Entry::Known(repo) if &repo.location == location => Some(repo),
            _ => None,
        })
    }

    /// Adds `location` (added now) unless it is known; `true` when it was added.
    pub fn add(&mut self, location: Location, now: u64) -> bool {
        if self.get(&location).is_some() {
            return false;
        }
        self.entries
            .push(Entry::Known(KnownRepo::new(location, now)));
        true
    }

    /// Records that a review of `location` was opened now; `false` when it isn't known.
    pub fn mark_opened(&mut self, location: &Location, now: u64) -> bool {
        match self.get_mut(location) {
            Some(repo) => {
                repo.opened_at = Some(now);
                true
            }
            None => false,
        }
    }

    /// Forgets `location`; `true` when it was known.
    pub fn remove(&mut self, location: &Location) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|entry| !matches!(entry, Entry::Known(repo) if &repo.location == location));
        self.entries.len() != before
    }

    /// The file's text: `v` and `repos` first, then the kept keys; pretty JSON with a trailing
    /// newline.
    pub fn to_json(&self) -> String {
        let mut object = Map::new();
        object.insert("v".into(), KNOWN_REPOS_VERSION.into());
        let repos: Vec<Value> = self
            .entries
            .iter()
            .map(|entry| match entry {
                Entry::Known(repo) => serde_json::to_value(repo)
                    .unwrap_or_else(|error| unreachable!("a known repo serialises: {error}")),
                Entry::Kept(value) => value.clone(),
            })
            .collect();
        object.insert("repos".into(), Value::Array(repos));
        for (key, value) in &self.extra {
            object.insert(key.clone(), value.clone());
        }
        let mut text = serde_json::to_string_pretty(&Value::Object(object))
            .unwrap_or_else(|error| unreachable!("JSON values serialise: {error}"));
        text.push('\n');
        text
    }
}
