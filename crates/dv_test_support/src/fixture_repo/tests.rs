use super::*;

fn panic_message(run: impl FnOnce()) -> String {
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
        .expect_err("the call should have panicked");
    match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => payload
            .downcast::<&str>()
            .map(|message| message.to_string())
            .unwrap_or_default(),
    }
}

fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

#[test]
fn a_new_repo_is_empty_on_main_with_the_fixture_settings() {
    let repo = FixtureRepo::new();
    assert!(repo.path().join(".git").is_dir());
    assert_eq!(repo.git(&["symbolic-ref", "HEAD"]), "refs/heads/main\n");
    assert_eq!(repo.git(&["rev-list", "--all"]), "");
    for (key, value) in LOCAL_CONFIG {
        assert_eq!(repo.git(&["config", key]).trim(), *value, "{key}");
    }
}

#[test]
fn only_the_repo_config_applies_to_fixture_git_calls() {
    let repo = FixtureRepo::new();
    let listing = repo.git(&["config", "--list", "--show-origin"]);
    assert!(!listing.is_empty());
    for line in listing.lines() {
        assert!(
            line.starts_with("file:.git/config\t"),
            "a setting from outside the repo leaked in: {line}"
        );
    }
}

#[test]
fn commits_carry_the_fixed_identity_and_consecutive_dates() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "one\n");
    let first = repo.commit("first");
    repo.write("a.txt", "two\n");
    let second = repo.commit("second");

    assert_eq!(first.len(), 40, "{first}");
    assert!(first.chars().all(|c| c.is_ascii_hexdigit()), "{first}");
    assert_eq!(repo.git(&["rev-parse", "HEAD~1"]).trim(), first);
    assert_eq!(repo.git(&["rev-parse", "HEAD"]).trim(), second);
    let log = repo.git(&["log", "--format=%an <%ae> %at %cn <%ce> %ct %s"]);
    assert_eq!(
        lines(&log),
        [
            format!(
                "{AUTHOR_NAME} <{AUTHOR_EMAIL}> {} {AUTHOR_NAME} <{AUTHOR_EMAIL}> {} second",
                EPOCH + TICK_SECS,
                EPOCH + TICK_SECS
            ),
            format!(
                "{AUTHOR_NAME} <{AUTHOR_EMAIL}> {EPOCH} {AUTHOR_NAME} <{AUTHOR_EMAIL}> {EPOCH} first"
            ),
        ]
    );
}

#[test]
fn the_same_script_gives_the_same_commit_ids() {
    let build = || {
        let repo = FixtureRepo::new();
        repo.write("src/lib.rs", "fn a() {}\n")
            .set_attributes("*.bin binary\n");
        let base = repo.commit("base");
        repo.branch("feat")
            .checkout("feat")
            .write("src/lib.rs", "fn b() {}\n");
        (base, repo.commit("change"))
    };
    assert_eq!(build(), build());
}

/// A file that only a `git init` template would put into `.git`.
const TEMPLATE_MARKER: &str = "dv-template-marker";

/// Checks a fixture commit's identity, its object id format and that no `git init` template was
/// applied, in whatever environment this test process has. On its own that is the normal one;
/// `inherited_git_variables_do_not_reach_the_fixture` runs it in a child process whose
/// environment exports another identity, a template and SHA-256.
#[test]
#[ignore = "run in a hostile git environment by inherited_git_variables_do_not_reach_the_fixture"]
fn fixture_commits_ignore_the_inherited_git_environment() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    let oid = repo.commit("first");
    assert_eq!(oid.len(), 40, "SHA-1 object ids: {oid}");
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an <%ae> %cn <%ce>"])
            .trim(),
        format!("{AUTHOR_NAME} <{AUTHOR_EMAIL}> {AUTHOR_NAME} <{AUTHOR_EMAIL}>")
    );
    assert!(
        !repo.path().join(".git").join(TEMPLATE_MARKER).exists(),
        "an inherited git init template was applied"
    );
}

#[test]
fn inherited_git_variables_do_not_reach_the_fixture() {
    // `git commit` exports GIT_AUTHOR_* to its hooks, and a shell or CI image can export any of
    // these, so a test run from either must still produce the fixture's commits.
    let template = tempfile::tempdir().unwrap();
    std::fs::write(template.path().join(TEMPLATE_MARKER), "from a template\n").unwrap();
    let test = "fixture_repo::tests::fixture_commits_ignore_the_inherited_git_environment";
    let output = Command::new(std::env::current_exe().unwrap())
        .args([test, "--exact", "--ignored", "--test-threads=1"])
        .env("GIT_AUTHOR_NAME", "Inherited Author")
        .env("GIT_AUTHOR_EMAIL", "author@inherited.invalid")
        .env("GIT_COMMITTER_NAME", "Inherited Committer")
        .env("GIT_COMMITTER_EMAIL", "committer@inherited.invalid")
        .env("GIT_TEMPLATE_DIR", template.path())
        .env("GIT_DEFAULT_HASH", "sha256")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains(" 1 passed"),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn commit_records_additions_changes_and_deletions() {
    let repo = FixtureRepo::new();
    repo.write("keep.txt", "keep\n").write("gone.txt", "gone\n");
    repo.commit("base");
    repo.write("keep.txt", "changed\n")
        .write("dir/new.txt", "new\n")
        .remove("gone.txt");
    repo.commit("change");

    let changes = repo.git(&["show", "--name-status", "--format=", "HEAD"]);
    assert_eq!(
        lines(&changes),
        ["A\tdir/new.txt", "D\tgone.txt", "M\tkeep.txt"]
    );
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
}

#[test]
fn remove_takes_whole_directories() {
    let repo = FixtureRepo::new();
    repo.write("dir/a.txt", "a\n").write("dir/sub/b.txt", "b\n");
    repo.commit("base");
    repo.remove("dir");
    assert!(!repo.path().join("dir").exists());
    repo.write("other.txt", "x\n");
    repo.commit("remove dir");
    let changes = repo.git(&["show", "--name-status", "--format=", "HEAD"]);
    assert_eq!(
        lines(&changes),
        ["D\tdir/a.txt", "D\tdir/sub/b.txt", "A\tother.txt"]
    );
}

#[test]
fn an_unchanged_move_is_a_rename() {
    let repo = FixtureRepo::new();
    repo.write("old/name.txt", "same content\nacross the move\n");
    repo.commit("base");
    repo.rename("old/name.txt", "new/dir/name.txt");
    repo.commit("move");
    let changes = repo.git(&["show", "-M", "--name-status", "--format=", "HEAD"]);
    assert_eq!(changes, "R100\told/name.txt\tnew/dir/name.txt\n");
}

#[test]
fn write_keeps_bytes_and_write_crlf_makes_every_ending_crlf() {
    let repo = FixtureRepo::new();
    repo.write("bin.dat", [0u8, 159, 146, 150, b'\n', 0])
        .write_crlf("crlf.txt", "a\nb\r\nc\n");
    assert_eq!(
        std::fs::read(repo.path().join("bin.dat")).unwrap(),
        [0u8, 159, 146, 150, b'\n', 0]
    );
    assert_eq!(
        std::fs::read(repo.path().join("crlf.txt")).unwrap(),
        b"a\r\nb\r\nc\r\n"
    );
}

#[test]
fn crlf_is_committed_as_is_until_autocrlf_is_set() {
    let repo = FixtureRepo::new();
    repo.write_crlf("as_is.txt", "one\ntwo\n");
    repo.commit("as is");
    assert_eq!(
        repo.git(&["cat-file", "-p", "HEAD:as_is.txt"]),
        "one\r\ntwo\r\n"
    );

    repo.config("core.autocrlf", "true")
        .write_crlf("normalised.txt", "one\ntwo\n");
    repo.commit("normalised");
    assert_eq!(
        repo.git(&["cat-file", "-p", "HEAD:normalised.txt"]),
        "one\ntwo\n"
    );
    assert_eq!(
        std::fs::read(repo.path().join("normalised.txt")).unwrap(),
        b"one\r\ntwo\r\n"
    );
}

#[test]
fn set_attributes_writes_the_root_gitattributes() {
    let repo = FixtureRepo::new();
    repo.set_attributes("*.png binary\n*.txt eol=crlf\n");
    assert_eq!(
        std::fs::read_to_string(repo.path().join(".gitattributes")).unwrap(),
        "*.png binary\n*.txt eol=crlf\n"
    );
    assert_eq!(
        repo.git(&["check-attr", "binary", "eol", "--", "a.png", "b.txt"]),
        "a.png: binary: set\na.png: eol: unspecified\nb.txt: binary: unspecified\nb.txt: eol: crlf\n"
    );
}

#[test]
fn branch_and_checkout_switch_lines_of_work() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "base\n");
    let base = repo.commit("base");
    repo.branch("feat");
    assert_eq!(repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "main\n");
    repo.checkout("feat").write("a.txt", "feature\n");
    let tip = repo.commit("feature");
    assert_eq!(repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "feat\n");
    assert_eq!(repo.git(&["rev-parse", "main"]).trim(), base);

    repo.checkout("main");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "base\n"
    );
    repo.checkout(&tip);
    assert_eq!(repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "HEAD\n");
}

#[test]
fn git_command_runs_with_the_fixture_environment() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    let output = repo
        .git_command()
        .args(["hash-object", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child.stdin.take().unwrap().write_all(b"a\n")?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        repo.git(&["rev-parse", "HEAD:a.txt"]).trim()
    );
}

#[test]
fn a_failing_git_call_panics_with_its_stderr() {
    let repo = FixtureRepo::new();
    let message = panic_message(|| {
        repo.git(&["rev-parse", "--verify", "no-such-rev"]);
    });
    assert!(
        message.contains("`git rev-parse --verify no-such-rev` failed"),
        "{message}"
    );
    assert!(message.contains("fatal:"), "{message}");
}

#[test]
fn committing_nothing_panics() {
    let repo = FixtureRepo::new();
    let message = panic_message(|| {
        repo.commit("empty");
    });
    assert!(
        message.contains("`git commit -q -m empty` failed"),
        "{message}"
    );
}

#[test]
fn paths_may_not_leave_the_repo() {
    let repo = FixtureRepo::new();
    for path in ["../escape.txt", "a/../../escape.txt", ""] {
        let message = panic_message(|| {
            repo.write(path, "x");
        });
        assert!(message.contains("stay inside it"), "{path}: {message}");
    }
    let absolute = repo.path().join("abs.txt");
    let message = panic_message(|| {
        repo.write(&absolute, "x");
    });
    assert!(message.contains("stay inside it"), "{message}");
}

#[test]
fn dropping_the_fixture_removes_it() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    let path = repo.path().to_path_buf();
    assert!(path.is_dir());
    drop(repo);
    assert!(!path.exists());
}
