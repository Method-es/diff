//! The repository's local branches and where each one stacks (PLAN §2.2 "Branches"): what the
//! switcher's branch picker offers (DESIGN §4.1), a `branch` target's branch and its base.
//!
//! **Stack bases.** A branch started from another unmerged branch (`main → feat-a → feat-b`)
//! is reviewed against that branch, not the default one: its *stack base* is the nearest
//! branch tip among its first-parent ancestors that isn't in the default branch's history.
//! One `git log --branches --simplify-by-decoration ^<default>` finds them all: it keeps
//! only the commits a branch points at, outside the default branch, and rewrites each one's
//! parents to its nearest such commit, so a branch's first rewritten parent is its stack
//! base's tip; a parent left out (in the default branch's history) means the default branch.
//! A branch merged into the default branch is outside the walk and has no stack base either.

use std::collections::HashMap;

use super::{GitError, GitRepo, Oid, stdout_text};

/// A local branch (`refs/heads/<name>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// Its name without `refs/heads/`.
    pub name: String,
    pub tip: Oid,
    /// Its tip's committer date, in seconds since the epoch.
    pub committed_at: i64,
    /// The unmerged branch it stacks on (module docs); `None` when it stacks on the default
    /// branch, is merged into it, or there is no default branch to tell.
    pub stack_base: Option<String>,
}

impl GitRepo {
    /// Every local branch, the most recently committed first, each with its stack base over
    /// `default_base` (module docs; `GitRepo::default_branch`). Two git commands, whatever
    /// the number of branches.
    pub fn branches(&self, default_base: Option<&str>) -> Result<Vec<Branch>, GitError> {
        let output = self.git(
            &[
                "for-each-ref",
                "--sort=-committerdate",
                "--format=%(refname)%09%(objectname)%09%(committerdate:unix)",
                "refs/heads/",
            ],
            &[0],
        )?;
        let text = stdout_text("for-each-ref", &output)?;
        let mut branches: Vec<Branch> = text
            .lines()
            .filter_map(|line| {
                let mut fields = line.split('\t');
                let name = fields.next()?.strip_prefix("refs/heads/")?;
                let tip = Oid::parse(fields.next()?)?;
                let committed_at = fields.next()?.parse().unwrap_or(0);
                Some(Branch {
                    name: name.to_string(),
                    tip,
                    committed_at,
                    stack_base: None,
                })
            })
            .collect();
        let Some(default_base) = default_base else {
            return Ok(branches);
        };
        let tips = self.unmerged_tips(default_base)?;
        for branch in &mut branches {
            branch.stack_base = tips
                .get(branch.tip.as_str())
                .and_then(|tip| tip.parent.as_deref())
                .and_then(|parent| tips.get(parent))
                .and_then(|parent| parent.names.first().cloned());
        }
        Ok(branches)
    }

    /// The commits local branches point at outside `default_base`'s history, each with its
    /// first parent in the simplified history (module docs) and the branches at it, sorted.
    fn unmerged_tips(&self, default_base: &str) -> Result<HashMap<String, Tip>, GitError> {
        let exclude = format!("^{default_base}");
        let output = self.git(
            &[
                "log",
                "--no-show-signature",
                "--simplify-by-decoration",
                "--decorate-refs=refs/heads/",
                "--format=%H%x09%P%x09%D",
                "--branches",
                "--end-of-options",
                &exclude,
                "--",
            ],
            &[0],
        )?;
        let text = stdout_text("log", &output)?;
        Ok(text
            .lines()
            .filter_map(|line| {
                let mut fields = line.split('\t');
                let commit = fields.next()?.to_string();
                let parent = fields.next()?.split_whitespace().next().map(str::to_string);
                // `HEAD -> feat, feat-copy`: ref names hold no spaces, so `, ` parts them.
                let mut names: Vec<String> = fields
                    .next()
                    .unwrap_or_default()
                    .split(", ")
                    .map(|name| name.strip_prefix("HEAD -> ").unwrap_or(name))
                    .filter(|name| !name.is_empty() && *name != "HEAD")
                    .map(str::to_string)
                    .collect();
                names.sort();
                Some((commit, Tip { parent, names }))
            })
            .collect())
    }
}

/// A commit a branch points at, in the simplified history.
struct Tip {
    parent: Option<String>,
    names: Vec<String>,
}

#[cfg(test)]
mod tests {
    use dv_test_support::FixtureRepo;

    use crate::testing::git_repo;

    fn bases(repo: &FixtureRepo, default: Option<&str>) -> Vec<(String, Option<String>)> {
        let mut found: Vec<(String, Option<String>)> = git_repo(repo)
            .branches(default)
            .expect("branches list")
            .into_iter()
            .map(|branch| (branch.name, branch.stack_base))
            .collect();
        found.sort();
        found
    }

    fn pair(name: &str, base: Option<&str>) -> (String, Option<String>) {
        (name.to_string(), base.map(str::to_string))
    }

    #[test]
    fn a_stack_finds_each_branch_below() {
        let repo = FixtureRepo::new();
        repo.write("f.txt", "m1\n");
        repo.commit("m1");
        repo.branch("merged");
        repo.write("f.txt", "m2\n");
        repo.commit("m2");
        repo.branch("feat-a")
            .checkout("feat-a")
            .write("a.txt", "a\n");
        repo.commit("a1");
        repo.branch("feat-b")
            .checkout("feat-b")
            .write("b.txt", "b\n");
        repo.commit("b1");
        repo.branch("feat-c")
            .checkout("feat-c")
            .write("c.txt", "c\n");
        repo.commit("c1");
        // A branch on the default branch's history, and the default branch moving on.
        repo.checkout("main").branch("side").checkout("side");
        repo.write("s.txt", "s\n");
        repo.commit("s1");
        repo.checkout("main").write("f.txt", "m3\n");
        repo.commit("m3");
        assert_eq!(
            bases(&repo, Some("main")),
            vec![
                pair("feat-a", None),
                pair("feat-b", Some("feat-a")),
                pair("feat-c", Some("feat-b")),
                pair("main", None),
                pair("merged", None),
                pair("side", None),
            ]
        );
    }

    #[test]
    fn no_default_branch_means_no_stack_bases() {
        let repo = FixtureRepo::new();
        repo.write("f.txt", "m1\n");
        repo.commit("m1");
        repo.branch("feat-a")
            .checkout("feat-a")
            .write("a.txt", "a\n");
        repo.commit("a1");
        repo.branch("feat-b")
            .checkout("feat-b")
            .write("b.txt", "b\n");
        repo.commit("b1");
        assert_eq!(
            bases(&repo, None),
            vec![
                pair("feat-a", None),
                pair("feat-b", None),
                pair("main", None)
            ]
        );
    }

    #[test]
    fn a_branch_at_another_ones_tip_stacks_below_both() {
        let repo = FixtureRepo::new();
        repo.write("f.txt", "m1\n");
        repo.commit("m1");
        repo.branch("feat-a")
            .checkout("feat-a")
            .write("a.txt", "a\n");
        repo.commit("a1");
        // `feat-a2` points where `feat-a` does: neither stacks on the other.
        repo.branch("feat-a2");
        repo.branch("feat-b")
            .checkout("feat-b")
            .write("b.txt", "b\n");
        repo.commit("b1");
        assert_eq!(
            bases(&repo, Some("main")),
            vec![
                pair("feat-a", None),
                pair("feat-a2", None),
                pair("feat-b", Some("feat-a")),
                pair("main", None),
            ]
        );
    }

    #[test]
    fn newest_first() {
        let repo = FixtureRepo::new();
        repo.write("f.txt", "m1\n");
        repo.commit("m1");
        repo.branch("older");
        repo.write("f.txt", "m2\n");
        repo.commit("m2");
        repo.branch("newer");
        let names: Vec<String> = git_repo(&repo)
            .branches(Some("main"))
            .expect("branches list")
            .into_iter()
            .map(|branch| branch.name)
            .collect();
        assert_eq!(names.first().map(String::as_str), Some("main"));
        assert_eq!(names.last().map(String::as_str), Some("older"));
    }
}
