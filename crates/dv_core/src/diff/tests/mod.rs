//! Fixture tests for the diff pipeline (docs/PLAN.md §6, §14.1): real git repositories, opened
//! hermetically, so git's own semantics (autocrlf, attributes, renames) are what's under test.

mod attributes;
mod file;
mod interdiff;
mod list;
mod parse_records;
mod rows;

use dv_test_support::FixtureRepo;

use super::*;
use crate::git::GitRepo;
use crate::target::{DiffSpec, Target};
use crate::testing::git_repo;

/// A resolved target with its file list.
struct Listed {
    git: GitRepo,
    spec: DiffSpec,
    entries: Vec<FileEntry>,
}

impl Listed {
    fn of(repo: &FixtureRepo, target: Target) -> Self {
        let git = git_repo(repo);
        let spec = target
            .resolve(&git)
            .unwrap_or_else(|err| panic!("{target}: {err:?}"));
        let entries = file_list(&git, &spec).unwrap_or_else(|err| panic!("{target}: {err:?}"));
        Self { git, spec, entries }
    }

    fn entry(&self, path: &str) -> &FileEntry {
        self.entries
            .iter()
            .find(|entry| entry.path == path)
            .unwrap_or_else(|| panic!("{path} not in {:#?}", self.entries))
    }

    fn diff(&self, path: &str) -> FileDiff {
        self.diff_with(path, &DiffLimits::default())
    }

    fn diff_with(&self, path: &str, limits: &DiffLimits) -> FileDiff {
        file_diff(&self.git, &self.spec, self.entry(path), limits)
            .unwrap_or_else(|err| panic!("{path}: {err:?}"))
    }

    fn text(&self, path: &str) -> TextDiff {
        match self.diff(path) {
            FileDiff::Text(text) => text,
            other => panic!("{path}: expected text, got {other:?}"),
        }
    }

    /// `(path, status, additions, deletions)` per entry.
    fn summary(&self) -> Vec<(&str, FileStatus, u32, u32)> {
        self.entries
            .iter()
            .map(|e| (e.path.as_str(), e.status, e.additions, e.deletions))
            .collect()
    }
}

/// Each line as `<kind><old>:<new> <text>`: ` 1:1 same`, `-2: gone`, `+:2 new`.
fn render(text: &TextDiff) -> Vec<String> {
    text.lines
        .iter()
        .map(|line| {
            let kind = match line.kind {
                LineKind::Context => ' ',
                LineKind::Removed => '-',
                LineKind::Added => '+',
            };
            let no = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
            format!(
                "{kind}{}:{} {}",
                no(line.old_no),
                no(line.new_no),
                line.text
            )
        })
        .collect()
}

/// `count` numbered lines, `line 1\n` to `line <count>\n`.
fn numbered(count: usize) -> String {
    (1..=count).map(|n| format!("line {n}\n")).collect()
}
