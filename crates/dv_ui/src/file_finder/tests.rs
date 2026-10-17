//! The file finder's list (PLAN §18.1): the side's files joined with the review's, filtered as
//! the file list filters, names before directories, changed files first among equals; and
//! thousands of files filter quickly.

use std::time::Instant;

use dv_core::diff::FileStatus;

use super::*;

fn paths(found: &[usize], candidates: &[Candidate]) -> Vec<String> {
    found
        .iter()
        .map(|&index| candidates[index].path.clone())
        .collect()
}

#[test]
fn the_side_and_the_reviews_files_list_once_each() {
    let side = ["src/app.ts", "src/util.ts", "README.md"].map(String::from);
    let changed = vec![
        ("src/app.ts".to_string(), FileStatus::Modified),
        ("src/gone.ts".to_string(), FileStatus::Deleted),
    ];
    let candidates = candidates(&side, &changed);
    let listed: Vec<(&str, Option<FileStatus>)> = candidates
        .iter()
        .map(|candidate| (candidate.path.as_str(), candidate.status))
        .collect();
    assert_eq!(
        listed,
        [
            ("README.md", None),
            ("src/app.ts", Some(FileStatus::Modified)),
            ("src/gone.ts", Some(FileStatus::Deleted)),
            ("src/util.ts", None),
        ]
    );
}

#[test]
fn every_word_filters_names_come_first_then_changed_files() {
    let side = [
        "docs/util-notes.md",
        "src/app.ts",
        "src/util.ts",
        "src/util/strings.ts",
        "tests/app.test.ts",
    ]
    .map(String::from);
    let changed = vec![("tests/app.test.ts".to_string(), FileStatus::Modified)];
    let candidates = candidates(&side, &changed);
    // Every file, the changed one first.
    assert_eq!(
        paths(&filter(&candidates, ""), &candidates)[0],
        "tests/app.test.ts"
    );
    // The name `util` but for its extension, then a name starting with it, then `util` in a
    // directory alone.
    assert_eq!(
        paths(&filter(&candidates, "UTIL"), &candidates),
        ["src/util.ts", "docs/util-notes.md", "src/util/strings.ts"]
    );
    // Every word, ignoring case; the changed file first among the names that match.
    assert_eq!(
        paths(&filter(&candidates, "app ts"), &candidates),
        ["tests/app.test.ts", "src/app.ts"]
    );
    assert!(filter(&candidates, "nothing").is_empty());
}

#[test]
fn an_exact_name_comes_before_a_prefix_and_matches_show() {
    let side = [
        "src/components/listing-feed-filters.tsx",
        "src/components/listing-feed.tsx",
        "src/routes/feed/listing-feed-page.tsx",
    ]
    .map(String::from);
    let candidates = candidates(&side, &[]);
    assert_eq!(
        paths(&filter(&candidates, "listing-feed"), &candidates),
        [
            "src/components/listing-feed.tsx",
            "src/components/listing-feed-filters.tsx",
            "src/routes/feed/listing-feed-page.tsx",
        ]
    );
    assert_eq!(
        matched("src/components/Listing-Feed.tsx", "feed list"),
        [15..19, 23..27]
    );
    assert!(matched("src/a.ts", "").is_empty());
}

#[test]
fn thousands_of_files_filter_in_memory_quickly() {
    let side: Vec<String> = (0..20_000)
        .map(|n| format!("packages/pkg{}/src/module{n}/file{n}.ts", n % 40))
        .collect();
    let candidates = candidates(&side, &[]);
    let started = Instant::now();
    for query in ["f", "fi", "fil", "file1", "file19", "pkg3 file19"] {
        let found = filter(&candidates, query);
        assert!(!found.is_empty(), "{query}");
    }
    // Six keystrokes over 20,000 paths; generous for a debug build on a busy machine.
    assert!(
        started.elapsed().as_millis() < 2_000,
        "took {:?}",
        started.elapsed()
    );
}
