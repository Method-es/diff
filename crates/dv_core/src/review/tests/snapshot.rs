//! Round snapshots (PLAN §2.3) on fixture repos: what they hold, and that they leave the user's
//! repo exactly as it was (PLAN §2.4).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

use crate::backend::{Backend, LocalBackend, LocalBackendOptions};
use crate::diff::{FileEntry, FileStatus, file_list};
use crate::git::GitRepo;
use crate::review::snapshot::{self, SnapshotError};
use crate::target::{DiffSpec, Side};
use crate::testing::{git_repo, options};

use super::*;

/// Everything a snapshot must not change: the index file's bytes and time, `HEAD`, every
/// worktree file, and the refs outside `refs/dv/`.
#[derive(Debug, PartialEq, Eq)]
struct Untouched {
    index: Option<(Vec<u8>, SystemTime)>,
    head: String,
    worktree: BTreeMap<String, Vec<u8>>,
    refs: String,
}

impl Untouched {
    fn of(repo: &FixtureRepo) -> Self {
        let index_path = repo.path().join(".git").join("index");
        let index = std::fs::read(&index_path).ok().map(|bytes| {
            let modified = std::fs::metadata(&index_path).unwrap().modified().unwrap();
            (bytes, modified)
        });
        let head = std::fs::read_to_string(repo.path().join(".git").join("HEAD")).unwrap();
        let mut worktree = BTreeMap::new();
        read_tree(repo.path(), "", &mut worktree);
        let refs = repo
            .git(&["for-each-ref", "--format=%(refname) %(objectname)"])
            .lines()
            .filter(|line| !line.starts_with("refs/dv/"))
            .collect::<Vec<_>>()
            .join("\n");
        Self {
            index,
            head,
            worktree,
            refs,
        }
    }
}

/// Every file below `dir` but the git dir, by `/`-separated path.
fn read_tree(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if prefix.is_empty() && name == ".git" {
            continue;
        }
        let rel = format!("{prefix}{name}");
        if entry.file_type().unwrap().is_dir() {
            read_tree(&entry.path(), &format!("{rel}/"), out);
        } else {
            out.insert(rel, std::fs::read(entry.path()).unwrap());
        }
    }
}

/// The store's temp directory listing: empty once a snapshot returns.
fn temp_files(repo: &FixtureRepo) -> Vec<String> {
    let dir = repo.path().join(".git").join("dv").join("tmp");
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// What a file list says about each file, with an untracked file counted as the addition a
/// snapshot makes of it.
fn listed(entries: &[FileEntry]) -> Vec<(String, Option<String>, FileStatus, u32, u32, bool)> {
    entries
        .iter()
        .map(|entry| {
            let status = match entry.status {
                FileStatus::Untracked => FileStatus::Added,
                status => status,
            };
            (
                entry.path.clone(),
                entry.old_path.clone(),
                status,
                entry.additions,
                entry.deletions,
                entry.binary,
            )
        })
        .collect()
}

fn resolve(git: &GitRepo, target: &Target) -> DiffSpec {
    target.resolve(git).unwrap()
}

fn rev(repo: &FixtureRepo, rev: &str) -> String {
    repo.git(&["rev-parse", rev]).trim().to_string()
}

#[test]
fn a_working_snapshot_holds_what_the_file_list_shows_and_changes_nothing() {
    let repo = FixtureRepo::new();
    repo.config("core.autocrlf", "true");
    repo.write(".gitignore", "*.log\n")
        .write("src/lib.rs", "fn a() {}\nfn b() {}\n")
        .write("src/gone.rs", "fn gone() {}\n")
        .write("same.txt", "unchanged\n")
        .write_crlf("windows.txt", "one\ntwo\nthree\n");
    let head = repo.commit("base");
    repo.write("src/lib.rs", "fn a() {}\nfn b2() {}\nfn c() {}\n")
        .write("src/new.rs", "struct New;\n")
        .write("debug.log", "ignored\n")
        .remove("src/gone.rs")
        // Rewritten with the same bytes: stat-dirty, unchanged.
        .write("same.txt", "unchanged\n")
        .write_crlf("windows.txt", "one\nTWO\nthree\n");
    // A staged change and an unstaged one on top of it, in the same file.
    repo.write("staged.txt", "staged\n");
    repo.git(&["add", "staged.txt"]);
    repo.write("staged.txt", "staged\nthen edited\n");
    // A nested repository with a commit: a gitlink in the snapshot, one addition either way.
    repo.write("nested/n.txt", "n\n");
    repo.git(&["-C", "nested", "init", "-q"]);
    repo.git(&["-C", "nested", "add", "-A"]);
    repo.git(&["-C", "nested", "commit", "-q", "-m", "nested"]);

    let git = git_repo(&repo);
    let spec = resolve(&git, &Target::Working);
    let working = file_list(&git, &spec).unwrap();
    let before = Untouched::of(&repo);
    let id = ReviewId::generate();
    let snap = snapshot::take(&git, &id, 1, &spec, None).unwrap();

    assert_eq!(
        Untouched::of(&repo),
        before,
        "the snapshot changed the repo"
    );
    assert!(temp_files(&repo).is_empty(), "{:?}", temp_files(&repo));
    assert_eq!(rev(&repo, &snapshot::ref_name(&id, 1)), snap.as_str());
    // A commit on HEAD with dv's message (the identity: `no_git_identity_is_needed`; here the
    // fixture's GIT_AUTHOR_* variables win, as they do over any configuration).
    assert_eq!(rev(&repo, &format!("{snap}^")), head);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s", snap.as_str()])
            .trim(),
        format!("dv review {id} round 1")
    );
    // The tree is the working tree as git sees it.
    let tree = repo.git(&["ls-tree", "-r", "--name-only", snap.as_str()]);
    assert_eq!(
        tree.lines().collect::<Vec<_>>(),
        [
            ".gitignore",
            "nested",
            "same.txt",
            "src/lib.rs",
            "src/new.rs",
            "staged.txt",
            "windows.txt"
        ]
    );
    assert_eq!(
        repo.git(&["show", &format!("{snap}:windows.txt")]),
        "one\nTWO\nthree\n",
        "stored with LF, as git stores it under core.autocrlf"
    );
    assert_eq!(
        repo.git(&["show", &format!("{snap}:staged.txt")]),
        "staged\nthen edited\n"
    );
    // HEAD against the snapshot lists what the working file list showed.
    let snapped = file_list(
        &git,
        &DiffSpec {
            old: spec.old.clone(),
            new: Side::Commit(snap),
        },
    )
    .unwrap();
    assert_eq!(listed(&snapped), listed(&working));
    assert!(
        listed(&working)
            .iter()
            .any(|(path, _, status, add, del, _)| path == "windows.txt"
                && *status == FileStatus::Modified
                && (*add, *del) == (1, 1)),
        "{working:#?}"
    );
}

#[test]
fn a_staged_snapshot_is_the_index_tree() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n").write("b.txt", "b\n");
    let head = repo.commit("base");
    repo.write("a.txt", "a staged\n").write("c.txt", "c\n");
    repo.git(&["add", "a.txt", "c.txt"]);
    repo.git(&["rm", "-q", "b.txt"]);
    // Not staged: not in the snapshot.
    repo.write("a.txt", "a edited later\n")
        .write("untracked.txt", "u\n");

    let git = git_repo(&repo);
    let spec = resolve(&git, &Target::Staged);
    let before = Untouched::of(&repo);
    let snap = snapshot::take(&git, &ReviewId::generate(), 1, &spec, None).unwrap();

    assert_eq!(Untouched::of(&repo), before);
    assert!(temp_files(&repo).is_empty());
    assert_eq!(rev(&repo, &format!("{snap}^")), head);
    // The tree's entries are the index's, entry for entry.
    let index: Vec<String> = repo
        .git(&["ls-files", "-s"])
        .lines()
        .map(|line| {
            let (meta, path) = line.split_once('\t').unwrap();
            let mut fields = meta.split(' ');
            let (mode, oid) = (fields.next().unwrap(), fields.next().unwrap());
            format!("{mode} {oid} {path}")
        })
        .collect();
    let tree: Vec<String> = repo
        .git(&["ls-tree", "-r", snap.as_str()])
        .lines()
        .map(|line| {
            let (meta, path) = line.split_once('\t').unwrap();
            let fields: Vec<&str> = meta.split(' ').collect();
            format!("{} {} {path}", fields[0], fields[2])
        })
        .collect();
    assert_eq!(tree, index);
    assert_eq!(repo.git(&["show", &format!("{snap}:a.txt")]), "a staged\n");
}

#[test]
fn a_commit_side_is_kept_as_it_is() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    repo.branch("feat")
        .checkout("feat")
        .write("a.txt", "feat\n");
    let tip = repo.commit("feat work");
    repo.checkout("main");
    let git = git_repo(&repo);
    for target in [
        Target::Commit {
            rev: "feat".to_string(),
        },
        // Not checked out here, so its new side is the branch tip.
        Target::Branch {
            branch: "feat".to_string(),
            base: Some("main".to_string()),
        },
    ] {
        let id = ReviewId::generate();
        let spec = resolve(&git, &target);
        assert_eq!(
            snapshot::take(&git, &id, 1, &spec, None).unwrap().as_str(),
            tip
        );
        assert_eq!(rev(&repo, &snapshot::ref_name(&id, 1)), tip);
    }
    assert!(temp_files(&repo).is_empty());
}

#[test]
fn an_unborn_head_snapshots_without_a_parent() {
    // Nothing ever staged: there is no index file, and the snapshot starts from an empty one.
    let repo = FixtureRepo::new();
    repo.write("first.txt", "1\n");
    assert!(!repo.path().join(".git").join("index").exists());
    let git = git_repo(&repo);
    let spec = resolve(&git, &Target::Working);
    assert_eq!(spec.old, Side::EmptyTree);
    let before = Untouched::of(&repo);
    let snap = snapshot::take(&git, &ReviewId::generate(), 1, &spec, None).unwrap();
    assert_eq!(Untouched::of(&repo), before);
    assert!(!repo.path().join(".git").join("index").exists());
    assert_eq!(repo.git(&["log", "--format=%P", snap.as_str()]), "\n");
    assert_eq!(
        repo.git(&["ls-tree", "-r", "--name-only", snap.as_str()]),
        "first.txt\n"
    );

    // Something staged, nothing committed: the index exists.
    repo.write("second.txt", "2\n");
    repo.git(&["add", "second.txt"]);
    let spec = resolve(&git, &Target::Staged);
    let before = Untouched::of(&repo);
    let snap = snapshot::take(&git, &ReviewId::generate(), 1, &spec, None).unwrap();
    assert_eq!(Untouched::of(&repo), before);
    assert_eq!(
        repo.git(&["ls-tree", "-r", "--name-only", snap.as_str()]),
        "second.txt\n"
    );
    assert!(temp_files(&repo).is_empty());
}

#[test]
fn no_git_identity_is_needed() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    repo.git(&["config", "--unset", "user.name"]);
    repo.git(&["config", "--unset", "user.email"]);
    // Never guess one from the machine either.
    repo.config("user.useConfigOnly", "true");
    repo.write("a.txt", "changed\n");
    let mut env: Vec<_> = options(&repo)
        .env
        .into_iter()
        .filter(|(name, _)| {
            !name.to_string_lossy().starts_with("GIT_AUTHOR_")
                && !name.to_string_lossy().starts_with("GIT_COMMITTER_")
        })
        .collect();
    for name in [
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
        "EMAIL",
    ] {
        env.push((name.into(), None));
    }
    let backend = LocalBackend::open_with(repo.path(), LocalBackendOptions { env }).unwrap();
    let tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
    // Without dv's own identity git refuses to make a commit here.
    assert!(
        backend
            .git(
                &["commit-tree", "-m", "x", tree.trim()],
                &crate::backend::GitOpts::new()
            )
            .is_err()
    );
    let git = GitRepo::new(std::sync::Arc::new(backend));
    let spec = resolve(&git, &Target::Working);
    let snap = snapshot::take(&git, &ReviewId::generate(), 1, &spec, None).unwrap();
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an <%ae>", snap.as_str()])
            .trim(),
        "dv <dv@dv.invalid>"
    );
}

#[test]
fn an_existing_round_ref_is_reused_only_for_the_same_content() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    let head = repo.commit("base");
    repo.write("a.txt", "changed\n");
    let git = git_repo(&repo);
    let spec = resolve(&git, &Target::Working);
    let id = ReviewId::generate();
    let name = snapshot::ref_name(&id, 1);

    // The ref of a round closed at another commit is never overwritten.
    repo.git(&["update-ref", &name, &head]);
    let closed: Oid = head.parse().unwrap();
    match snapshot::take(&git, &id, 1, &spec, Some(&closed)) {
        Err(SnapshotError::RefExists {
            name: got,
            existing,
        }) => {
            assert_eq!(got, name);
            assert_eq!(existing.as_str(), head);
        }
        other => panic!("expected RefExists, got {other:?}"),
    }
    assert_eq!(rev(&repo, &name), head);
    assert!(temp_files(&repo).is_empty());

    // A commit of the same tree on the same parent, made by an earlier attempt at another
    // time, is reused.
    repo.git(&["update-ref", "-d", &name]);
    let first = snapshot::take(&git, &id, 1, &spec, None).unwrap();
    repo.git(&["update-ref", "-d", &name]);
    let earlier = repo
        .git(&[
            "commit-tree",
            "-m",
            "an earlier attempt",
            "-p",
            &head,
            &format!("{first}^{{tree}}"),
        ])
        .trim()
        .to_string();
    repo.git(&["update-ref", &name, &earlier]);
    assert_eq!(
        snapshot::take(&git, &id, 1, &spec, None).unwrap().as_str(),
        earlier
    );
    // The same commit again is fine too.
    assert_eq!(
        snapshot::take(
            &git,
            &id,
            1,
            &DiffSpec {
                old: spec.old.clone(),
                new: Side::Commit(earlier.parse().unwrap()),
            },
            None
        )
        .unwrap()
        .as_str(),
        earlier
    );
}

#[test]
fn a_leftover_ref_of_a_round_in_progress_is_replaced() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    let head = repo.commit("base");
    repo.write("a.txt", "first\n");
    let git = git_repo(&repo);
    let id = ReviewId::generate();
    let name = snapshot::ref_name(&id, 1);
    // An attempt whose round never closed, then the content changed.
    let leftover = snapshot::take(&git, &id, 1, &resolve(&git, &Target::Working), None).unwrap();
    repo.write("a.txt", "second\n");
    let spec = resolve(&git, &Target::Working);
    // Had the round closed at it, it would stay.
    assert!(matches!(
        snapshot::take(&git, &id, 1, &spec, Some(&leftover)),
        Err(SnapshotError::RefExists { .. })
    ));
    assert_eq!(rev(&repo, &name), leftover.as_str());
    // The round is in progress: the new snapshot replaces it.
    let snap = snapshot::take(&git, &id, 1, &spec, None).unwrap();
    assert_ne!(snap, leftover);
    assert_eq!(rev(&repo, &name), snap.as_str());
    assert_eq!(repo.git(&["show", &format!("{snap}:a.txt")]), "second\n");
    assert_eq!(
        repo.git(&["log", "--format=%P", "-1", snap.as_str()])
            .trim(),
        head
    );
    // So does a commit side (a branch whose tip moved).
    let tip = repo.commit("tip");
    let commits = DiffSpec {
        old: spec.old.clone(),
        new: Side::Commit(tip.parse().unwrap()),
    };
    assert_eq!(
        snapshot::take(&git, &id, 1, &commits, None)
            .unwrap()
            .as_str(),
        tip
    );
    assert_eq!(rev(&repo, &name), tip);
}

#[test]
fn a_leftover_ref_at_an_earlier_rounds_commit_is_replaced() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    let base = repo.commit("base");
    repo.write("a.txt", "b\n");
    let first = repo.commit("first");
    let git = git_repo(&repo);
    let id = ReviewId::generate();
    let commits = |tip: &str| DiffSpec {
        old: Side::Commit(base.parse().unwrap()),
        new: Side::Commit(tip.parse().unwrap()),
    };
    // Round 1 closed at the tip; round 2 was finished before it moved, and its events never
    // landed, so round 2's ref holds round 1's commit.
    let round_one = snapshot::take(&git, &id, 1, &commits(&first), None).unwrap();
    assert_eq!(
        snapshot::take(&git, &id, 2, &commits(&first), None).unwrap(),
        round_one
    );
    // The tip moves on: round 2 closes at it, and round 1's own ref keeps its commit.
    repo.write("a.txt", "c\n");
    let second = repo.commit("second");
    assert_eq!(
        snapshot::take(&git, &id, 2, &commits(&second), None)
            .unwrap()
            .as_str(),
        second
    );
    assert_eq!(rev(&repo, &snapshot::ref_name(&id, 2)), second);
    assert_eq!(rev(&repo, &snapshot::ref_name(&id, 1)), first);
}

#[test]
fn left_out_paths_tells_nested_repositories_from_unreadable_files() {
    let stderr = "error: open(\"locked.txt\"): Permission denied\n\
                  error: unable to index file 'locked.txt'\n\
                  error: 'nested/' does not have a commit checked out\n\
                  error: unable to index file 'nested/'\n\
                  error: unable to index file 'dir/it's.txt'\n";
    let left_out = snapshot::left_out_paths(stderr);
    assert_eq!(left_out.nested, ["nested/"]);
    assert_eq!(left_out.unreadable, ["locked.txt", "dir/it's.txt"]);
    assert_eq!(
        snapshot::left_out_paths(
            "error: 'a/' does not have a commit checked out\nerror: unable to index file 'a/'\n"
        ),
        snapshot::LeftOut {
            nested: vec!["a/".into()],
            unreadable: Vec::new(),
        }
    );
}

/// Keeps `path` unreadable to other processes until dropped: held open with a Windows share
/// mode of none, or (Unix) without read permission.
struct Unreadable {
    _file: Option<std::fs::File>,
    restore: Option<(std::path::PathBuf, std::fs::Permissions)>,
}

impl Unreadable {
    /// `None` where that doesn't stop a read (Unix as root).
    fn lock(path: &Path) -> Option<Self> {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(path)
                .unwrap();
            Some(Self {
                _file: Some(file),
                restore: None,
            })
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let before = std::fs::metadata(path).unwrap().permissions();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
            let locked = Self {
                _file: None,
                restore: Some((path.to_path_buf(), before)),
            };
            std::fs::read(path).is_err().then_some(locked)
        }
    }
}

impl Drop for Unreadable {
    fn drop(&mut self) {
        if let Some((path, permissions)) = self.restore.take() {
            let _ = std::fs::set_permissions(path, permissions);
        }
    }
}

#[test]
fn a_file_git_cant_read_fails_the_round_rather_than_keeping_an_old_version() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n").write("locked.txt", "v1\n");
    repo.commit("base");
    repo.write("a.txt", "changed\n")
        .write("locked.txt", "v2 reviewed\n");
    // A nested repository without a commit is still left out quietly.
    repo.git(&["init", "-q", "nested"]);
    let git = git_repo(&repo);
    let spec = resolve(&git, &Target::Working);
    let id = ReviewId::generate();
    let before = Untouched::of(&repo);
    let Some(lock) = Unreadable::lock(&repo.path().join("locked.txt")) else {
        eprintln!("skipped: this user reads files without read permission");
        return;
    };
    let error = snapshot::take(&git, &id, 1, &spec, None).unwrap_err();
    assert_eq!(
        error.to_string(),
        "git couldn't read locked.txt to snapshot the round"
    );
    match error {
        SnapshotError::Unreadable { paths, detail } => {
            assert_eq!(paths, ["locked.txt"]);
            assert!(
                detail.contains("unable to index file 'locked.txt'"),
                "{detail}"
            );
        }
        other => panic!("expected Unreadable, got {other:?}"),
    }
    drop(lock);
    assert!(temp_files(&repo).is_empty(), "{:?}", temp_files(&repo));
    assert_eq!(
        repo.git(&["for-each-ref", "refs/dv/"]),
        "",
        "no round ref without a snapshot"
    );
    assert_eq!(Untouched::of(&repo), before);
    // Readable again, the round snapshots what the reviewer was shown.
    let snap = snapshot::take(&git, &id, 1, &spec, None).unwrap();
    assert_eq!(
        repo.git(&["show", &format!("{snap}:locked.txt")]),
        "v2 reviewed\n"
    );
}

#[test]
fn temp_indexes_a_killed_snapshot_left_are_swept_after_an_hour() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    repo.write("a.txt", "changed\n");
    let backend = crate::testing::open(&repo);
    let now = SystemTime::now();
    let name_at = |ago: std::time::Duration| {
        let ulid = ulid::Ulid::from_datetime(now - ago);
        format!("index-{}", ulid.to_string().to_ascii_lowercase())
    };
    let stale = name_at(snapshot::STALE_TEMP + std::time::Duration::from_secs(60));
    let fresh = name_at(std::time::Duration::from_secs(60));
    for name in [
        stale.clone(),
        format!("{stale}.lock"),
        fresh.clone(),
        "notes".to_string(),
    ] {
        backend
            .store_write_atomic(&format!("tmp/{name}"), b"index bytes")
            .unwrap();
    }
    let git = git_repo(&repo);
    let spec = resolve(&git, &Target::Working);
    snapshot::take(&git, &ReviewId::generate(), 1, &spec, None).unwrap();
    let mut left = temp_files(&repo);
    left.sort();
    assert_eq!(
        left,
        [fresh, "notes".to_string()],
        "only the stale index and its lock go"
    );
}

#[test]
fn unresolved_conflicts_refuse_an_index_snapshot_and_stage_as_files_stand() {
    let repo = FixtureRepo::new();
    repo.write("f.txt", "base\n");
    repo.commit("base");
    repo.branch("other").write("f.txt", "main\n");
    repo.commit("main side");
    repo.checkout("other").write("f.txt", "other\n");
    repo.commit("other side");
    repo.checkout("main");
    let merge = repo
        .git_command()
        .args(["merge", "-q", "other"])
        .output()
        .unwrap();
    assert!(!merge.status.success(), "the merge conflicts");
    assert!(repo.git(&["ls-files", "-u"]).contains("f.txt"));

    let git = git_repo(&repo);
    let before = Untouched::of(&repo);
    let staged = resolve(&git, &Target::Staged);
    match snapshot::take(&git, &ReviewId::generate(), 1, &staged, None) {
        Err(SnapshotError::Unmerged { detail }) => assert!(detail.contains("f.txt"), "{detail}"),
        other => panic!("expected Unmerged, got {other:?}"),
    }
    assert!(temp_files(&repo).is_empty(), "{:?}", temp_files(&repo));
    assert_eq!(Untouched::of(&repo), before);

    // The working tree has one version of the file: the one with the conflict markers.
    let working = resolve(&git, &Target::Working);
    let snap = snapshot::take(&git, &ReviewId::generate(), 1, &working, None).unwrap();
    assert_eq!(
        repo.git(&["show", &format!("{snap}:f.txt")]),
        String::from_utf8(before.worktree["f.txt"].clone()).unwrap()
    );
    assert!(temp_files(&repo).is_empty());
    assert_eq!(Untouched::of(&repo), before);
    assert!(
        repo.git(&["ls-files", "-u"]).contains("f.txt"),
        "still unmerged"
    );
}

#[test]
fn a_split_index_writes_nothing_into_the_git_dir() {
    let repo = FixtureRepo::new();
    repo.config("core.splitIndex", "true");
    repo.write("a.txt", "a\n");
    repo.commit("base");
    repo.write("a.txt", "changed\n").write("b.txt", "new\n");
    let git_dir_files = || -> Vec<String> {
        std::fs::read_dir(repo.path().join(".git"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            // The store, which the snapshot creates.
            .filter(|name| name != "dv")
            .collect()
    };
    assert!(
        git_dir_files()
            .iter()
            .any(|name| name.starts_with("sharedindex.")),
        "{:?}",
        git_dir_files()
    );
    let git = git_repo(&repo);
    let before = (git_dir_files(), Untouched::of(&repo));
    for target in [Target::Working, Target::Staged] {
        let spec = resolve(&git, &target);
        snapshot::take(&git, &ReviewId::generate(), 1, &spec, None).unwrap();
    }
    assert_eq!((git_dir_files(), Untouched::of(&repo)), before);
}

#[test]
fn an_unborn_nested_repository_is_left_out_rather_than_failing() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    repo.write("a.txt", "changed\n");
    // A nested repository without a commit, which git can't stage.
    repo.git(&["init", "-q", "nested"]);
    let git = git_repo(&repo);
    let spec = resolve(&git, &Target::Working);
    let snap = snapshot::take(&git, &ReviewId::generate(), 1, &spec, None).unwrap();
    assert_eq!(
        repo.git(&["ls-tree", "-r", "--name-only", snap.as_str()]),
        "a.txt\n"
    );
    assert_eq!(repo.git(&["show", &format!("{snap}:a.txt")]), "changed\n");
}

#[test]
fn the_index_file_option_resolves_inside_the_store_only() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    let backend = crate::testing::open(&repo);
    assert!(backend.copy_index_to_store("tmp/index-test").unwrap());
    let copy = repo
        .path()
        .join(".git")
        .join("dv")
        .join("tmp")
        .join("index-test");
    let real = repo.path().join(".git").join("index");
    assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&real).unwrap());
    assert_eq!(
        std::fs::metadata(&copy).unwrap().modified().unwrap(),
        std::fs::metadata(&real).unwrap().modified().unwrap(),
        "the copy keeps the index's time"
    );
    let opts = crate::backend::GitOpts::new().with_index_file("tmp/index-test");
    let listed = backend.git(&["ls-files"], &opts).unwrap();
    assert_eq!(listed.stdout, b"a.txt\n");
    for bad in ["../index", "/abs", "tmp/.hidden", "tmp\\x"] {
        let opts = crate::backend::GitOpts::new().with_index_file(bad);
        assert!(
            matches!(
                backend.git(&["ls-files"], &opts),
                Err(crate::BackendError::InvalidPath { .. })
            ),
            "{bad}"
        );
    }
    // A missing index file is an empty index.
    let opts = crate::backend::GitOpts::new().with_index_file("tmp/missing");
    assert_eq!(backend.git(&["ls-files"], &opts).unwrap().stdout, b"");
    // Only a backend can resolve the option; the bare runner refuses it.
    assert!(matches!(
        crate::git::command::run(repo.path(), &[], &["version"], &opts),
        Err(crate::BackendError::InvalidPath { .. })
    ));
    // No index yet: false, and the directory exists for git to start one in.
    let unborn = FixtureRepo::new();
    let backend = crate::testing::open(&unborn);
    assert!(!backend.copy_index_to_store("tmp/index-none").unwrap());
    assert!(unborn.path().join(".git").join("dv").join("tmp").is_dir());
}
