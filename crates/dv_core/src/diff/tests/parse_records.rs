//! Pure tests of the `--raw --numstat -z` record parser and the join (PLAN §6 step 1).

use super::*;
use crate::diff::list::{NumstatRecord, RawRecord, join, parse_raw_numstat};

const A: &str = "1111111111111111111111111111111111111111";
const B: &str = "2222222222222222222222222222222222222222";
const ZERO: &str = "0000000000000000000000000000000000000000";

fn raw(header: &str, paths: &[&str]) -> String {
    let mut out = format!(":{header}\0");
    for path in paths {
        out.push_str(path);
        out.push('\0');
    }
    out
}

#[test]
fn parses_every_status_with_modes_and_ids() {
    let output = [
        raw(&format!("100644 100644 {A} {B} M"), &["m.txt"]),
        raw(&format!("000000 100755 {ZERO} {B} A"), &["run.sh"]),
        raw(&format!("100644 000000 {A} {ZERO} D"), &["gone.txt"]),
        raw(
            &format!("100644 100644 {A} {B} R087"),
            &["old name", "new name"],
        ),
        raw(&format!("100644 100644 {A} {A} C100"), &["src", "copy"]),
        raw(&format!("100644 120000 {A} {B} T"), &["link"]),
        raw(&format!("000000 000000 {ZERO} {ZERO} U"), &["conflict"]),
        raw(&format!("160000 160000 {A} {B} M"), &["sub"]),
        "1\t2\tm.txt\0".to_string(),
        "3\t0\trun.sh\0".to_string(),
        "0\t4\tgone.txt\0".to_string(),
        "5\t6\t\0old name\0new name\0".to_string(),
        "0\t0\t\0src\0copy\0".to_string(),
        "-\t-\tlink\0".to_string(),
        "0\t0\tconflict\0".to_string(),
        "1\t1\tsub\0".to_string(),
    ]
    .concat();
    let (raw, numstat) = parse_raw_numstat(output.as_bytes()).unwrap();
    assert_eq!(raw.len(), 8);
    assert_eq!(numstat.len(), 8);
    assert_eq!(
        raw[3],
        RawRecord {
            old_mode: Some(FileMode::Regular),
            new_mode: Some(FileMode::Regular),
            old_oid: Some(Oid::parse(A).unwrap()),
            new_oid: Some(Oid::parse(B).unwrap()),
            status: FileStatus::Renamed { similarity: 87 },
            old_path: Some("old name".to_string()),
            path: "new name".to_string(),
        }
    );
    assert_eq!(
        numstat[3],
        NumstatRecord {
            additions: Some(5),
            deletions: Some(6),
            old_path: Some("old name".to_string()),
            path: "new name".to_string(),
        }
    );

    let entries = join(raw, numstat).unwrap();
    let summary: Vec<_> = entries
        .iter()
        .map(|e| {
            (
                e.path.as_str(),
                e.old_path.as_deref(),
                e.status,
                e.additions,
                e.deletions,
                e.binary,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("m.txt", None, FileStatus::Modified, 1, 2, false),
            ("run.sh", None, FileStatus::Added, 3, 0, false),
            ("gone.txt", None, FileStatus::Deleted, 0, 4, false),
            (
                "new name",
                Some("old name"),
                FileStatus::Renamed { similarity: 87 },
                5,
                6,
                false
            ),
            (
                "copy",
                Some("src"),
                FileStatus::Copied { similarity: 100 },
                0,
                0,
                false
            ),
            ("link", None, FileStatus::TypeChanged, 0, 0, true),
            ("conflict", None, FileStatus::Unmerged, 0, 0, false),
            ("sub", None, FileStatus::Modified, 1, 1, false),
        ]
    );
    assert_eq!(entries[1].old_mode, None);
    assert_eq!(entries[1].new_mode, Some(FileMode::Executable));
    assert_eq!(entries[1].old_oid, None);
    assert_eq!(entries[2].new_oid, None);
    assert_eq!(entries[5].new_mode, Some(FileMode::Symlink));
    assert!(entries[7].is_submodule());
}

#[test]
fn a_stat_dirty_entry_without_numstat_is_dropped() {
    // `M` with an unhashed new side and no numstat record: content unchanged (PLAN §6 step 1).
    let output = [
        raw(&format!("100644 100644 {A} {ZERO} M"), &["dirty.txt"]),
        raw(&format!("100644 100644 {A} {ZERO} M"), &["real.txt"]),
        "1\t1\treal.txt\0".to_string(),
    ]
    .concat();
    let (raw, numstat) = parse_raw_numstat(output.as_bytes()).unwrap();
    let entries = join(raw, numstat).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "real.txt");
}

#[test]
fn the_same_path_twice_joins_in_order() {
    // The index against the working tree lists an unmerged path twice (`U` and `M`).
    let output = [
        raw(&format!("000000 000000 {ZERO} {ZERO} U"), &["c.txt"]),
        raw(&format!("100644 100644 {A} {ZERO} M"), &["c.txt"]),
        "0\t0\tc.txt\0".to_string(),
        "4\t0\tc.txt\0".to_string(),
    ]
    .concat();
    let (raw, numstat) = parse_raw_numstat(output.as_bytes()).unwrap();
    let entries = join(raw, numstat).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        (entries[0].status, entries[0].additions),
        (FileStatus::Unmerged, 0)
    );
    assert_eq!(
        (entries[1].status, entries[1].additions),
        (FileStatus::Modified, 4)
    );
}

#[test]
fn paths_are_taken_verbatim() {
    // `-z` never quotes: tabs, quotes, colons, leading dashes and unicode arrive as they are.
    let odd = [
        "-dash.txt",
        "tab\there.txt",
        "q\"uote.txt",
        ":colon",
        "ünï cødé.txt",
    ];
    let mut output = String::new();
    for path in odd {
        output.push_str(&raw(&format!("100644 100644 {A} {B} M"), &[path]));
    }
    for path in odd {
        output.push_str(&format!("1\t1\t{path}\0"));
    }
    let (raw, numstat) = parse_raw_numstat(output.as_bytes()).unwrap();
    let entries = join(raw, numstat).unwrap();
    let paths: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, odd);
}

#[test]
fn malformed_records_are_errors() {
    for bad in [
        format!(":100644 100644 {A} {B}\0x\0"),
        format!(":100644 100644 {A} {B} Q\0x\0"),
        format!(":100644 100644 {A} {B} R\0x\0y\0"),
        format!(":100644 100644 {A} {B} R101\0x\0y\0"),
        format!(":100644 100644 {A} {B} R50\0x\0"),
        format!(":100644 100644 {A} {B} M\0"),
        format!(":100644 040000 {A} {B} M\0x\0"),
        format!(":100644 100644 abc {B} M\0x\0"),
        "1\t2\0".to_string(),
        "x\t2\tpath\0".to_string(),
        "1\t2\t\0only-one\0".to_string(),
    ] {
        assert!(parse_raw_numstat(bad.as_bytes()).is_err(), "{bad:?}");
    }
    // A numstat record no raw record matches.
    let (raw, numstat) = parse_raw_numstat(b"1\t1\tstray\0").unwrap();
    assert!(join(raw, numstat).is_err());
    assert_eq!(parse_raw_numstat(b"").unwrap(), (vec![], vec![]));
}

#[test]
fn modes_parse_from_octal() {
    assert_eq!(FileMode::parse_octal("000000"), Some(None));
    assert_eq!(
        FileMode::parse_octal("100644"),
        Some(Some(FileMode::Regular))
    );
    assert_eq!(
        FileMode::parse_octal("100755"),
        Some(Some(FileMode::Executable))
    );
    assert_eq!(
        FileMode::parse_octal("100664"),
        Some(Some(FileMode::Regular))
    );
    assert_eq!(
        FileMode::parse_octal("120000"),
        Some(Some(FileMode::Symlink))
    );
    assert_eq!(
        FileMode::parse_octal("160000"),
        Some(Some(FileMode::Submodule))
    );
    assert_eq!(FileMode::parse_octal("040000"), None);
    assert_eq!(FileMode::parse_octal("10064x"), None);
}

#[test]
fn side_args_cover_every_supported_pair() {
    let repo = dv_test_support::FixtureRepo::new();
    let git = super::git_repo(&repo);
    let a = Oid::parse(A).unwrap();
    let empty = git.empty_tree().to_string();
    let args = |old: Side, new: Side| side_args(&git, &DiffSpec { old, new });
    assert_eq!(
        args(Side::Commit(a.clone()), Side::Commit(a.clone())).unwrap(),
        [A, A]
    );
    assert_eq!(
        args(Side::EmptyTree, Side::Commit(a.clone())).unwrap(),
        [empty.as_str(), A]
    );
    assert_eq!(
        args(Side::Commit(a.clone()), Side::Index).unwrap(),
        ["--cached", A]
    );
    assert_eq!(
        args(Side::EmptyTree, Side::Worktree).unwrap(),
        [empty.as_str()]
    );
    assert!(args(Side::Index, Side::Worktree).unwrap().is_empty());
    for (old, new) in [
        (Side::Worktree, Side::Commit(a.clone())),
        (Side::Index, Side::Commit(a.clone())),
        (Side::Worktree, Side::Index),
        (Side::Index, Side::Index),
    ] {
        let err = args(old, new).unwrap_err();
        assert!(matches!(err, DiffError::UnsupportedSpec { .. }), "{err:?}");
    }
}
