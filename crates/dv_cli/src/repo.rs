//! The repository a command works on (docs/PLAN.md §10.1, §10.4): `--wsl <distro>:<path>`,
//! else `--repo <path>`, else the working directory's. A WSL location can't be served from
//! Windows yet (PLAN D6: only through `dv backend --stdio` in the distro), so it is
//! exit 4 at once, without starting `wsl.exe`.

use std::path::Path;
use std::sync::Arc;

use dv_core::backend::{Backend, LocalBackend, LocalBackendOptions};
use dv_core::git::GitRepo;
use dv_core::label::Labels;
use dv_core::location::Location;
use dv_core::review::{Home, HomeError, Review, ReviewStore, home, same_worktree as home_is};

use crate::args::GlobalArgs;
use crate::context::Context;
use crate::error::{CliError, Exit, code};

/// An open repository: its backend, git queries over it and its review store.
#[derive(Clone)]
pub(crate) struct Repo {
    pub backend: Arc<dyn Backend>,
    pub git: GitRepo,
    pub store: ReviewStore,
}

impl Repo {
    /// The repository the command means (module docs), opened with the context's git
    /// environment.
    pub fn open(global: &GlobalArgs, ctx: &Context) -> Result<Self, CliError> {
        let location = location(global, &ctx.cwd)?;
        let path = match location {
            Location::Local(path) => path,
            Location::Wsl { distro, path } => return Err(wsl_unavailable(&distro, &path)),
        };
        Self::open_at(Path::new(&path), ctx)
    }

    /// The work tree at (or containing) `path`, with the context's git environment.
    fn open_at(path: &Path, ctx: &Context) -> Result<Self, CliError> {
        let backend: Arc<dyn Backend> = Arc::new(LocalBackend::open_with(
            path,
            LocalBackendOptions {
                env: ctx.git_env.clone(),
            },
        )?);
        Ok(Self {
            git: GitRepo::new(backend.clone()),
            store: ReviewStore::new(backend.clone()),
            backend,
        })
    }

    /// Where `review`'s target resolves: its home worktree (`dv_core::review::home`, PLAN §2.2,
    /// §2.4) — for a `working` or `staged` review the worktree it was made in, for a `branch`
    /// review the worktree that has the branch checked out — which may be another worktree of
    /// this repository, the same one the GUI resolves it in; any other target resolves the same
    /// from every worktree, so here. A home worktree that is gone (removed, moved, or deleted)
    /// is exit 1 (`target`): what the review is of can't be read anywhere else.
    pub fn for_review(&self, review: &Review, ctx: &Context) -> Result<Self, CliError> {
        let home = match home(&self.git, &review.target, &review.worktree) {
            Ok(home) => home,
            Err(HomeError::Git(error)) => return Err(error.into()),
            Err(gone @ HomeError::Gone { .. }) => {
                return Err(CliError::new(
                    Exit::Failure,
                    code::TARGET,
                    format!("review {} can't be read: {gone}", review.id),
                ));
            }
        };
        let root = match home {
            Home::Worktree(root) if !home_is(&root, self.root()) => root,
            Home::Worktree(_) | Home::Anywhere => return Ok(self.clone()),
        };
        let there = Self::open_at(Path::new(&root), ctx).map_err(|error| CliError {
            message: format!(
                "review {} is read in the worktree {root}, which doesn't open: {}",
                review.id, error.message
            ),
            ..error
        })?;
        if !same_path_text(
            &there.backend.repo().common_dir,
            &self.backend.repo().common_dir,
        ) {
            return Err(CliError::new(
                Exit::Failure,
                code::TARGET,
                format!(
                    "review {} is read in the worktree {root}, which is another repository's now",
                    review.id
                ),
            ));
        }
        Ok(there)
    }

    /// The worktree root.
    pub fn root(&self) -> &str {
        &self.backend.repo().toplevel
    }

    /// `path`, a file of `review`, as a path from a worktree root, `/`-separated, as reviews
    /// record paths: an absolute path inside the worktree the review was made in (where a
    /// `working` or `staged` review is read, [`Repo::for_review`]) or the one the command runs
    /// in loses that root, the longer one when both hold it, `\` becomes `/`, and a leading
    /// `./` goes; any other path is taken as already relative to the root.
    pub fn relative_path(&self, path: &str, review: &Review) -> String {
        let path = path.trim().replace('\\', "/");
        let roots = [review.worktree.as_str(), self.root()]
            .map(|root| format!("{}/", root.replace('\\', "/").trim_end_matches('/')));
        let inside = roots
            .iter()
            .filter(|root| {
                path.get(..root.len())
                    .is_some_and(|head| same_path_text(head, root))
            })
            .max_by_key(|root| root.len())
            .map(|root| &path[root.len()..]);
        let mut rel = inside.unwrap_or(&path);
        while let Some(rest) = rel.strip_prefix("./") {
            rel = rest;
        }
        rel.to_string()
    }

    /// The names targets are labelled with, every worktree's `HEAD` included (blocking: a
    /// few git calls).
    pub fn labels(&self) -> Labels {
        Labels::read(&self.git)
    }
}

/// Where the repository is, from the flags or the working directory `cwd`. A relative
/// `--repo` resolves against `cwd`, and the path that makes is read as a location again, so a
/// relative path from a working directory inside WSL (`\\wsl.localhost\…`, where PowerShell
/// can stand) is a WSL location too. Nothing is opened: a WSL location is returned as it is.
pub fn location(global: &GlobalArgs, cwd: &Path) -> Result<Location, CliError> {
    if let Some(wsl) = &global.wsl {
        return Location::parse_wsl(wsl)
            .map_err(|error| CliError::usage(format!("--wsl: {error}")));
    }
    let (text, from_flag) = match &global.repo {
        Some(repo) => (repo.clone(), true),
        None if cwd.as_os_str().is_empty() => {
            return Err(CliError::new(
                Exit::Failure,
                code::IO,
                "the current directory can't be read; pass --repo <path>",
            ));
        }
        None => (cwd.to_string_lossy().into_owned(), false),
    };
    let location = Location::parse(&text).map_err(|error| {
        CliError::usage(if from_flag {
            format!("--repo: {error}")
        } else {
            error.to_string()
        })
    })?;
    match location {
        Location::Local(path) if path.is_relative() => {
            let joined = cwd.join(&path);
            Ok(Location::parse(&joined.to_string_lossy()).unwrap_or(Location::Local(joined)))
        }
        other => Ok(other),
    }
}

/// Whether two spellings of a path are the same: ignoring case on Windows and macOS, whose
/// file systems usually do.
fn same_path_text(a: &str, b: &str) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// The exit-4 error for a repository inside a WSL distro.
pub(crate) fn wsl_unavailable(distro: &str, path: &str) -> CliError {
    CliError::unavailable(format!(
        "{distro}:{path} is inside WSL: run dv inside the distro for now \
         (wsl -d {distro} --cd {path} dv …); dv on Windows reaches WSL repositories in a later \
         version"
    ))
}
