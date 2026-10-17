//! The worktree reads and the review store.

use std::collections::HashSet;
use std::sync::{Arc, Barrier};

use dv_core::backend::{BackendError, PROBE_READ_MAX, PROBE_READ_SMALL, PathProbe, TextStats};
use dv_test_support::FixtureRepo;
use pretty_assertions::assert_eq;

use crate::{Subject, committed};

crate::conformance!(
    worktree_reads_and_sizes,
    a_large_file_crosses_whole,
    binary_files_keep_every_byte,
    text_stats_count_like_git,
    symbolic_links_read_as_their_targets,
    absolute_paths_probe_outside_the_worktree,
    store_files_read_list_replace_and_remove,
    appends_from_two_threads_never_interleave,
    the_index_copies_into_the_store,
    an_unborn_repo_has_no_index_to_copy,
    paths_are_validated_before_use,
);

fn worktree_reads_and_sizes(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    assert_eq!(
        backend.worktree_read("a.txt").unwrap(),
        Some(b"a\n".to_vec())
    );
    assert_eq!(backend.worktree_len("sub/deep/f.txt").unwrap(), Some(2));
    assert_eq!(backend.worktree_read("missing.txt").unwrap(), None);
    assert_eq!(backend.worktree_len("missing/x").unwrap(), None);
    assert_eq!(backend.worktree_text_stats("missing.txt").unwrap(), None);
    assert_eq!(backend.worktree_read("a.txt/below-a-file").unwrap(), None);
    // An empty file is there, with nothing in it.
    repo.write("empty.txt", "");
    assert_eq!(
        backend.worktree_read("empty.txt").unwrap(),
        Some(Vec::new())
    );
    assert_eq!(backend.worktree_len("empty.txt").unwrap(), Some(0));
    for dir in ["sub", "sub/deep"] {
        assert!(matches!(
            backend.worktree_read(dir),
            Err(BackendError::Io { .. })
        ));
        assert!(matches!(
            backend.worktree_len(dir),
            Err(BackendError::Io { .. })
        ));
        assert!(matches!(
            backend.worktree_text_stats(dir),
            Err(BackendError::Io { .. })
        ));
    }
}

fn a_large_file_crosses_whole(subject: &Subject) {
    let repo = committed();
    // 5 MB of numbered lines, the last without a newline.
    let mut big = String::with_capacity(5_300_000);
    let mut n = 0u64;
    while big.len() < 5 * 1024 * 1024 {
        big.push_str(&format!("line {n:08} of a large file\n"));
        n += 1;
    }
    big.push_str("the end");
    repo.write("big.txt", &big);
    let backend = subject.open(&repo);
    let read = backend.worktree_read("big.txt").unwrap().unwrap();
    assert_eq!(read.len(), big.len());
    assert!(read == big.as_bytes(), "the bytes differ");
    assert_eq!(
        backend.worktree_len("big.txt").unwrap(),
        Some(big.len() as u64)
    );
    assert_eq!(
        backend.worktree_text_stats("big.txt").unwrap(),
        Some(TextStats {
            len: big.len() as u64,
            lines: n + 1,
            binary: false,
            symlink: false,
        })
    );
    // The store carries as much.
    backend
        .store_write_atomic("cache/big", big.as_bytes())
        .unwrap();
    assert!(backend.store_read("cache/big").unwrap().unwrap() == big.as_bytes());
}

fn binary_files_keep_every_byte(subject: &Subject) {
    let repo = committed();
    let mut binary: Vec<u8> = (0..=255u8).collect();
    binary.extend(std::iter::repeat_n(0u8, 100));
    binary.extend(std::iter::repeat_n(b'x', 200_000));
    repo.write("blob.bin", &binary);
    let backend = subject.open(&repo);
    assert_eq!(
        backend.worktree_read("blob.bin").unwrap(),
        Some(binary.clone())
    );
    assert_eq!(
        backend.worktree_text_stats("blob.bin").unwrap(),
        Some(TextStats {
            len: binary.len() as u64,
            lines: 0,
            binary: true,
            symlink: false,
        })
    );
}

fn text_stats_count_like_git(subject: &Subject) {
    let repo = committed();
    repo.write("crlf.txt", "a\r\nb\r\n")
        .write("no-newline.txt", "one\ntwo")
        .write("blank.txt", "\n\n");
    let backend = subject.open(&repo);
    let stats = |rel: &str| backend.worktree_text_stats(rel).unwrap().unwrap();
    assert_eq!((stats("crlf.txt").len, stats("crlf.txt").lines), (6, 2));
    assert_eq!(stats("no-newline.txt").lines, 2);
    assert_eq!(stats("blank.txt").lines, 2);
    assert!(!stats("a.txt").symlink);
}

/// Makes a symbolic link at `name` to `target`; `false` where this machine doesn't let the
/// test (Windows without developer mode).
fn symlink(repo: &FixtureRepo, target: &str, name: &str, dir: bool) -> bool {
    let link = repo.path().join(name);
    #[cfg(unix)]
    {
        let _ = dir;
        std::os::unix::fs::symlink(target, link).unwrap();
        true
    }
    #[cfg(windows)]
    {
        let made = if dir {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        };
        match made {
            Ok(()) => true,
            Err(error) => {
                eprintln!("skipped: this machine can't make symbolic links ({error})");
                false
            }
        }
    }
}

fn symbolic_links_read_as_their_targets(subject: &Subject) {
    let repo = committed();
    if !symlink(&repo, "a.txt", "link", false)
        || !symlink(&repo, "nonexistent/x", "dangling", false)
        || !symlink(&repo, "sub", "dirlink", true)
    {
        return;
    }
    let backend = subject.open(&repo);
    for (path, target) in [
        ("link", "a.txt"),
        ("dangling", "nonexistent/x"),
        ("dirlink", "sub"),
    ] {
        assert_eq!(
            backend.worktree_read(path).unwrap(),
            Some(target.as_bytes().to_vec()),
            "{path}"
        );
        assert_eq!(
            backend.worktree_len(path).unwrap(),
            Some(target.len() as u64),
            "{path}"
        );
        assert_eq!(
            backend.worktree_text_stats(path).unwrap(),
            Some(TextStats {
                len: target.len() as u64,
                lines: 1,
                binary: false,
                symlink: true,
            }),
            "{path}"
        );
    }
}

fn absolute_paths_probe_outside_the_worktree(subject: &Subject) {
    let repo = committed();
    let outside = repo.outside_dir("store");
    std::fs::create_dir_all(outside.join("sub")).unwrap();
    std::fs::write(outside.join("package.json"), br#"{"version": "7.0.2"}"#).unwrap();
    let big = vec![b'x'; PROBE_READ_MAX as usize + 1];
    std::fs::write(outside.join("big.bin"), &big).unwrap();
    let backend = subject.open(&repo);
    let at = |name: &str| outside.join(name).to_str().unwrap().to_string();
    assert_eq!(
        backend
            .probe_path(outside.to_str().unwrap(), PROBE_READ_MAX)
            .unwrap(),
        PathProbe::Dir
    );
    assert_eq!(
        backend
            .probe_path(&at("package.json"), PROBE_READ_SMALL)
            .unwrap(),
        PathProbe::File {
            len: 20,
            contents: Some(br#"{"version": "7.0.2"}"#.to_vec())
        }
    );
    // Only looking: no bytes; and none past what was asked for.
    for read in [0, 19] {
        assert_eq!(
            backend.probe_path(&at("package.json"), read).unwrap(),
            PathProbe::File {
                len: 20,
                contents: None
            },
            "{read}"
        );
    }
    assert_eq!(
        backend.probe_path(&at("big.bin"), u64::MAX).unwrap(),
        PathProbe::File {
            len: big.len() as u64,
            contents: None
        }
    );
    assert_eq!(
        backend.probe_path(&at("missing"), 0).unwrap(),
        PathProbe::Missing
    );
    assert_eq!(
        backend.probe_path(&at("package.json/below"), 0).unwrap(),
        PathProbe::Missing
    );
    // The worktree itself is a path like any other.
    assert_eq!(
        backend.probe_path(&backend.repo().toplevel, 0).unwrap(),
        PathProbe::Dir
    );
    let separator = std::path::MAIN_SEPARATOR;
    for bad in [
        "relative/x".to_string(),
        format!("{}{separator}..{separator}x", outside.to_str().unwrap()),
        format!("{}{separator}", outside.to_str().unwrap()),
        String::new(),
    ] {
        assert!(
            matches!(
                backend.probe_path(&bad, 0),
                Err(BackendError::InvalidPath { .. })
            ),
            "{bad:?} should be refused"
        );
    }
    let link = outside.join("link");
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink("sub", &link);
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir("sub", &link);
    match made {
        Ok(()) => assert_eq!(
            backend.probe_path(link.to_str().unwrap(), 0).unwrap(),
            PathProbe::Link {
                target: "sub".into()
            }
        ),
        Err(error) => eprintln!("skipped the link: this machine can't make one ({error})"),
    }
}

fn store_files_read_list_replace_and_remove(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let rel = "reviews/r-1.jsonl";
    assert_eq!(backend.store_read(rel).unwrap(), None);
    assert_eq!(backend.store_list("reviews").unwrap(), Vec::<String>::new());

    backend.store_append(rel, br#"{"n":1}"#).unwrap();
    backend.store_append(rel, br#"{"n":2}"#).unwrap();
    assert_eq!(
        backend.store_read(rel).unwrap().unwrap(),
        b"{\"n\":1}\n{\"n\":2}\n"
    );
    let on_disk = repo
        .path()
        .join(".git")
        .join("dv")
        .join("reviews")
        .join("r-1.jsonl");
    assert_eq!(std::fs::read(on_disk).unwrap(), b"{\"n\":1}\n{\"n\":2}\n");

    backend
        .store_write_atomic("reviews/r-1.state.json", b"old")
        .unwrap();
    backend
        .store_write_atomic("reviews/r-1.state.json", b"new")
        .unwrap();
    assert_eq!(
        backend
            .store_read("reviews/r-1.state.json")
            .unwrap()
            .unwrap(),
        b"new"
    );
    // Bytes that aren't text survive the store too.
    backend
        .store_write_atomic("cache/raw", &[0, 255, 10, 13])
        .unwrap();
    assert_eq!(
        backend.store_read("cache/raw").unwrap().unwrap(),
        [0, 255, 10, 13]
    );

    // Directories and dot files (temp files mid-rename) aren't listed.
    let store = repo.path().join(".git").join("dv");
    std::fs::create_dir_all(store.join("reviews").join("nested")).unwrap();
    std::fs::write(store.join("reviews").join(".dv-tmp-x"), "").unwrap();
    assert_eq!(
        backend.store_list("reviews").unwrap(),
        ["r-1.jsonl", "r-1.state.json"]
    );
    assert_eq!(backend.store_list("").unwrap(), Vec::<String>::new());
    assert_eq!(backend.store_list("missing").unwrap(), Vec::<String>::new());

    assert!(backend.store_remove("reviews/r-1.state.json").unwrap());
    assert!(!backend.store_remove("reviews/r-1.state.json").unwrap());
    assert_eq!(backend.store_list("reviews").unwrap(), ["r-1.jsonl"]);
}

const LINES_PER_WRITER: usize = 200;

/// A line whose length and padding depend on its writer and number, so a torn or interleaved
/// write can't parse back into the same line.
fn append_line(writer: &str, n: usize) -> String {
    let fill = if writer == "w0" { 'a' } else { 'b' };
    let pad = fill.to_string().repeat(100 + (n * 37) % 900);
    format!(r#"{{"w":"{writer}","n":{n},"pad":"{pad}"}}"#)
}

fn appends_from_two_threads_never_interleave(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let rel = "reviews/r-concurrent.jsonl";
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = ["w0", "w1"]
        .into_iter()
        .map(|writer| {
            let (backend, barrier) = (Arc::clone(&backend), Arc::clone(&barrier));
            std::thread::spawn(move || {
                barrier.wait();
                for n in 0..LINES_PER_WRITER {
                    backend
                        .store_append(rel, append_line(writer, n).as_bytes())
                        .unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let text = String::from_utf8(backend.store_read(rel).unwrap().unwrap()).unwrap();
    assert!(text.ends_with('\n'));
    let mut seen = HashSet::new();
    for line in text.lines() {
        let value: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|err| panic!("a torn or interleaved line ({err}): {line:.120}"));
        let writer = value["w"].as_str().unwrap().to_string();
        let n = usize::try_from(value["n"].as_u64().unwrap()).unwrap();
        assert_eq!(line, append_line(&writer, n), "line content intact");
        assert!(seen.insert((writer, n)), "duplicate line {line:.60}");
    }
    assert_eq!(seen.len(), 2 * LINES_PER_WRITER);
}

fn the_index_copies_into_the_store(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let index = repo.path().join(".git").join("index");
    assert!(backend.copy_index_to_store("tmp/index.copy").unwrap());
    let copy = repo
        .path()
        .join(".git")
        .join("dv")
        .join("tmp")
        .join("index.copy");
    assert_eq!(
        std::fs::read(&copy).unwrap(),
        std::fs::read(&index).unwrap()
    );
    let modified = |path: &std::path::Path| std::fs::metadata(path).unwrap().modified().unwrap();
    assert_eq!(
        modified(&copy),
        modified(&index),
        "the copy keeps the index's time"
    );
}

fn an_unborn_repo_has_no_index_to_copy(subject: &Subject) {
    let repo = FixtureRepo::new();
    let backend = subject.open(&repo);
    assert!(!backend.copy_index_to_store("tmp/index.copy").unwrap());
    // Its parent is made either way.
    assert!(repo.path().join(".git").join("dv").join("tmp").is_dir());
}

fn paths_are_validated_before_use(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let outside = repo.outside_dir("outside");
    std::fs::write(outside.join("secret"), "secret").unwrap();
    let invalid =
        |result: Result<(), BackendError>| matches!(result, Err(BackendError::InvalidPath { .. }));
    for rel in [
        "../outside/secret",
        "/etc/passwd",
        "sub\\..\\..\\x",
        "",
        "a.txt/",
        "./a.txt",
    ] {
        assert!(invalid(backend.worktree_read(rel).map(drop)), "{rel:?}");
        assert!(invalid(backend.worktree_len(rel).map(drop)), "{rel:?}");
        assert!(
            invalid(backend.worktree_text_stats(rel).map(drop)),
            "{rel:?}"
        );
        assert!(invalid(backend.store_append(rel, b"x")), "{rel:?}");
        assert!(invalid(backend.store_read(rel).map(drop)), "{rel:?}");
        assert!(invalid(backend.store_write_atomic(rel, b"x")), "{rel:?}");
        assert!(invalid(backend.store_remove(rel).map(drop)), "{rel:?}");
        assert!(
            invalid(backend.copy_index_to_store(rel).map(drop)),
            "{rel:?}"
        );
    }
    for rel in ["../x", "/x", "a\\b"] {
        assert!(invalid(backend.store_list(rel).map(drop)), "{rel:?}");
    }
    let error = backend
        .store_append("reviews/r.jsonl", b"two\nlines")
        .unwrap_err();
    assert!(
        matches!(error, BackendError::InvalidLine { .. }),
        "{error:?}"
    );
    assert_eq!(error.to_string(), "invalid store line: contains a newline");
}
