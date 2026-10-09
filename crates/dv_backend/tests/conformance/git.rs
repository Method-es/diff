//! `Backend::repo` and `Backend::git`.

use std::time::{Duration, Instant};

use dv_core::Location;
use dv_core::backend::{BackendError, GitOpts};
use dv_test_support::FixtureRepo;
use pretty_assertions::assert_eq;

use crate::{Subject, committed, same_dir};

crate::conformance!(
    repo_info_is_what_the_server_resolved,
    a_linked_worktree_resolves_its_own_git_dir,
    git_runs_in_the_worktree_root,
    a_failing_git_carries_its_command_and_stderr,
    ok_codes_decide_what_succeeds,
    git_reads_its_stdin,
    git_output_keeps_every_byte,
    a_timeout_kills_git,
    an_index_file_is_a_store_file,
);

fn repo_info_is_what_the_server_resolved(subject: &Subject) {
    let repo = committed();
    let local = crate::Subject::new(crate::Side::Local).open(&repo);
    for at in [
        repo.path().to_path_buf(),
        repo.path().join("sub").join("deep"),
    ] {
        let backend = subject.open_at(&repo, &at).unwrap();
        let info = backend.repo();
        // A server on this machine names the repository by its local path, like the
        // `LocalBackend` it is (a WSL host rewrites only `location`, `client` tests).
        assert_eq!(info, local.repo());
        assert!(same_dir(&info.toplevel, repo.path()), "{info:?}");
        assert_eq!(info.location, Location::Local(info.toplevel.clone().into()));
        assert!(!info.is_linked_worktree());
        assert!(info.git_version.at_least(2, 31), "{:?}", info.git_version);
    }
}

fn a_linked_worktree_resolves_its_own_git_dir(subject: &Subject) {
    let repo = committed();
    let worktree = repo.add_worktree("wt", "feat");
    let backend = subject.open_at(&repo, &worktree.join("sub")).unwrap();
    let info = backend.repo();
    assert!(same_dir(&info.toplevel, &worktree), "{info:?}");
    assert!(
        same_dir(
            &info.git_dir,
            repo.path().join(".git").join("worktrees").join("wt")
        ),
        "{info:?}"
    );
    assert!(
        same_dir(&info.common_dir, repo.path().join(".git")),
        "{info:?}"
    );
    assert!(info.is_linked_worktree());
}

fn git_runs_in_the_worktree_root(subject: &Subject) {
    let repo = committed();
    let backend = subject.open_at(&repo, &repo.path().join("sub")).unwrap();
    let output = backend.git(&["ls-files"], &GitOpts::default()).unwrap();
    assert_eq!(output.code, 0);
    assert_eq!(output.stdout_utf8(), Some("a.txt\nsub/deep/f.txt\n"));
    assert_eq!(output.stderr, b"");
    // The fixture's environment reaches the server's git.
    let name = backend
        .git(&["config", "user.name"], &GitOpts::default())
        .unwrap();
    assert_eq!(name.stdout_utf8(), Some("dv fixture\n"));
}

fn a_failing_git_carries_its_command_and_stderr(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let error = backend
        .git(
            &["rev-parse", "--verify", "no-such-rev"],
            &GitOpts::default(),
        )
        .unwrap_err();
    let BackendError::Git { args, code, stderr } = &error else {
        panic!("{error:?}");
    };
    assert_eq!(args, &["rev-parse", "--verify", "no-such-rev"]);
    assert_eq!(*code, Some(128));
    assert!(stderr.contains("fatal:"), "{stderr}");
    assert_eq!(error.to_string(), "git rev-parse failed (exit code 128)");
    let detail = error.detail().unwrap();
    assert!(
        detail.starts_with("$ git rev-parse --verify no-such-rev\nexit code 128\nfatal:"),
        "{detail}"
    );
}

fn ok_codes_decide_what_succeeds(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    repo.write("a.txt", "changed\n");
    let quiet = ["diff", "--quiet", "HEAD", "--", "a.txt"];
    let error = backend.git(&quiet, &GitOpts::default()).unwrap_err();
    assert!(
        matches!(error, BackendError::Git { code: Some(1), .. }),
        "{error:?}"
    );
    let output = backend
        .git(&quiet, &GitOpts::new().with_ok_codes(&[0, 1]))
        .unwrap();
    assert_eq!(output.code, 1);
}

fn git_reads_its_stdin(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let hashed = backend
        .git(
            &["hash-object", "--stdin"],
            &GitOpts::new().with_stdin("a\n"),
        )
        .unwrap();
    assert_eq!(
        hashed.stdout_utf8().unwrap().trim(),
        repo.git(&["rev-parse", "HEAD:a.txt"]).trim()
    );
    // Bytes that aren't UTF-8 reach git intact.
    let bytes: Vec<u8> = (0..=255u8).rev().collect();
    let hashed = backend
        .git(
            &["hash-object", "--stdin"],
            &GitOpts::new().with_stdin(bytes.clone()),
        )
        .unwrap();
    let oid = hashed.stdout_utf8().unwrap().trim().to_string();
    let stored = backend
        .git(
            &["hash-object", "-w", "--stdin"],
            &GitOpts::new().with_stdin(bytes.clone()),
        )
        .unwrap();
    assert_eq!(stored.stdout_utf8().unwrap().trim(), oid);
    let read = backend
        .git(&["cat-file", "blob", &oid], &GitOpts::default())
        .unwrap();
    assert_eq!(read.stdout, bytes);
}

fn git_output_keeps_every_byte(subject: &Subject) {
    let repo = FixtureRepo::new();
    let mut blob = vec![0u8, 0xff, b'\n', b'\r', 0x80];
    blob.extend(std::iter::repeat_n(b'x', 300_000));
    repo.write("blob.bin", &blob);
    repo.commit("binary");
    let backend = subject.open(&repo);
    let output = backend
        .git(&["cat-file", "blob", "HEAD:blob.bin"], &GitOpts::default())
        .unwrap();
    assert_eq!(output.stdout.len(), blob.len());
    assert_eq!(output.stdout, blob);
    // And stderr, with an exit code git chose.
    let error = backend
        .git(&["cat-file", "blob", "HEAD:missing"], &GitOpts::default())
        .unwrap_err();
    let BackendError::Git { stderr, .. } = &error else {
        panic!("{error:?}");
    };
    assert!(stderr.contains("missing"), "{stderr}");
}

fn a_timeout_kills_git(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let started = Instant::now();
    let error = backend
        .git(
            &["-c", "alias.slow=!sleep 3", "slow"],
            &GitOpts::new().with_timeout(Duration::from_millis(300)),
        )
        .unwrap_err();
    assert!(
        matches!(
            error,
            BackendError::Timeout {
                timeout_ms: 300,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(error.to_string(), "git slow timed out after 300 ms");
    assert_eq!(
        error.detail().as_deref(),
        Some("$ git -c alias.slow=!sleep 3 slow")
    );
}

fn an_index_file_is_a_store_file(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let index = repo.path().join(".git").join("index");
    let before = std::fs::read(&index).unwrap();
    repo.write("new.txt", "new\n");

    assert!(backend.copy_index_to_store("tmp/snapshot.index").unwrap());
    let opts = GitOpts::new().with_index_file("tmp/snapshot.index");
    backend.git(&["add", "--", "new.txt"], &opts).unwrap();
    let staged = backend
        .git(&["diff", "--cached", "--name-only"], &opts)
        .unwrap();
    assert_eq!(staged.stdout_utf8(), Some("new.txt\n"));
    // The worktree's own index never changed.
    let own = backend
        .git(&["diff", "--cached", "--name-only"], &GitOpts::default())
        .unwrap();
    assert_eq!(own.stdout_utf8(), Some(""));
    assert_eq!(std::fs::read(&index).unwrap(), before);

    // The index file is a store path, validated as one.
    let error = backend
        .git(&["status"], &GitOpts::new().with_index_file("../index"))
        .unwrap_err();
    assert!(
        matches!(error, BackendError::InvalidPath { .. }),
        "{error:?}"
    );
}
