//! Relocation and placement (PLAN §8): every step of [`relocate`] and its edges, placement
//! across renames and lost sides, the anchor a move records, and the cost on a large file.

use std::time::Instant;

use crate::diff::{FileEntry, FileMode, FileStatus};
use crate::review::anchor::{AnchorPlace, anchor_selection};
use crate::review::history::History;
use crate::review::relocate::{
    FUZZY_MAX_ANCHORED, FUZZY_MAX_LINES, FuzzyLines, MoveKind, NEARBY_LINES, Placement, Relocation,
    SideLines, SideText, is_slight, line_similarity, place_thread, relocate, relocate_with,
    relocated_anchor, shows_edited, shows_moved,
};

use super::*;

/// Distinct, code-like lines `0..count`, each made of words picked pseudo-randomly from a
/// list, so two lines are about as alike as two lines of real code (and line `i` is the same
/// whatever `count` is).
fn code(count: usize) -> Vec<String> {
    const WORDS: [&str; 48] = [
        "let", "value", "request", "response", "parse", "config", "handler", "buffer", "stream",
        "token", "cache", "client", "server", "error", "result", "option", "vector", "string",
        "number", "index", "offset", "length", "window", "cursor", "thread", "review", "anchor",
        "column", "header", "footer", "render", "update", "insert", "remove", "select", "filter",
        "reduce", "expand", "commit", "branch", "merge", "status", "format", "decode", "encode",
        "signal", "worker", "socket",
    ];
    (0..count)
        .map(|i| {
            let mut state = (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5851_f42d;
            let mut word = || {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                WORDS[(state >> 33) as usize % WORDS.len()]
            };
            format!("    {} {}({}, {}) {i}", word(), word(), word(), word())
        })
        .collect()
}

/// The anchor of lines `from..=to` (1-based) of `lines`, on the new side of `a.rs` in round 1.
fn anchor_at(lines: &[String], from: u32, to: u32) -> Anchor {
    anchor_selection(
        AnchorPlace {
            round: 1,
            path: "a.rs".to_string(),
            old_path: None,
            side: AnchorSide::New,
        },
        lines,
        from,
        to,
    )
    .unwrap()
}

/// `lines` with `count` new lines inserted before line `at` (1-based).
fn insert(lines: &[String], at: usize, count: usize) -> Vec<String> {
    let mut out = lines.to_vec();
    let new: Vec<String> = (0..count)
        .map(|i| format!("// inserted comment number {i}"))
        .collect();
    out.splice(at - 1..at - 1, new);
    out
}

fn moved(start_line: u32, end_line: u32, how: MoveKind) -> Relocation {
    Relocation::Moved {
        start_line,
        end_line,
        how,
    }
}

/// The relocation's lines and kind name, for cases whose fuzzy score isn't the point.
fn summary(relocation: Relocation) -> Option<(u32, u32, &'static str)> {
    match relocation {
        Relocation::Moved {
            start_line,
            end_line,
            how,
        } => Some((start_line, end_line, how.as_str())),
        _ => None,
    }
}

fn fuzzy_score(relocation: Relocation) -> f64 {
    match relocation {
        Relocation::Moved {
            how: MoveKind::Fuzzy { score },
            ..
        } => score,
        other => panic!("not a fuzzy move: {other:?}"),
    }
}

#[test]
fn text_still_at_its_lines_is_unchanged() {
    let lines = code(40);
    let anchor = anchor_at(&lines, 10, 12);
    assert_eq!(relocate(&anchor, &lines), Relocation::Unchanged);
    // Context edits alone don't move it.
    let mut edited = lines.clone();
    edited[8] = "something else".to_string();
    edited[12] = "and here".to_string();
    assert_eq!(relocate(&anchor, &edited), Relocation::Unchanged);
}

#[test]
fn lines_inserted_or_removed_above_move_it_nearby() {
    let lines = code(40);
    let anchor = anchor_at(&lines, 20, 22);
    let inserted = insert(&lines, 5, 7);
    assert_eq!(
        relocate(&anchor, &inserted),
        moved(27, 29, MoveKind::Nearby)
    );
    let mut removed = lines.clone();
    removed.drain(2..6);
    assert_eq!(relocate(&anchor, &removed), moved(16, 18, MoveKind::Nearby));
    // Still nearby at the edge of the window, no longer beyond it (where it is unique).
    let edge = insert(&lines, 1, NEARBY_LINES);
    assert_eq!(relocate(&anchor, &edge), moved(220, 222, MoveKind::Nearby));
    let beyond = insert(&lines, 1, NEARBY_LINES + 1);
    assert_eq!(
        relocate(&anchor, &beyond),
        moved(221, 223, MoveKind::Unique)
    );
}

#[test]
fn context_picks_between_nearby_copies_before_distance_does() {
    let mut lines = code(120);
    lines[59] = "    return total;".to_string();
    let anchor = anchor_at(&lines, 60, 60);
    // The anchored line moves 40 lines down with its context, and a bare copy of it lands
    // 5 lines up, nearer but among other lines.
    let mut now = code(120);
    now[59] = "    // replaced".to_string();
    let block: Vec<String> = lines[56..63].to_vec();
    now.splice(97..97, block);
    now[54] = "    return total;".to_string();
    assert_eq!(relocate(&anchor, &now), moved(101, 101, MoveKind::Nearby));
    // With no context to tell them apart, the nearest wins, then the one above.
    let bare = Anchor {
        context_before: Vec::new(),
        context_after: Vec::new(),
        ..anchor.clone()
    };
    assert_eq!(relocate(&bare, &now), moved(55, 55, MoveKind::Nearby));
    let mut even = code(120);
    even[49] = "    return total;".to_string();
    even[69] = "    return total;".to_string();
    even[59] = "    // gone".to_string();
    assert_eq!(relocate(&bare, &even), moved(50, 50, MoveKind::Nearby));
}

#[test]
fn far_and_unique_is_found_anywhere() {
    let lines = code(1200);
    let anchor = anchor_at(&lines, 100, 101);
    let mut now: Vec<String> = lines.clone();
    let block: Vec<String> = now.drain(99..101).collect();
    now.splice(899..899, block);
    assert_eq!(relocate(&anchor, &now), moved(900, 901, MoveKind::Unique));
}

#[test]
fn duplicated_far_falls_to_fuzzy_or_outdated() {
    let lines = code(1200);
    let anchor = anchor_at(&lines, 100, 102);
    // Two far copies: one keeps the anchor's context (fuzzy finds it, every line equal), the
    // other is bare.
    let mut now = code(1200);
    for line in &mut now[96..105] {
        *line = format!("{line} // edited");
    }
    let with_context: Vec<String> = lines[96..105].to_vec();
    let bare: Vec<String> = lines[99..102].to_vec();
    now.splice(1100..1100, with_context);
    now.splice(600..600, bare);
    let found = relocate(&anchor, &now);
    assert_eq!(summary(found), Some((1107, 1109, "fuzzy")));
    assert!((fuzzy_score(found) - 1.0).abs() < 1e-9, "{found:?}");

    // Without its context anywhere, two exact copies are neither unique nor similar enough.
    let mut now = code(1200);
    now[99..102].clone_from_slice(&["a".to_string(), "b".to_string(), "c".to_string()]);
    let copy: Vec<String> = lines[99..102].to_vec();
    now.splice(700..700, copy.clone());
    now.splice(400..400, copy);
    assert_eq!(relocate(&anchor, &now), Relocation::Outdated);
}

#[test]
fn a_whitespace_only_edit_is_a_perfect_fuzzy_match() {
    let lines = code(40);
    let anchor = anchor_at(&lines, 10, 11);
    let mut now = lines.clone();
    now[9] = format!("\t{}  ", lines[9].trim().replace(" = ", "   =   "));
    let found = relocate(&anchor, &now);
    assert_eq!(summary(found), Some((10, 11, "fuzzy")));
    assert!((fuzzy_score(found) - 1.0).abs() < 1e-9, "{found:?}");
}

#[test]
fn a_one_word_edit_is_found_fuzzily() {
    let mut lines = code(60);
    lines[29] = "    let response = client.fetch_user(request.user_id).await?;".to_string();
    let anchor = anchor_at(&lines, 30, 30);
    let mut now = insert(&lines, 3, 4);
    now[33] = "    let response = client.load_user(request.user_id).await?;".to_string();
    let found = relocate(&anchor, &now);
    assert_eq!(summary(found), Some((34, 34, "fuzzy")));
    assert!(fuzzy_score(found) >= 0.8, "{found:?}");
    assert!(line_similarity(&lines[29], &now[33]) < 1.0);
}

#[test]
fn a_heavy_rewrite_is_outdated() {
    let lines = code(60);
    let anchor = anchor_at(&lines, 30, 32);
    let mut now = lines.clone();
    now[29] = "fn unrelated() {".to_string();
    now[30] = "    panic!(\"nothing like it\");".to_string();
    now[31] = "}".to_string();
    now[27] = "// rewritten too".to_string();
    assert_eq!(relocate(&anchor, &now), Relocation::Outdated);
}

#[test]
fn anchors_at_the_files_edges_have_short_context() {
    let lines = code(20);
    let first = anchor_at(&lines, 1, 1);
    assert!(first.context_before.is_empty());
    assert_eq!(
        relocate(&first, &insert(&lines, 1, 3)),
        moved(4, 4, MoveKind::Nearby)
    );
    let last = anchor_at(&lines, 20, 20);
    assert!(last.context_after.is_empty());
    let mut appended = insert(&lines, 1, 2);
    appended.push("// a new last line".to_string());
    assert_eq!(relocate(&last, &appended), moved(22, 22, MoveKind::Nearby));

    // A fuzzy window is clipped at the edges, and the lines it lacks score 0: the anchor's
    // three lines of context before can't stand above line 1.
    let mut lines = code(20);
    lines[4] = "    let response = client.fetch_user(request.user_id).await?;".to_string();
    let anchor = anchor_at(&lines, 5, 5);
    let mut now = lines[4..].to_vec();
    now[0] = "    let response = client.load_user(request.user_id).await?;".to_string();
    assert_eq!(relocate(&anchor, &now), Relocation::Outdated);
}

#[test]
fn a_multi_line_range_moves_whole() {
    let lines = code(80);
    let anchor = anchor_at(&lines, 40, 45);
    assert_eq!(anchor.text.len(), 6);
    let now = insert(&lines, 10, 11);
    assert_eq!(relocate(&anchor, &now), moved(51, 56, MoveKind::Nearby));
    // One of its lines edited: fuzzy, the whole range.
    let mut edited = now.clone();
    edited[52] = format!("{} // tweak", edited[52]);
    let found = relocate(&anchor, &edited);
    assert_eq!(summary(found), Some((51, 56, "fuzzy")));
}

#[test]
fn a_blank_anchor_needs_its_context_to_match() {
    let mut lines = code(60);
    lines[29] = String::new();
    lines[30] = "   ".to_string();
    let anchor = anchor_at(&lines, 30, 31);
    let blank_run = |lines: &mut [String], at: usize| {
        lines[at] = String::new();
        lines[at + 1] = "   ".to_string();
    };
    // Copies of the blank run nearer than the real one, which moved down with its context.
    let mut now = insert(&lines, 5, 10);
    blank_run(&mut now, 20);
    blank_run(&mut now, 33);
    assert_eq!(relocate(&anchor, &now), moved(40, 41, MoveKind::Nearby));

    // Its context gone, a blank run matches nowhere by its text alone: neither of two runs at
    // the same distance, nor the only one in the file.
    let mut lonely = code(60);
    for line in &mut lonely[26..34] {
        *line = format!("{line} // changed");
    }
    lonely[29] = "x".to_string();
    lonely[30] = "y".to_string();
    let mut two = lonely.clone();
    blank_run(&mut two, 9);
    blank_run(&mut two, 49);
    assert_eq!(relocate(&anchor, &two), Relocation::Outdated);
    let mut one = insert(&lonely, 1, NEARBY_LINES);
    blank_run(&mut one, 250);
    assert_eq!(relocate(&anchor, &one), Relocation::Outdated);
    // A plain anchor in the same spot would be found there: the rule is the blank lines'.
    let mut plain = lonely.clone();
    plain[9] = "only once".to_string();
    let once = Anchor {
        text: vec!["only once".to_string()],
        end_line: anchor.start_line,
        ..anchor.clone()
    };
    assert_eq!(relocate(&once, &plain), moved(10, 10, MoveKind::Nearby));
}

#[test]
fn line_similarity_follows_the_rules() {
    assert_eq!(line_similarity("", ""), 1.0);
    assert_eq!(
        line_similarity("  a  b ", "a b"),
        1.0,
        "whitespace normalised"
    );
    assert_eq!(line_similarity("a", "b"), 0.0, "short lines by equality");
    assert_eq!(line_similarity("a", "ab"), 0.0);
    assert_eq!(line_similarity("abc", "xyz"), 0.0);
    // "night" / "nacht": ni ig gh ht / na ac ch ht share one of eight.
    assert!((line_similarity("night", "nacht") - 0.25).abs() < 1e-9);
    // Repeats count once per occurrence: "aaa" (aa aa) / "aa" (aa): 2·1/3.
    assert!((line_similarity("aaa", "aa") - 2.0 / 3.0).abs() < 1e-9);
}

#[test]
fn an_empty_or_too_long_anchor_is_outdated() {
    let lines = code(5);
    let mut anchor = anchor_at(&lines, 2, 3);
    anchor.start_line = 40;
    anchor.end_line = 41;
    // Not at its (impossible) lines, but exactly once elsewhere: moved.
    assert_eq!(relocate(&anchor, &lines), moved(2, 3, MoveKind::Nearby));
    let long = anchor_at(&code(10), 1, 10);
    assert_eq!(relocate(&long, &lines), Relocation::Outdated);
}

/// `save()` and `remove()` both end in `return Ok(());`, ten lines apart.
fn two_functions() -> Vec<String> {
    [
        "pub fn save(store: &Store, item: &Item) -> Result<()> {",
        "    store.write(item)?;",
        "    store.flush()?;",
        "    return Ok(());",
        "}",
        "",
        "pub fn load(store: &Store, id: u64) -> Result<Item> {",
        "    let item = store.read(id)?;",
        "    Ok(item)",
        "}",
        "",
        "pub fn remove(store: &Store, id: u64) -> Result<()> {",
        "    store.delete(id)?;",
        "    return Ok(());",
        "}",
    ]
    .iter()
    .map(|line| line.to_string())
    .collect()
}

#[test]
fn an_edit_to_the_anchored_line_is_found_where_it_was_made_not_at_a_bare_copy() {
    let lines = two_functions();
    let anchor = anchor_at(&lines, 4, 4);
    // The agent edits the very line the reviewer commented on; `remove()` still ends in the
    // old text, among other lines.
    let mut now = lines.clone();
    now[3] = "    return Ok(written_id);".to_string();
    let found = relocate(&anchor, &now);
    assert_eq!(summary(found), Some((4, 4, "fuzzy")), "{found:?}");

    // The same with the copy far away (the only exact match in the file).
    let mut far = lines.clone();
    far.extend(code(400));
    far.extend(lines[11..].iter().cloned());
    far[3] = "    return Ok(written_id);".to_string();
    let found = relocate(&anchor, &far);
    assert_eq!(summary(found), Some((4, 4, "fuzzy")), "{found:?}");

    // A copy that stands among its context still wins at once: lines inserted above.
    let shifted = insert(&lines, 1, 3);
    assert_eq!(relocate(&anchor, &shifted), moved(7, 7, MoveKind::Nearby));
    // And a bare copy still wins when nothing scores better than it does: the anchored line
    // and its context rewritten where they stood.
    let mut rewritten = lines.clone();
    for line in &mut rewritten[0..6] {
        *line = "// gone".to_string();
    }
    assert_eq!(
        relocate(&anchor, &rewritten),
        moved(14, 14, MoveKind::Nearby)
    );
}

#[test]
fn a_fuzzy_match_prefers_the_nearest_window_within_the_margin() {
    // A flag list: the reviewer asks for flags 201–205 to be off, and the agent does that.
    let flags: Vec<String> = (1..=300)
        .map(|n| format!(r#"  {{ "key": "flag_{n}", "enabled": true, "weight": 3 }},"#))
        .collect();
    let anchor = anchor_at(&flags, 201, 205);
    let mut now = flags.clone();
    for line in &mut now[200..205] {
        *line = line.replace("true", "false");
    }
    let found = relocate(&anchor, &now);
    assert_eq!(summary(found), Some((201, 205, "fuzzy")), "{found:?}");

    // Rows of data, edited in place: an untouched run far away scores a little higher.
    let rows: Vec<String> = (1..=2_000)
        .map(|n| {
            format!(
                "INSERT INTO users (id, name, email) VALUES ({n}, 'user{n}', 'user{n}@example.com');"
            )
        })
        .collect();
    let anchor = anchor_at(&rows, 1_001, 1_040);
    let mut now = rows.clone();
    for line in &mut now[1_000..1_040] {
        *line = line.replace("example.com", "example.org");
    }
    let found = relocate(&anchor, &now);
    assert_eq!(summary(found), Some((1_001, 1_040, "fuzzy")), "{found:?}");
}

#[test]
fn a_long_anchor_is_scored_on_a_spread_of_its_lines() {
    let lines = code(600);
    let anchor = anchor_at(&lines, 200, 399);
    assert!(anchor.text.len() > FUZZY_MAX_ANCHORED);
    // Moved far with one line edited: no exact match, found fuzzily, whole.
    let mut now = lines.clone();
    let block: Vec<String> = now.drain(196..402).collect();
    now.splice(0..0, block);
    now[100] = format!("{} // edited", now[100]);
    let found = relocate(&anchor, &now);
    assert_eq!(summary(found), Some((4, 203, "fuzzy")), "{found:?}");
    // Rewritten: still outdated.
    let mut rewritten = lines.clone();
    for line in &mut rewritten[190..410] {
        *line = format!("// {}", line.chars().rev().collect::<String>());
    }
    assert_eq!(relocate(&anchor, &rewritten), Relocation::Outdated);
}

/// A side's lines with the fuzzy table kept by the caller.
struct Kept<'a> {
    lines: &'a [String],
    fuzzy: &'a FuzzyLines,
}

impl SideLines for Kept<'_> {
    type Line = String;

    fn lines(&self) -> &[String] {
        self.lines
    }

    fn fuzzy(&self) -> Option<&FuzzyLines> {
        Some(self.fuzzy)
    }
}

#[test]
fn threads_on_one_side_can_share_its_fuzzy_table() {
    let lines = code(300);
    let anchors = [
        anchor_at(&lines, 50, 52),
        anchor_at(&lines, 150, 150),
        anchor_at(&lines, 250, 260),
    ];
    let mut now = insert(&lines, 1, 5);
    for at in [54, 154, 256] {
        now[at] = format!("{} // tweak", now[at]);
    }
    let fuzzy = FuzzyLines::default();
    for anchor in &anchors {
        assert_eq!(
            relocate_with(anchor, &now, &fuzzy),
            relocate(anchor, &now),
            "{anchor:?}"
        );
        let thread = thread_on(anchor.clone());
        let entries = [entry("a.rs", None, FileStatus::Modified)];
        let kept = place_thread(&thread, &entries, History::None, |_, _| {
            SideText::Lines(Kept {
                lines: &now,
                fuzzy: &fuzzy,
            })
        });
        let fresh = place_thread(&thread, &entries, History::None, |_, _| {
            SideText::Lines(now.clone())
        });
        assert_eq!(kept, fresh);
    }
}

/// A file entry for placement tests.
fn entry(path: &str, old_path: Option<&str>, status: FileStatus) -> FileEntry {
    FileEntry {
        path: path.to_string(),
        old_path: old_path.map(str::to_string),
        status,
        additions: 1,
        deletions: 1,
        binary: false,
        old_mode: Some(FileMode::Regular),
        new_mode: Some(FileMode::Regular),
        old_oid: None,
        new_oid: None,
        new_untracked: false,
    }
}

fn thread_on(anchor: Anchor) -> Thread {
    Thread {
        id: ThreadId::generate(),
        anchor,
        original_anchor: None,
        author: reviewer(),
        body: "Why?".to_string(),
        created_at: 1,
        edited_at: None,
        deleted_at: None,
        state: ThreadState::Open,
        last_change: None,
        replies: Vec::new(),
    }
}

#[test]
fn placement_follows_the_file_and_its_side() {
    let lines = code(30);
    let anchor = anchor_at(&lines, 10, 10);
    let thread = thread_on(anchor.clone());
    let shifted = insert(&lines, 1, 2);
    let modified = [entry("a.rs", None, FileStatus::Modified)];
    let text = |lines: &[String]| {
        let lines = lines.to_vec();
        move |entry: &FileEntry, side| {
            assert_eq!((entry.path.as_str(), side), ("a.rs", AnchorSide::New));
            SideText::Lines(lines)
        }
    };
    assert_eq!(
        place_thread(&thread, &modified, History::None, text(&lines)),
        Placement::Unchanged {
            file: "a.rs".to_string()
        }
    );
    let placed = place_thread(&thread, &modified, History::None, text(&shifted));
    assert_eq!(
        placed,
        Placement::Moved {
            file: "a.rs".to_string(),
            start_line: 12,
            end_line: 12,
            how: MoveKind::Nearby
        }
    );
    assert_eq!(placed.lines(&anchor), Some((12, 12)));
    assert_eq!(placed.as_str(), "moved");

    // Renamed since: the entry renamed from its path holds it.
    let renamed = [
        entry("b.rs", None, FileStatus::Added),
        entry("c.rs", Some("a.rs"), FileStatus::Renamed { similarity: 90 }),
    ];
    assert_eq!(
        place_thread(&thread, &renamed, History::None, |entry: &FileEntry, _| {
            assert_eq!(entry.path, "c.rs");
            SideText::Lines(shifted.clone())
        })
        .file(),
        Some("c.rs")
    );

    // Not loaded yet, or no lines on that side.
    assert_eq!(
        place_thread(&thread, &modified, History::None, |_: &FileEntry, _| {
            SideText::<Vec<&str>>::Pending
        }),
        Placement::Pending {
            file: Some("a.rs".to_string())
        }
    );
    assert_eq!(
        place_thread(&thread, &modified, History::None, |_: &FileEntry, _| {
            SideText::<Vec<&str>>::Unavailable
        }),
        Placement::Outdated {
            file: Some("a.rs".to_string()),
            trail: None,
        }
    );
    // Its text gone from the file.
    let rewritten = vec!["nothing"; 30];
    let gone = place_thread(&thread, &modified, History::None, |_: &FileEntry, _| {
        SideText::Lines(rewritten.clone())
    });
    assert!(gone.is_outdated() && gone.file() == Some("a.rs"));
    assert_eq!(gone.lines(&anchor), None);
}

#[test]
fn a_thread_whose_file_or_side_is_gone_is_outdated() {
    let lines = code(30);
    let thread = thread_on(anchor_at(&lines, 10, 10));
    let never = |_: &FileEntry, _: AnchorSide| -> SideText<Vec<&str>> {
        panic!("no text is read for a file or side that isn't there")
    };
    assert_eq!(
        place_thread(
            &thread,
            &[entry("other.rs", None, FileStatus::Modified)],
            History::None,
            never
        ),
        Placement::Outdated {
            file: None,
            trail: None,
        }
    );
    // A new-side thread on a file deleted since.
    assert_eq!(
        place_thread(
            &thread,
            &[entry("a.rs", None, FileStatus::Deleted)],
            History::None,
            never
        ),
        Placement::Outdated {
            file: Some("a.rs".to_string()),
            trail: None,
        }
    );
    // An old-side thread on a file that is new now.
    let old_side = thread_on(Anchor {
        side: AnchorSide::Old,
        ..anchor_at(&lines, 3, 4)
    });
    for status in [FileStatus::Added, FileStatus::Untracked] {
        assert_eq!(
            place_thread(
                &old_side,
                &[entry("a.rs", None, status)],
                History::None,
                never
            ),
            Placement::Outdated {
                file: Some("a.rs".to_string()),
                trail: None,
            }
        );
    }
}

#[test]
fn an_old_side_thread_on_a_renamed_file_finds_its_entry_by_the_source() {
    let lines = code(30);
    // Opened on the old side of `a.rs → c.rs`: the anchor records the source path.
    let renamed = entry("c.rs", Some("a.rs"), FileStatus::Renamed { similarity: 95 });
    let place = AnchorPlace::for_entry(&renamed, AnchorSide::Old, 1);
    assert_eq!(
        (place.path.as_str(), place.old_path.as_deref()),
        ("a.rs", Some("a.rs"))
    );
    let anchor = anchor_selection(place, &lines, 7, 8).unwrap();
    let thread = thread_on(anchor);
    let placed = place_thread(
        &thread,
        std::slice::from_ref(&renamed),
        History::None,
        |entry, side| {
            assert_eq!((entry.path.as_str(), side), ("c.rs", AnchorSide::Old));
            SideText::Lines(lines.clone())
        },
    );
    assert_eq!(
        placed,
        Placement::Unchanged {
            file: "c.rs".to_string()
        }
    );
}

#[test]
fn a_file_renamed_again_keeps_its_threads() {
    let lines = code(30);
    // Written on the new side of `a.rs → b.rs`; the agent then renames it to `c.rs`, which the
    // target's diff reports as one rename, `a.rs → c.rs`.
    let first = entry("b.rs", Some("a.rs"), FileStatus::Renamed { similarity: 95 });
    let anchor = anchor_selection(
        AnchorPlace::for_entry(&first, AnchorSide::New, 1),
        &lines,
        5,
        5,
    )
    .unwrap();
    assert_eq!(
        (anchor.path.as_str(), anchor.old_path.as_deref()),
        ("b.rs", Some("a.rs"))
    );
    let thread = thread_on(anchor);
    let again = entry("c.rs", Some("a.rs"), FileStatus::Renamed { similarity: 95 });
    let placed = place_thread(
        &thread,
        std::slice::from_ref(&again),
        History::None,
        |entry, side| {
            assert_eq!((entry.path.as_str(), side), ("c.rs", AnchorSide::New));
            SideText::Lines(lines.clone())
        },
    );
    assert_eq!(
        placed,
        Placement::Unchanged {
            file: "c.rs".to_string()
        }
    );
    // A new file that only reuses the first name isn't it.
    let other = [
        entry("a.rs", None, FileStatus::Added),
        entry("z.rs", None, FileStatus::Modified),
    ];
    assert_eq!(
        place_thread(&thread, &other, History::None, |_, _| SideText::Lines(
            lines.clone()
        )),
        Placement::Outdated {
            file: None,
            trail: None,
        }
    );
}

#[test]
fn a_moved_thread_records_its_anchor_retaken_where_it_moved() {
    let lines = code(30);
    let anchor = anchor_at(&lines, 10, 11);
    let thread = thread_on(anchor.clone());
    let now = insert(&lines, 1, 4);
    let renamed = entry("b.rs", Some("a.rs"), FileStatus::Renamed { similarity: 90 });
    let placed = place_thread(
        &thread,
        std::slice::from_ref(&renamed),
        History::None,
        |_, _| SideText::Lines(now.clone()),
    );
    let recorded = relocated_anchor(&anchor, &placed, &renamed, &now, 3).expect("it moved");
    assert_eq!(recorded.round, 3);
    assert_eq!(recorded.path, "b.rs");
    assert_eq!(recorded.old_path.as_deref(), Some("a.rs"));
    assert_eq!((recorded.start_line, recorded.end_line), (14, 15));
    assert_eq!(recorded.text, anchor.text);
    assert_eq!(
        recorded.context_before,
        now[10..13].to_vec(),
        "fresh context"
    );
    assert!(recorded.matches(&now));
    // Nothing to record while it stands at its lines in its own file, or when it is lost.
    let unchanged = Placement::Unchanged {
        file: "b.rs".to_string(),
    };
    let modified = entry("a.rs", None, FileStatus::Modified);
    let in_place = Placement::Unchanged {
        file: "a.rs".to_string(),
    };
    assert_eq!(
        relocated_anchor(&anchor, &in_place, &modified, &lines, 3),
        None
    );
    let lost = Placement::Outdated {
        file: None,
        trail: None,
    };
    assert_eq!(relocated_anchor(&anchor, &lost, &renamed, &now, 3), None);

    // It shows as moved while relocation finds it moved, and still once the move is recorded
    // and it stands at the recorded lines.
    assert!(shows_moved(&thread, &placed));
    assert!(!shows_moved(&thread, &unchanged));
    let recorded_thread = Thread {
        anchor: recorded.clone(),
        original_anchor: Some(anchor.clone()),
        ..thread.clone()
    };
    let placed = place_thread(
        &recorded_thread,
        std::slice::from_ref(&renamed),
        History::None,
        |_, _| SideText::Lines(now.clone()),
    );
    assert!(matches!(placed, Placement::Unchanged { .. }));
    assert!(shows_moved(&recorded_thread, &placed));
    assert!(!shows_moved(&recorded_thread, &lost));
}

#[test]
fn a_thread_unchanged_in_a_renamed_file_records_the_files_new_name() {
    let lines = code(30);
    let anchor = anchor_at(&lines, 10, 11);
    let thread = thread_on(anchor.clone());
    // `a.rs` renamed to `b.rs` since, its text where it was.
    let renamed = entry("b.rs", Some("a.rs"), FileStatus::Renamed { similarity: 97 });
    let placed = place_thread(
        &thread,
        std::slice::from_ref(&renamed),
        History::None,
        |_, _| SideText::Lines(lines.clone()),
    );
    assert_eq!(
        placed,
        Placement::Unchanged {
            file: "b.rs".to_string()
        }
    );
    let recorded =
        relocated_anchor(&anchor, &placed, &renamed, &lines, 2).expect("the rename is recorded");
    assert_eq!(
        (recorded.path.as_str(), recorded.old_path.as_deref()),
        ("b.rs", Some("a.rs"))
    );
    assert_eq!((recorded.start_line, recorded.end_line), (10, 11));
    assert_eq!(recorded.round, 2);
    // Recorded, it isn't "moved": its lines are the ones it was written on.
    let recorded_thread = Thread {
        anchor: recorded.clone(),
        original_anchor: Some(anchor.clone()),
        ..thread.clone()
    };
    assert!(!shows_moved(&recorded_thread, &placed));
    // Once the rename is committed (the old side has `b.rs`, which is no longer a rename),
    // the recorded thread keeps its file; the unrecorded one has nothing to find it by.
    let committed = [entry("b.rs", None, FileStatus::Modified)];
    let text = |_: &FileEntry, _| SideText::Lines(lines.clone());
    assert_eq!(
        place_thread(&recorded_thread, &committed, History::None, text),
        Placement::Unchanged {
            file: "b.rs".to_string()
        }
    );
    assert_eq!(
        place_thread(&thread, &committed, History::None, text),
        Placement::Outdated {
            file: None,
            trail: None,
        }
    );
    // An old-side thread names the rename's source, which doesn't change.
    let old_anchor = Anchor {
        side: AnchorSide::Old,
        ..anchor.clone()
    };
    let old_placed = Placement::Unchanged {
        file: "b.rs".to_string(),
    };
    assert_eq!(
        relocated_anchor(&old_anchor, &old_placed, &renamed, &lines, 2),
        None
    );
}

/// A dispatcher's `match`: arms that look alike, as list-shaped code does.
fn dispatch_arms() -> Vec<String> {
    let mut lines = vec![
        "impl Dispatcher {".to_string(),
        "    pub async fn dispatch(&self, ctx: &Ctx, kind: Kind, request: Request) -> Result<Response> {"
            .to_string(),
        "        match kind {".to_string(),
    ];
    for name in [
        "get", "put", "delete", "list", "search", "export", "import", "watch", "stat", "copy",
        "move", "touch",
    ] {
        let kind = format!("{}{}", name[..1].to_uppercase(), &name[1..]);
        lines.push(format!(
            "            Kind::{kind} => self.{name}(ctx, request).await,"
        ));
    }
    lines.extend(["        }", "    }", "}"].map(String::from));
    lines
}

#[test]
fn a_deleted_line_is_outdated_not_moved_onto_a_line_like_it() {
    // "Drop the List arm": the agent deletes it. Its neighbours look alike enough for a fuzzy
    // window to land on the next arm, but its context closed up where it was: it's gone.
    let lines = dispatch_arms();
    assert_eq!(
        lines[6],
        "            Kind::List => self.list(ctx, request).await,"
    );
    let anchor = anchor_at(&lines, 7, 7);
    let mut now = lines.clone();
    now.remove(6);
    assert_eq!(relocate(&anchor, &now), Relocation::Outdated);
    // Lines inserted above as well: the same.
    let shifted = insert(&now, 1, 5);
    assert_eq!(relocate(&anchor, &shifted), Relocation::Outdated);

    // Rows of data: one deleted.
    let rows: Vec<String> = (1..=40)
        .map(|n| format!("INSERT INTO users VALUES ({n}, 'user{n}', 'user{n}@example.com');"))
        .collect();
    let anchor = anchor_at(&rows, 20, 20);
    let mut now = rows.clone();
    now.remove(19);
    assert_eq!(relocate(&anchor, &now), Relocation::Outdated);
    // Several rows anchored, all deleted.
    let anchor = anchor_at(&rows, 20, 22);
    let mut now = rows.clone();
    now.drain(19..22);
    assert_eq!(relocate(&anchor, &now), Relocation::Outdated);

    // At the file's edges, where the context is short.
    let first = anchor_at(&rows, 1, 1);
    assert_eq!(relocate(&first, &rows[1..]), Relocation::Outdated);
    let last = anchor_at(&rows, 40, 40);
    assert_eq!(relocate(&last, &rows[..39]), Relocation::Outdated);
}

#[test]
fn a_line_rewritten_where_it_stood_is_found_there_or_nowhere() {
    let lines = dispatch_arms();
    let anchor = anchor_at(&lines, 7, 7);
    // Rewritten to something else entirely: outdated, not the next arm.
    let mut now = lines.clone();
    now[6] = "            struct Unrelated;".to_string();
    assert_eq!(relocate(&anchor, &now), Relocation::Outdated);
    // An edit to the arm itself, though another arm is more like the old text than the edit:
    // found where it was made.
    let mut edited = lines.clone();
    edited[6] = "            Kind::List => Err(Error::Unsupported),".to_string();
    assert!(
        line_similarity(&lines[6], &lines[7]) > line_similarity(&lines[6], &edited[6]),
        "the next arm is the closer line"
    );
    let found = relocate(&anchor, &edited);
    assert_eq!(summary(found), Some((7, 7, "fuzzy")), "{found:?}");
    assert!(fuzzy_score(found) >= 0.8);
    // Rewritten into more lines than it had: found over the lines that stand there now.
    let mut grown = lines.clone();
    grown.splice(
        6..7,
        [
            "            Kind::List => {",
            "                self.list(ctx, request).await",
            "            }",
        ]
        .map(String::from),
    );
    let found = relocate(&anchor, &grown);
    assert_eq!(summary(found), Some((7, 9, "fuzzy")), "{found:?}");
}

#[test]
fn a_block_moved_away_is_found_where_it_went_though_its_context_closed_up() {
    let lines = code(200);
    let anchor = anchor_at(&lines, 50, 52);
    // The three anchored lines alone are cut and pasted far down: their context closes up
    // behind them, and the exact text found elsewhere wins.
    let mut now = lines.clone();
    let block: Vec<String> = now.drain(49..52).collect();
    now.splice(170..170, block);
    assert_eq!(relocate(&anchor, &now), moved(171, 173, MoveKind::Nearby));
}

/// Three functions that each print the same debug line, ending in the same brace, a blank
/// line between them; `filler` lines of other code stand between the first and the second.
fn twin_lines(filler: usize) -> Vec<String> {
    let mut lines: Vec<String> = [
        "use std::path::Path;",
        "",
        "// Reading and writing notes.",
        "fn load(path: &Path) -> Result<String> {",
        "    let text = std::fs::read_to_string(path)?;",
        "    println!(\"here\");",
        "    Ok(text)",
        "}",
        "",
    ]
    .map(String::from)
    .to_vec();
    lines.extend(code(filler));
    lines.extend(
        [
            "fn save(path: &Path, text: &str) -> Result<()> {",
            "    std::fs::write(path, text)?;",
            "    println!(\"here\");",
            "    Ok(())",
            "}",
            "",
            "fn wipe(path: &Path) -> Result<()> {",
            "    std::fs::remove_file(path)?;",
            "    println!(\"here\");",
            "    Ok(())",
            "}",
        ]
        .map(String::from),
    );
    lines
}

#[test]
fn a_line_with_a_twin_elsewhere_is_not_moved_onto_the_twin_once_it_goes() {
    for filler in [0, 2 * NEARBY_LINES] {
        let lines = twin_lines(filler);
        assert_eq!(lines[5], "    println!(\"here\");");
        // "Remove this debug print": the agent deletes it, and the other prints stay.
        let print = anchor_at(&lines, 6, 6);
        let mut now = lines.clone();
        now.remove(5);
        assert_eq!(relocate(&print, &now), Relocation::Outdated, "{filler}");
        // Lines inserted above as well: the same.
        assert_eq!(
            relocate(&print, &insert(&now, 1, 4)),
            Relocation::Outdated,
            "{filler}"
        );
        // Edited where it stands: found there, not at an untouched copy of the old text.
        let mut edited = lines.clone();
        edited[5] = "    println!(\"here: {}\", text.len());".to_string();
        let found = relocate(&print, &edited);
        assert_eq!(summary(found), Some((6, 6, "fuzzy")), "{filler}: {found:?}");

        // The closing brace edited to `};`, or deleted: never the next function's `}`.
        let brace = anchor_at(&lines, 8, 8);
        let mut edited = lines.clone();
        edited[7] = "};".to_string();
        assert_eq!(relocate(&brace, &edited), Relocation::Outdated, "{filler}");
        let mut deleted = lines.clone();
        deleted.remove(7);
        assert_eq!(relocate(&brace, &deleted), Relocation::Outdated, "{filler}");

        // The blank line after it deleted: not the blank line between the next two.
        let blank = anchor_at(&lines, 9, 9);
        let mut deleted = lines.clone();
        deleted.remove(8);
        assert_eq!(relocate(&blank, &deleted), Relocation::Outdated, "{filler}");
    }

    // A twin standing among the anchor's own context is still where the anchor went: lines
    // inserted above leave the whole context around it.
    let lines = twin_lines(0);
    let print = anchor_at(&lines, 6, 6);
    assert_eq!(
        relocate(&print, &insert(&lines, 2, 3)),
        moved(9, 9, MoveKind::Nearby)
    );
    // Lines inserted right beside it, inside its context, leave the line itself standing
    // between the context: found there, alone, not as a rewrite of the lines around it.
    let mut beside = lines.clone();
    beside.insert(5, "    // Says where it got to.".to_string());
    assert_eq!(relocate(&print, &beside), moved(7, 7, MoveKind::Nearby));
    beside.insert(7, "    log::debug!(\"loaded\");".to_string());
    assert_eq!(relocate(&print, &beside), moved(7, 7, MoveKind::Nearby));
    // At the end of a file the closed-up context runs to the file's end, so a line added just
    // above the last line is inserted beside it: the line is found where it went, alone.
    let tail: Vec<String> = [
        "export const a = 1;",
        "export const c = 33;",
        "export const d = 4;",
    ]
    .map(String::from)
    .to_vec();
    let last = anchor_at(&tail, 3, 3);
    let mut documented = tail.clone();
    documented.insert(2, "/** d. */".to_string());
    assert_eq!(relocate(&last, &documented), moved(4, 4, MoveKind::Nearby));
    // A slight line cut and pasted elsewhere looks the same as one deleted beside a twin that
    // was always there, and is outdated too (PLAN §8): its text says too little to follow it.
    let mut lines = twin_lines(40);
    lines[5] = "    let total = 0;".to_string();
    let total = anchor_at(&lines, 6, 6);
    let mut now = lines.clone();
    let line = now.remove(5);
    now.insert(30, line);
    assert_eq!(relocate(&total, &now), Relocation::Outdated);
}

/// A handler of 15 lines: a short debug line (7) with a twin in the next function (14), the
/// file ending on that function's brace (15).
fn handler() -> Vec<String> {
    [
        "use std::path::Path;",
        "",
        "fn handle(req: &Request) -> Result<Response> {",
        "    let user = lookup(req.user_id)?;",
        "    let limit = user.limit();",
        "    let items = fetch(req, limit)?;",
        "    log(\"fetched\");",
        "    let total = items.len();",
        "    let page = paginate(items, req.page);",
        "    respond(page, total)",
        "}",
        "",
        "fn other() {",
        "    log(\"fetched\");",
        "}",
    ]
    .map(String::from)
    .to_vec()
}

#[test]
fn a_line_that_says_enough_is_followed_where_it_moves_alone() {
    // One line, but a unique one: moved up past its context, it is found where it went.
    let lines = handler();
    let page = anchor_at(&lines, 9, 9);
    assert!(!is_slight(&page), "a long line isn't slight");
    let mut now = lines.clone();
    let line = now.remove(8);
    now.insert(5, line);
    assert_eq!(relocate(&page, &now), moved(6, 6, MoveKind::Nearby));
    // A short one, moved the same way, is too slight to follow (PLAN §8).
    let log = anchor_at(&lines, 7, 7);
    assert!(is_slight(&log));
    let mut now = lines.clone();
    let line = now.remove(6);
    now.insert(9, line);
    assert_eq!(relocate(&log, &now), Relocation::Outdated);
}

#[test]
fn a_files_last_line_keeps_its_thread_when_code_is_added_above_it() {
    let router: Vec<String> = [
        "const router = express.Router();",
        "router.get(\"/a\", a);",
        "router.get(\"/b\", b);",
        "module.exports = router;",
    ]
    .map(String::from)
    .to_vec();
    let export = anchor_at(&router, 4, 4);
    assert!(is_slight(&export) && export.context_after.is_empty());
    // Routes added above the last line, three or one: the thread stays on that line, alone.
    let mut three = router.clone();
    three.splice(
        3..3,
        [
            "router.get(\"/c\", c);",
            "router.get(\"/d\", d);",
            "router.get(\"/e\", e);",
        ]
        .map(String::from),
    );
    assert_eq!(relocate(&export, &three), moved(7, 7, MoveKind::Nearby));
    let mut one = router.clone();
    one.insert(3, "// one more route".to_string());
    assert_eq!(relocate(&export, &one), moved(5, 5, MoveKind::Nearby));
    // Lines appended after it leave it where it is; lines inserted at the top shift it.
    let lines = handler();
    let brace = anchor_at(&lines, 15, 15);
    let mut appended = lines.clone();
    appended.extend(["", "fn more() {}"].map(String::from));
    assert_eq!(relocate(&brace, &appended), Relocation::Unchanged);
    let shifted = insert(&lines, 2, 2);
    assert_eq!(relocate(&brace, &shifted), moved(17, 17, MoveKind::Nearby));
    // The price (PLAN §8): the last `}` deleted while a function is appended, whose own `}`
    // is the new last line, has the same shape as a function inserted above the last line,
    // and lands on the new `}`.
    let mut now = lines.clone();
    now.remove(14);
    now.extend(["fn more() {", "}"].map(String::from));
    assert_eq!(relocate(&brace, &now), moved(16, 16, MoveKind::Nearby));
}

#[test]
fn a_line_rewritten_into_a_block_where_it_stands_does_not_show_as_moved() {
    let lines = handler();
    let anchor = anchor_at(&lines, 4, 4);
    let thread = thread_on(anchor.clone());
    // `lookup` rewritten in place into a four-line `match`.
    let mut now = lines.clone();
    now.splice(
        3..4,
        [
            "    let user = match lookup(req.user_id) {",
            "        Some(user) => user,",
            "        None => return Err(Error::NoUser),",
            "    };",
        ]
        .map(String::from),
    );
    let modified = entry("a.rs", None, FileStatus::Modified);
    let text = |_: &FileEntry, _| SideText::Lines(now.clone());
    let placed = place_thread(
        &thread,
        std::slice::from_ref(&modified),
        History::None,
        text,
    );
    assert!(
        matches!(
            placed,
            Placement::Moved {
                start_line: 4,
                end_line: 7,
                ..
            }
        ),
        "{placed:?}"
    );
    assert!(!shows_moved(&thread, &placed), "it starts on its own line");
    assert!(shows_edited(&thread, &placed), "its text changed there");
    // Recorded by the next event, at lines 4–7: still not moved.
    let recorded = relocated_anchor(&anchor, &placed, &modified, &now, 2).expect("recorded");
    let recorded_thread = Thread {
        anchor: recorded,
        original_anchor: Some(anchor.clone()),
        ..thread.clone()
    };
    let placed = place_thread(
        &recorded_thread,
        std::slice::from_ref(&modified),
        History::None,
        text,
    );
    assert!(matches!(placed, Placement::Unchanged { .. }), "{placed:?}");
    assert!(!shows_moved(&recorded_thread, &placed));
    assert!(
        shows_edited(&recorded_thread, &placed),
        "not the text it was written on"
    );
    // Lines inserted above it as well: it starts elsewhere now, and that is a move.
    let shifted = insert(&now, 2, 3);
    let placed = place_thread(
        &recorded_thread,
        std::slice::from_ref(&modified),
        History::None,
        |_, _| SideText::Lines(shifted.clone()),
    );
    assert!(shows_moved(&recorded_thread, &placed), "{placed:?}");
    assert!(
        !shows_edited(&recorded_thread, &placed),
        "moved, not edited"
    );
}

#[test]
fn a_line_edited_where_it_stands_does_not_show_as_moved() {
    let lines = twin_lines(0);
    let anchor = anchor_at(&lines, 6, 6);
    let thread = thread_on(anchor.clone());
    let mut now = lines.clone();
    now[5] = "    println!(\"here: {}\", text.len());".to_string();
    let modified = entry("a.rs", None, FileStatus::Modified);
    let text = |_: &FileEntry, _| SideText::Lines(now.clone());
    let placed = place_thread(
        &thread,
        std::slice::from_ref(&modified),
        History::None,
        text,
    );
    assert!(matches!(
        placed,
        Placement::Moved {
            start_line: 6,
            end_line: 6,
            ..
        }
    ));
    assert!(!shows_moved(&thread, &placed), "its lines are its own");
    // The next event records the edited text at the same lines: still not moved.
    let recorded = relocated_anchor(&anchor, &placed, &modified, &now, 2).expect("recorded");
    let recorded_thread = Thread {
        anchor: recorded,
        original_anchor: Some(anchor.clone()),
        ..thread.clone()
    };
    let placed = place_thread(
        &recorded_thread,
        std::slice::from_ref(&modified),
        History::None,
        text,
    );
    assert!(matches!(placed, Placement::Unchanged { .. }));
    assert!(!shows_moved(&recorded_thread, &placed));
    // Moved back to the lines it was written on after a recorded move: not moved either.
    let elsewhere = Thread {
        anchor: Anchor {
            start_line: 20,
            end_line: 20,
            ..anchor.clone()
        },
        original_anchor: Some(anchor.clone()),
        ..thread.clone()
    };
    let back = Placement::Moved {
        file: "a.rs".to_string(),
        start_line: 6,
        end_line: 6,
        how: MoveKind::Nearby,
    };
    assert!(!shows_moved(&elsewhere, &back));
    assert!(shows_moved(
        &elsewhere,
        &Placement::Unchanged {
            file: "a.rs".to_string()
        }
    ));
}

#[test]
fn relocating_in_a_20000_line_file_is_quick() {
    let lines = code(FUZZY_MAX_LINES);
    let anchor = anchor_at(&lines, 15_000, 15_002);
    // The block moves 10,000 lines up with one line edited, and a line of its context too (so
    // the context doesn't stand whole anywhere): every step runs to the fuzzy one, which
    // scores every window.
    let mut now = lines.clone();
    let block: Vec<String> = now.drain(14_996..15_005).collect();
    now.splice(4_996..4_996, block);
    now[5_000] = format!("{} // edited", now[5_000]);
    now[4_996] = format!("{} // edited", now[4_996]);
    let started = Instant::now();
    let found = relocate(&anchor, &now);
    let elapsed = started.elapsed();
    assert_eq!(summary(found), Some((5_000, 5_002, "fuzzy")), "{found:?}");
    eprintln!("relocating in {FUZZY_MAX_LINES} lines took {elapsed:?}");
    // The budget holds for an optimised build (PLAN §8); a debug build is several times slower.
    #[cfg(not(debug_assertions))]
    assert!(
        elapsed < std::time::Duration::from_millis(50),
        "took {elapsed:?}"
    );

    // A long anchor costs no more: 200 lines moved far with one edited, and the same range
    // rewritten, so that every window is scored and none is good enough.
    let long = anchor_at(&lines, 15_000, 15_199);
    let mut far = lines.clone();
    let block: Vec<String> = far.drain(14_996..15_203).collect();
    far.splice(4_996..4_996, block);
    far[5_100] = format!("{} // edited", far[5_100]);
    far[4_996] = format!("{} // edited", far[4_996]);
    let started = Instant::now();
    let found = relocate(&long, &far);
    let moved_far = started.elapsed();
    assert_eq!(summary(found), Some((5_000, 5_199, "fuzzy")), "{found:?}");
    let mut rewritten = lines.clone();
    for line in &mut rewritten[14_990..15_210] {
        *line = line.chars().rev().collect();
    }
    let started = Instant::now();
    assert_eq!(relocate(&long, &rewritten), Relocation::Outdated);
    let outdated = started.elapsed();
    eprintln!("a 200-line anchor: moved far {moved_far:?}, outdated {outdated:?}");
    #[cfg(not(debug_assertions))]
    assert!(
        moved_far.max(outdated) < std::time::Duration::from_millis(50),
        "took {moved_far:?} / {outdated:?}"
    );

    // One line more and the fuzzy step is skipped.
    let mut longer = now.clone();
    longer.push("// one more".to_string());
    assert_eq!(relocate(&anchor, &longer), Relocation::Outdated);
}
