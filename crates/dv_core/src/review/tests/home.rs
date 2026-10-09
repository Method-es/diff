//! A review's home worktree (PLAN §2.2, §2.4): where its target resolves, whichever worktree
//! of the repository reads it.

use super::*;
use crate::git::GitRepo;
use crate::location::Location;
use crate::review::home::{Home, HomeError, home, same_worktree};
use crate::testing::open_at;

/// A local repository's location: what `Home::is` and `same_worktree` read is its case rule.
fn local() -> Location {
    Location::Local(std::env::temp_dir())
}

fn branch(name: &str) -> Target {
    Target::Branch {
        branch: name.to_string(),
        base: None,
    }
}

fn git_at(repo: &FixtureRepo, path: &std::path::Path) -> GitRepo {
    GitRepo::new(Arc::new(open_at(repo, path)))
}

#[test]
fn a_working_review_is_homed_in_its_own_worktree_from_every_worktree() {
    let (repo, main) = fixture_store();
    let tree = repo.add_worktree("wt", "feat");
    let (main_git, linked_git) = (git_at(&repo, repo.path()), git_at(&repo, &tree));
    let linked_root = linked_git.info().toplevel.clone();
    for target in [Target::Working, Target::Staged] {
        for git in [&main_git, &linked_git] {
            let found = home(git, &target, &linked_root).unwrap();
            assert!(
                found.is(&local(), &linked_root),
                "{target:?} from {}",
                git.info().toplevel
            );
            assert!(!found.is(&local(), main.worktree()));
        }
        assert!(
            home(&linked_git, &target, main.worktree())
                .unwrap()
                .is(&local(), main.worktree())
        );
    }
}

#[test]
fn a_branch_review_is_homed_where_the_branch_is_checked_out_else_anywhere() {
    let (repo, main) = fixture_store();
    let tree = repo.add_worktree("wt", "feat");
    repo.branch("idle");
    let (main_git, linked_git) = (git_at(&repo, repo.path()), git_at(&repo, &tree));
    let linked_root = linked_git.info().toplevel.clone();
    for git in [&main_git, &linked_git] {
        // However the branch is spelled, from either worktree.
        for name in ["feat", "heads/feat", "refs/heads/feat"] {
            assert!(
                home(git, &branch(name), main.worktree())
                    .unwrap()
                    .is(&local(), &linked_root),
                "{name}"
            );
        }
        assert!(
            home(git, &branch("main"), &linked_root)
                .unwrap()
                .is(&local(), main.worktree())
        );
        // Checked out nowhere: its tip, the same from everywhere.
        assert_eq!(
            home(git, &branch("idle"), main.worktree()).unwrap(),
            Home::Anywhere
        );
        // Gone: nowhere either, and resolving the target says what's wrong.
        assert_eq!(
            home(git, &branch("deleted"), main.worktree()).unwrap(),
            Home::Anywhere
        );
        let commit = Target::Commit {
            rev: "HEAD".to_string(),
        };
        assert_eq!(home(git, &commit, &linked_root).unwrap(), Home::Anywhere);
    }
    // Detached, the main worktree has no branch checked out to be a home for.
    repo.git(&["checkout", "-q", "--detach"]);
    assert_eq!(
        home(&linked_git, &branch("main"), main.worktree()).unwrap(),
        Home::Anywhere
    );
}

#[test]
fn a_branch_checked_out_in_several_worktrees_is_homed_where_it_is_worked_on() {
    let (repo, main) = fixture_store();
    let first = repo.add_worktree("first", "feat");
    // `--force` checks the same branch out a second time.
    let second = repo.outside_dir("second");
    repo.git(&[
        std::ffi::OsStr::new("worktree"),
        std::ffi::OsStr::new("add"),
        std::ffi::OsStr::new("-q"),
        std::ffi::OsStr::new("--force"),
        second.as_os_str(),
        std::ffi::OsStr::new("feat"),
    ]);
    let (main_git, first_git, second_git) = (
        git_at(&repo, repo.path()),
        git_at(&repo, &first),
        git_at(&repo, &second),
    );
    let (first_root, second_root) = (
        first_git.info().toplevel.clone(),
        second_git.info().toplevel.clone(),
    );
    // The worktree asking, whichever the review was made in.
    for (git, root) in [(&first_git, &first_root), (&second_git, &second_root)] {
        for made_in in [&first_root, &second_root, &main.worktree().to_string()] {
            assert!(
                home(git, &branch("feat"), made_in)
                    .unwrap()
                    .is(&local(), root)
            );
        }
    }
    // Asked from elsewhere: the worktree it was made in, else one git lists with it (git
    // promises no order).
    for made_in in [&first_root, &second_root] {
        assert!(
            home(&main_git, &branch("feat"), made_in)
                .unwrap()
                .is(&local(), made_in)
        );
    }
    let found = home(&main_git, &branch("feat"), main.worktree()).unwrap();
    assert!(
        found.is(&local(), &first_root) || found.is(&local(), &second_root),
        "{found:?}"
    );
}

#[test]
fn a_repository_with_its_git_dir_apart_is_homed_in_its_files_never_its_git_dir() {
    let (repo, _main) = fixture_store();
    let files = repo.outside_dir("apart");
    let git_dir = repo.outside_dir("apart.git");
    std::fs::remove_dir_all(&git_dir).unwrap();
    let os = std::ffi::OsStr::new;
    repo.git(&[
        os("init"),
        os("-q"),
        os("-b"),
        os("feat"),
        os("--separate-git-dir"),
        git_dir.as_os_str(),
        files.as_os_str(),
    ]);
    std::fs::write(
        files.join("a.txt"),
        "a
",
    )
    .unwrap();
    for args in [&["add", "a.txt"][..], &["commit", "-q", "-m", "one"]] {
        let mut full = vec![os("-C"), files.as_os_str()];
        full.extend(args.iter().map(|arg| os(arg)));
        repo.git(&full);
    }
    let apart = git_at(&repo, &files);
    let root = apart.info().toplevel.clone();
    // git lists this worktree by its git dir; the home is the files.
    assert!(
        home(&apart, &branch("feat"), &root)
            .unwrap()
            .is(&local(), &root)
    );
    // Detached, the branch is checked out nowhere: the git dir is never taken for a worktree.
    let mut detach = vec![os("-C"), files.as_os_str()];
    detach.extend([os("checkout"), os("-q"), os("--detach")]);
    repo.git(&detach);
    assert_eq!(
        home(&apart, &branch("feat"), &root).unwrap(),
        Home::Anywhere
    );
}

#[test]
fn a_review_whose_worktree_is_gone_has_no_home() {
    let (repo, main) = fixture_store();
    let removed = repo.add_worktree("removed", "removed");
    let deleted = repo.add_worktree("deleted", "deleted");
    let roots: Vec<String> = [&removed, &deleted]
        .map(|tree| git_at(&repo, tree).info().toplevel.clone())
        .to_vec();
    let main_git = git_at(&repo, repo.path());
    repo.git(&[
        std::ffi::OsStr::new("worktree"),
        std::ffi::OsStr::new("remove"),
        removed.as_os_str(),
    ]);
    // Deleted without `git worktree remove`: git still lists it, as prunable.
    std::fs::remove_dir_all(&deleted).unwrap();
    for root in &roots {
        for target in [Target::Working, Target::Staged] {
            match home(&main_git, &target, root) {
                Err(HomeError::Gone { worktree }) => assert_eq!(&worktree, root),
                other => panic!("expected Gone for {root}, got {other:?}"),
            }
        }
    }
    // The branch a deleted worktree had checked out is checked out nowhere alive.
    assert_eq!(
        home(&main_git, &branch("deleted"), main.worktree()).unwrap(),
        Home::Anywhere
    );
    // A folder that was never one of the repository's worktrees.
    let elsewhere = repo.outside_dir("elsewhere");
    assert!(matches!(
        home(
            &main_git,
            &Target::Working,
            &elsewhere.display().to_string()
        ),
        Err(HomeError::Gone { .. })
    ));
}

#[test]
fn worktree_roots_compare_however_they_are_spelled() {
    let here = local();
    assert!(same_worktree(&here, "D:/code/api", "D:/code/api/"));
    assert!(same_worktree(&here, r"D:\code\api", "D:/code/api"));
    assert!(!same_worktree(&here, "/code/api", "/code/api2"));
    assert_eq!(
        same_worktree(&here, "/code/API", "/code/api"),
        cfg!(any(windows, target_os = "macos"))
    );
    // A WSL repository's roots are the distro's, on a Linux file system: case counts there,
    // whatever this machine's file systems do.
    let wsl = Location::Wsl {
        distro: "Ubuntu".into(),
        path: "/home/kyle/api".into(),
    };
    assert!(!same_worktree(&wsl, "/home/kyle/API", "/home/kyle/api"));
    assert!(same_worktree(&wsl, "/home/kyle/api/", "/home/kyle/api"));
    assert!(Home::Worktree("/home/kyle/api".into()).is(&wsl, "/home/kyle/api"));
    assert!(!Home::Worktree("/home/kyle/Api".into()).is(&wsl, "/home/kyle/api"));
}
