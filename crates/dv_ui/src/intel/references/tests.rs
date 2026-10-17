//! The References tab's pure parts: a row's symbol and highlight runs on character
//! boundaries, a line's start giving way so the symbol shows, paths dropping whole leading
//! segments, and the lines of a 700-place answer read and highlighted in parallel.

use std::sync::Arc;
use std::time::Instant;

use dv_core::backend::{LocalBackend, LocalBackendOptions};
use dv_lsp::{SymbolLocation, TextPosition, TextRange, UriTarget};
use dv_test_support::FixtureRepo;
use futures::StreamExt as _;
use pretty_assertions::assert_eq;

use super::render::elide_line;
use super::*;
use crate::components::elide_path_start;

fn location(path: &str, line: u32, from: u32, to: u32) -> SymbolLocation {
    SymbolLocation {
        target: UriTarget::Worktree(path.to_string()),
        range: TextRange {
            start: TextPosition::new(line, from),
            end: TextPosition::new(line, to),
        },
    }
}

fn row(text: &str, from: u32, to: u32) -> RefRow {
    RefRow {
        location: location("a.ts", 0, from, to),
        line: 1,
        text: Some(text.to_string()),
        tokens: Vec::new(),
    }
}

#[test]
fn a_rows_symbol_stays_on_character_boundaries() {
    // A UTF-16 server's columns read as bytes land inside `é` (two bytes in UTF-8).
    let text = "const é = helper(1);"; // `é` is bytes 6..8, `helper` 11..17.
    assert_eq!(row(text, 7, 9).symbol(), 6..9);
    assert_eq!(row(text, 11, 17).symbol(), 11..17);
    // Past the end, and a span over several lines.
    assert_eq!(row(text, 30, 40).symbol(), text.len()..text.len());
    let mut wide = row(text, 10, 0);
    wide.location.range.end = TextPosition::new(3, 0);
    assert_eq!(wide.symbol(), 10..text.len());
}

#[test]
fn highlight_runs_never_cut_inside_a_character() {
    let text = "é = 1"; // `é` is bytes 0..2.
    let red = gpui::red();
    let runs = runs(text, &[(1..3, red)], Some(0..1), gpui::blue());
    for (range, _) in &runs {
        assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
    }
    // The colour starts at 0 (no cut at 1); the mark is nothing.
    assert_eq!(
        runs.iter()
            .map(|(range, _)| range.clone())
            .collect::<Vec<_>>(),
        vec![0..3]
    );
}

#[test]
fn a_line_whose_symbol_is_past_the_row_gives_way_at_its_start() {
    let text = "        <span className={styles.badge}>{formatQuantity(rep.count)}</span>";
    let indent = 8;
    let start = text.find("formatQuantity").unwrap();
    let symbol = start..start + "formatQuantity".len();
    // Room for it all: only the indentation goes.
    let shown = elide_line(text, indent, Some(symbol.clone()), 120);
    assert_eq!(shown.text, text.trim_start());
    assert_eq!(
        shown.map(symbol.clone()),
        Some(start - indent..symbol.end - indent)
    );
    // Too narrow: the start gives way, the symbol shows with a little before it.
    let shown = elide_line(text, indent, Some(symbol.clone()), 30);
    assert!(shown.text.starts_with('…'), "{}", shown.text);
    let mark = shown.map(symbol.clone()).expect("the symbol shows");
    assert_eq!(&shown.text[mark.clone()], "formatQuantity");
    let before = shown.text[..mark.start].chars().count();
    assert!(before + "formatQuantity".len() <= 30, "{}", shown.text);
    assert!(
        before > 1,
        "something of the line before it: {}",
        shown.text
    );
    // A range before the cut is gone; one across it is cut there.
    assert_eq!(shown.map(0..indent + 5), None);
    // Characters, not bytes, are counted: an accent before the symbol cuts on a boundary.
    let accented = "ééééééééééééééééééééé helper(1)";
    let at = accented.find("helper").unwrap();
    let shown = elide_line(accented, 0, Some(at..at + 6), 12);
    let mark = shown.map(at..at + 6).unwrap();
    assert_eq!(&shown.text[mark], "helper");
}

#[test]
fn a_path_drops_whole_leading_segments() {
    let path = "src/components/feedback/feedback-badge.tsx";
    let fits = |max: usize| move |text: &str| text.chars().count() <= max;
    assert_eq!(elide_path_start(path, fits(80)), path);
    assert_eq!(
        elide_path_start(path, fits(32)),
        "…/feedback/feedback-badge.tsx"
    );
    assert_eq!(
        elide_path_start(path, fits(40)),
        "…/components/feedback/feedback-badge.tsx"
    );
    // Nothing but the name fits: the name alone, which truncates as it draws.
    assert_eq!(elide_path_start(path, fits(5)), "…/feedback-badge.tsx");
    assert_eq!(elide_path_start("README.md", fits(3)), "README.md");
    // A Windows path outside the repository drops whole segments too.
    assert_eq!(
        elide_path_start(r"C:\Users\kyle\lib\index.d.ts", fits(16)),
        r"…\lib\index.d.ts"
    );
}

/// 100 files of 40 lines, seven places in each: how long reading and highlighting them takes
/// (`cargo test -p dv_ui --lib seven_hundred -- --ignored --nocapture`).
#[test]
#[ignore = "a timing measurement, not a check"]
fn seven_hundred_places_read_in_parallel() {
    let repo = FixtureRepo::new();
    let mut jobs = Vec::new();
    for file in 0..100 {
        let path = format!("src/dir{}/file{file}.tsx", file % 10);
        let text: String = (0..40)
            .map(|line| format!("export const value{line} = cn(\"a\", props.x{line});\n"))
            .collect();
        repo.write(&path, &text);
        jobs.push(ReadJob {
            index: file,
            target: UriTarget::Worktree(path),
            lines: (0..7).map(|n| n * 5).collect(),
            known: None,
        });
    }
    let backend: Arc<dyn Backend> = Arc::new(
        LocalBackend::open_with(
            repo.path(),
            LocalBackendOptions {
                env: repo.env_overlay(),
            },
        )
        .unwrap(),
    );
    let started = Instant::now();
    let (send, landed) = futures::channel::mpsc::unbounded();
    read_lines(backend.as_ref(), jobs, send);
    let landed: Vec<Landed> = futures::executor::block_on(landed.collect());
    let texts = landed
        .iter()
        .filter(|landed| matches!(landed, Landed::Text { .. }))
        .count();
    let tokens = landed.len() - texts;
    println!(
        "700 places in 100 files: read and highlighted in {:?} ({texts} files' lines, \
         {tokens} files' tokens)",
        started.elapsed()
    );
    assert_eq!(texts, 100);
    assert_eq!(tokens, 100);
}
