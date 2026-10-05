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

/// Checks a fixture commit's identity, its object id format, that no `git init` template was
/// applied, and that the fixture's `git diff` is git's plain one (default context, git's own
/// diff, glob pathspecs), in whatever environment this test process has. On its own that is the
/// normal one; `inherited_git_variables_do_not_reach_the_fixture` runs it in a child process
/// whose environment exports another identity, a template, SHA-256, diff options, an external
/// diff tool, literal pathspecs and an attribute source (which, naming `HEAD`, fails every git
/// call that reads attributes before the first commit).
#[test]
#[ignore = "run in a hostile git environment by inherited_git_variables_do_not_reach_the_fixture"]
fn fixture_commits_ignore_the_inherited_git_environment() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "1\n2\n3\n4\n5\n6\n7\n");
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

    repo.write("a.txt", "1\n2\n3\nfour\n5\n6\n7\n");
    let diff = repo.git(&["diff", "--no-color"]);
    assert!(
        diff.contains("@@ -1,7 +1,7 @@\n 1\n 2\n 3\n-4\n+four\n"),
        "git's own diff with three lines of context: {diff}"
    );
    assert_eq!(
        repo.git(&["diff", "--name-only", "--", "*.txt"]),
        "a.txt\n",
        "a glob pathspec"
    );
}

#[test]
fn inherited_git_variables_do_not_reach_the_fixture() {
    // `git commit` exports GIT_AUTHOR_* to its hooks, and a shell or CI image can export any of
    // these (difftastic's setup suggests `GIT_EXTERNAL_DIFF=difft`), so a test run from either
    // must still produce the fixture's commits and diffs.
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
        .env("GIT_DIFF_OPTS", "--unified=0")
        .env("GIT_EXTERNAL_DIFF", "false")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_ATTR_SOURCE", "HEAD")
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
fn env_overlay_is_what_the_fixture_git_runs_with() {
    let repo = FixtureRepo::new();
    std::fs::write(
        repo.home().join(".gitconfig"),
        "[dv]\n\tprobe = from-fixture-home\n",
    )
    .unwrap();
    let overlay = repo.env_overlay();
    let value = |name: &str| {
        overlay
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(value("GIT_DIR"), Some(None), "inherited GIT_DIR is removed");
    assert_eq!(
        value("GIT_CONFIG_GLOBAL"),
        Some(Some(repo.home().join(".gitconfig").into_os_string()))
    );
    assert_eq!(value("GIT_CONFIG_NOSYSTEM"), Some(Some("1".into())));
    assert_eq!(value("GIT_ATTR_NOSYSTEM"), Some(Some("1".into())));

    // A plain `git` with only the overlay applied sees the fixture's global config.
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo.path())
        .args(["config", "--get", "dv.probe"]);
    apply_env(&mut command, &overlay);
    let output = command.output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "from-fixture-home\n"
    );
}

#[test]
fn env_overlay_dates_follow_the_commit_count() {
    let repo = FixtureRepo::new();
    let date = |repo: &FixtureRepo| {
        repo.env_overlay()
            .into_iter()
            .find(|(key, _)| key == "GIT_COMMITTER_DATE")
            .and_then(|(_, value)| value)
            .unwrap()
    };
    assert_eq!(date(&repo), format!("{EPOCH} +0000").as_str());
    repo.write("a.txt", "a\n");
    repo.commit("first");
    assert_eq!(date(&repo), format!("{} +0000", EPOCH + TICK_SECS).as_str());
}

#[test]
fn add_worktree_makes_a_linked_worktree_beside_the_repo() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    let base = repo.commit("base");
    let worktree = repo.add_worktree("feature", "feat");
    assert_eq!(worktree.parent().unwrap().file_name().unwrap(), "worktrees");
    assert!(
        worktree.join(".git").is_file(),
        "a linked worktree's .git is a file"
    );
    assert_eq!(repo.git(&["rev-parse", "feat"]).trim(), base);
    let listing = repo.git(&["worktree", "list", "--porcelain"]);
    assert_eq!(listing.matches("worktree ").count(), 2, "{listing}");
    assert!(listing.contains("branch refs/heads/feat"), "{listing}");
}

#[test]
fn bare_clone_has_the_history_and_no_work_tree() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    let base = repo.commit("base");
    let bare = repo.bare_clone("bare.git");
    let output = repo
        .git_command()
        .arg("-C")
        .arg(&bare)
        .args(["rev-parse", "--is-bare-repository", "main"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("true\n{base}\n")
    );
}

#[test]
fn outside_dir_is_not_inside_any_repository() {
    let repo = FixtureRepo::new();
    let dir = repo.outside_dir("plain");
    assert!(dir.is_dir());
    assert!(!dir.starts_with(repo.path()));
    let output = repo
        .git_command()
        .arg("-C")
        .arg(&dir)
        .args(["rev-parse", "--git-dir"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not a git repository"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn names_beside_the_repo_are_plain_and_unique() {
    let repo = FixtureRepo::new();
    repo.outside_dir("taken");
    for name in ["", "../x", "a/b", ".hidden", "repo", "home", "taken"] {
        let message = panic_message(|| {
            repo.outside_dir(name);
        });
        assert!(
            message.contains("plain file names") || message.contains("already exists"),
            "{name}: {message}"
        );
    }
}

#[test]
fn new_sha256_uses_sha256_object_ids() {
    let repo = FixtureRepo::new_sha256();
    assert_eq!(repo.git(&["rev-parse", "--show-object-format"]), "sha256\n");
    repo.write("a.txt", "a\n");
    let oid = repo.commit("first");
    assert_eq!(oid.len(), 64, "{oid}");
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

#[test]
fn hash_blob_writes_the_object_and_returns_its_id() {
    let repo = FixtureRepo::new();
    let oid = repo.hash_blob("hello\n");
    // git's well-known id for the blob "hello\n".
    assert_eq!(oid, "ce013625030ba8dba906f756967f9e9ca394464a");
    assert_eq!(repo.git(&["cat-file", "-p", &oid]), "hello\n");
    let binary = repo.hash_blob([0u8, 1, 2, 255]);
    assert_eq!(repo.git(&["cat-file", "-s", &binary]).trim(), "4");
}

#[test]
fn stage_and_commit_staged_record_index_entries_the_work_tree_lacks() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    let first = repo.commit("first");
    let link = repo.hash_blob("target/path");
    let gitlink = "1111111111111111111111111111111111111111";
    repo.stage("120000", &link, "link")
        .stage("160000", gitlink, "sub")
        .stage("100755", &repo.hash_blob("#!/bin/sh\n"), "dir/run.sh");
    let second = repo.commit_staged("plumbing");

    assert_eq!(repo.git(&["rev-parse", "HEAD~1"]).trim(), first);
    assert_eq!(repo.git(&["rev-parse", "HEAD"]).trim(), second);
    let tree = repo.git(&["ls-tree", "-r", "HEAD"]);
    assert!(
        tree.contains(&format!("120000 blob {link}\tlink\n")),
        "{tree}"
    );
    assert!(
        tree.contains(&format!("160000 commit {gitlink}\tsub\n")),
        "{tree}"
    );
    assert!(tree.contains("100755 blob "), "{tree}");
    assert!(
        !repo.path().join("link").exists(),
        "the work tree is untouched"
    );
    // The commit got the next date in the sequence.
    assert_eq!(
        repo.git(&["log", "-1", "--format=%at"]).trim(),
        (EPOCH + TICK_SECS).to_string()
    );
}

#[test]
fn stage_takes_names_the_os_cannot_create() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("first");
    let name = "q\"uote\tand tab.txt";
    repo.stage("100644", &repo.hash_blob("x\n"), name);
    repo.commit_staged("odd name");
    let listed = repo.git(&["ls-tree", "-z", "--name-only", "HEAD"]);
    assert!(listed.split('\0').any(|entry| entry == name), "{listed:?}");
}

#[test]
fn a_persistent_fixture_is_built_at_its_path_and_kept() {
    // Inside another repository, as a scenario's data dir sits inside dv's own checkout.
    let scratch = tempfile::tempdir().unwrap();
    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(scratch.path())
        .status()
        .unwrap();
    assert!(init.success());
    let path = scratch.path().join("repos").join("api-server");
    let repo = FixtureRepo::persistent(&path);
    repo.write("src/lib.rs", "fn a() {}\n");
    let base = repo.commit("base");
    assert_eq!(repo.path().file_name().unwrap(), "api-server");
    // The fixture's own directories sit beside the work tree, never in it.
    assert!(!repo.home().starts_with(repo.path()), "{repo:?}");
    let outside = repo.outside_dir("plain");
    assert!(!outside.starts_with(repo.path()), "{}", outside.display());
    assert_eq!(repo.git(&["status", "--porcelain", "--ignored"]), "");
    // Discovery stops above the work tree: a directory beside it is in no repository.
    let output = repo
        .git_command()
        .arg("-C")
        .arg(&outside)
        .args(["rev-parse", "--git-dir"])
        .output()
        .unwrap();
    assert!(!output.status.success());

    drop(repo);
    assert!(path.join(".git").is_dir(), "kept after the drop");
    let head = std::fs::read_to_string(path.join(".git").join("refs/heads/main")).unwrap();
    assert_eq!(head.trim(), base);
}

#[test]
fn a_persistent_fixture_reopens_where_it_was_built() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("repos").join("api-server");
    let repo = FixtureRepo::persistent(&path);
    repo.write(
        "a.txt", "a
",
    );
    repo.commit("one");
    let overlay = repo.env_overlay();
    let home = repo.home().to_path_buf();
    drop(repo);

    let again = FixtureRepo::reopen(&path);
    assert_eq!(again.home(), home);
    assert_eq!(
        again.env_overlay(),
        overlay,
        "the same environment, the same next date"
    );
    again.write(
        "a.txt", "b
",
    );
    again.commit("two");
    let dates = again.git(&["log", "--format=%at"]);
    let dates: Vec<i64> = dates.lines().map(|date| date.parse().unwrap()).collect();
    assert_eq!(
        dates,
        [EPOCH + TICK_SECS, EPOCH],
        "the dates continue the sequence"
    );

    let message = panic_message(|| {
        FixtureRepo::reopen(scratch.path().join("repos").join("missing"));
    });
    assert!(message.contains("isn't a persistent fixture"), "{message}");
}

#[test]
fn a_persistent_fixture_needs_a_new_or_empty_directory() {
    let scratch = tempfile::tempdir().unwrap();
    let empty = scratch.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    FixtureRepo::persistent(&empty);
    assert!(empty.join(".git").is_dir());

    let taken = scratch.path().join("taken");
    std::fs::create_dir_all(&taken).unwrap();
    std::fs::write(taken.join("file.txt"), "x\n").unwrap();
    let message = panic_message(|| {
        FixtureRepo::persistent(&taken);
    });
    assert!(message.contains("new or empty directory"), "{message}");
}
