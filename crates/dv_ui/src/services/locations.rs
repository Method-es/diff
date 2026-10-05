//! Where repositories live and the backends that reach them (PLAN §11 `Locations`, §5.1).
//!
//! [`Locations::open`] turns a [`Location`] into the one `Arc<dyn Backend>` dv keeps for that
//! repository, opened off the UI thread (opening runs git) and cached by both the location
//! asked for and the worktree root it resolved to, so the switcher, the reviews list and an
//! open review share a backend. A local path opens a [`LocalBackend`] with the user's own git
//! configuration, unless an environment overlay is registered for it
//! ([`Locations::register_env`]): the automation host registers its seeded fixture repos'
//! overlays, so scenarios never depend on the owner's git configuration (PLAN §14.2). A WSL
//! location fails with [`BackendError::Unavailable`] until the in-distro backend exists
//! (PLAN D6); it never boots a distro.
//!
//! A cached backend whose worktree root has since disappeared is dropped and the location
//! opened again, so a deleted repo reports "no such directory" instead of a git spawn error.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dv_core::{Backend, BackendError, LocalBackend, LocalBackendOptions, Location};
use futures::channel::oneshot;
use gpui::{AppContext as _, Context, Task};

/// Environment changes for every git a backend spawns, as `LocalBackendOptions::env` takes
/// them: `(name, Some(value))` sets a variable, `(name, None)` removes it.
pub type EnvOverlay = Vec<(OsString, Option<OsString>)>;

/// What [`Locations::open`] hands back: the backend, or why there is none. The error is shared
/// because more than one view may show it.
pub type OpenResult = Result<Arc<dyn Backend>, Arc<BackendError>>;

/// The backends of the repositories dv has opened (see the module docs).
#[derive(Default)]
pub struct Locations {
    backends: HashMap<Location, Arc<dyn Backend>>,
    /// Overlays by the root they apply to (a path and everything below it).
    overlays: Vec<(PathBuf, EnvOverlay)>,
    /// Opens in flight.
    opening: usize,
}

impl Locations {
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes every backend opened for `root`, or a path inside it, run git with `env` laid
    /// over the inherited environment (`LocalBackendOptions::env`). Registering a root again
    /// replaces its overlay; a backend already open keeps the one it opened with.
    pub fn register_env(&mut self, root: PathBuf, env: EnvOverlay) {
        self.overlays.retain(|(known, _)| known != &root);
        self.overlays.push((root, env));
    }

    /// The backend already open for `location`, if any.
    pub fn cached(&self, location: &Location) -> Option<Arc<dyn Backend>> {
        self.backends.get(location).cloned()
    }

    /// Nothing is opening: automation's `settled`.
    pub fn is_settled(&self) -> bool {
        self.opening == 0
    }

    /// The backend for `location`: the cached one while its worktree still exists, else a
    /// newly opened one (see the module docs). Never blocks the UI thread. The open finishes
    /// (and is cached) even if the returned task is dropped.
    pub fn open(&mut self, location: Location, cx: &mut Context<Self>) -> Task<OpenResult> {
        let cached = self.cached(&location);
        let overlays = self.overlays.clone();
        self.opening += 1;
        let job = cx.background_spawn({
            let (location, cached) = (location.clone(), cached.clone());
            async move { open_blocking(&location, cached, &overlays) }
        });
        let (sender, receiver) = oneshot::channel();
        cx.spawn({
            let location = location.clone();
            async move |this, cx| {
                let opened = job.await.map_err(Arc::new);
                let opened = this
                    .update(cx, |this, cx| {
                        this.opening -= 1;
                        cx.notify();
                        this.opened(&location, cached.as_ref(), opened.clone())
                    })
                    .unwrap_or(opened);
                let _ = sender.send(opened);
            }
        })
        .detach();
        cx.background_spawn(async move {
            receiver.await.unwrap_or_else(|_| {
                Err(Arc::new(BackendError::Unavailable {
                    location: location.display_name(),
                    reason: "dv is closing".to_string(),
                }))
            })
        })
    }

    /// Caches what an open of `location` produced, given the backend that was cached when it
    /// started. When another open of the same repository finished first (nothing was cached
    /// then), that backend wins, so a repository has one backend; a cached backend the open
    /// found stale is replaced, or forgotten when the location no longer opens.
    fn opened(
        &mut self,
        location: &Location,
        cached: Option<&Arc<dyn Backend>>,
        opened: OpenResult,
    ) -> OpenResult {
        match opened {
            Ok(backend) => {
                let root = backend.repo().location.clone();
                let backend = match (cached, self.backends.get(&root)) {
                    (None, Some(first)) => first.clone(),
                    _ => backend,
                };
                if let Some(stale) = cached.filter(|stale| !Arc::ptr_eq(stale, &backend)) {
                    self.backends.retain(|_, kept| !Arc::ptr_eq(kept, stale));
                }
                self.backends.insert(location.clone(), backend.clone());
                self.backends.insert(root, backend.clone());
                Ok(backend)
            }
            Err(error) => {
                self.backends.remove(location);
                if let Some(stale) = cached {
                    self.backends.retain(|_, kept| !Arc::ptr_eq(kept, stale));
                }
                Err(error)
            }
        }
    }
}

/// Opens `location` on this thread: `cached` while its worktree root and git dir still exist,
/// else a new backend with the overlay registered for the path, if any. A repository whose
/// `.git` was removed opens anew, and fails as a fresh start would, rather than reusing a
/// backend over a folder that is no longer a repository.
fn open_blocking(
    location: &Location,
    cached: Option<Arc<dyn Backend>>,
    overlays: &[(PathBuf, EnvOverlay)],
) -> Result<Arc<dyn Backend>, BackendError> {
    if let Some(backend) = cached
        && still_there(backend.repo())
    {
        return Ok(backend);
    }
    match location {
        Location::Local(path) => {
            let options = LocalBackendOptions {
                env: overlay_for(path, overlays).unwrap_or_default(),
            };
            Ok(Arc::new(LocalBackend::open_with(path, options)?))
        }
        Location::Wsl { .. } => location.open_backend(),
    }
}

/// Whether the repository a backend was opened on is still there: its worktree root, the
/// root's `.git` (a directory, or a file for a linked worktree) and its git dir.
fn still_there(repo: &dv_core::backend::RepoInfo) -> bool {
    let toplevel = Path::new(&repo.toplevel);
    toplevel.is_dir()
        && std::fs::symlink_metadata(toplevel.join(".git")).is_ok()
        && Path::new(&repo.git_dir).is_dir()
}

/// The overlay registered for `path` or the closest root above it. Paths compare in their
/// canonical form, so `D:\repos\api`, `d:/repos/api/` and a path through a junction match.
fn overlay_for(path: &Path, overlays: &[(PathBuf, EnvOverlay)]) -> Option<EnvOverlay> {
    let canonical = |path: &Path| std::fs::canonicalize(path).ok();
    let path = canonical(path)?;
    overlays
        .iter()
        .filter_map(|(root, env)| {
            let root = canonical(root)?;
            path.starts_with(&root).then_some((root, env))
        })
        .max_by_key(|(root, _)| root.components().count())
        .map(|(_, env)| env.clone())
}

#[cfg(test)]
mod tests {
    use dv_test_support::FixtureRepo;
    use gpui::{Entity, TestAppContext};

    use super::*;

    async fn open(
        locations: &Entity<Locations>,
        location: Location,
        cx: &mut TestAppContext,
    ) -> OpenResult {
        locations
            .update(cx, |locations, cx| locations.open(location, cx))
            .await
    }

    #[gpui::test]
    async fn a_repo_opens_once_and_is_shared_by_its_root(cx: &mut TestAppContext) {
        let repo = FixtureRepo::new();
        repo.write("src/lib.rs", "fn a() {}\n");
        repo.commit("base");
        let locations = cx.new(|_| Locations::new());
        locations.update(cx, |locations, _| {
            locations.register_env(repo.path().to_path_buf(), repo.env_overlay())
        });
        let inside = Location::Local(repo.path().join("src"));
        let first = open(&locations, inside.clone(), cx)
            .await
            .expect("the repo opens");
        assert_eq!(
            Path::new(&first.repo().toplevel),
            repo.path(),
            "a path inside the work tree opens its root"
        );
        let root = first.repo().location.clone();
        let again = open(&locations, root.clone(), cx).await.unwrap();
        assert!(Arc::ptr_eq(&first, &again), "one backend per repo");
        locations.read_with(cx, |locations, _| {
            assert!(locations.is_settled());
            assert!(locations.cached(&inside).is_some());
            assert!(locations.cached(&root).is_some());
        });
    }

    #[gpui::test]
    async fn racing_opens_share_one_backend_and_a_dropped_open_still_lands(
        cx: &mut TestAppContext,
    ) {
        let (repo, other) = (FixtureRepo::new(), FixtureRepo::new());
        let locations = cx.new(|_| Locations::new());
        locations.update(cx, |locations, _| {
            locations.register_env(repo.path().to_path_buf(), repo.env_overlay());
            locations.register_env(other.path().to_path_buf(), other.env_overlay());
        });
        let location = Location::Local(repo.path().to_path_buf());
        let (first, second) = locations.update(cx, |locations, cx| {
            (
                locations.open(location.clone(), cx),
                locations.open(Location::Local(repo.path().join(".")), cx),
            )
        });
        let (first, second) = (first.await.unwrap(), second.await.unwrap());
        assert!(Arc::ptr_eq(&first, &second), "one backend per repo");

        let dropped = locations.update(cx, |locations, cx| {
            locations.open(Location::Local(other.path().to_path_buf()), cx)
        });
        drop(dropped);
        cx.run_until_parked();
        locations.read_with(cx, |locations, _| {
            assert!(locations.is_settled());
            assert!(
                locations
                    .cached(&Location::Local(other.path().to_path_buf()))
                    .is_some()
            );
        });
    }

    #[gpui::test]
    async fn the_registered_overlay_reaches_git(cx: &mut TestAppContext) {
        let repo = FixtureRepo::new();
        // Only the fixture's overlay points git at this global config, so only a backend
        // opened with it sees the alias.
        std::fs::write(
            repo.home().join(".gitconfig"),
            "[dv-test]\n\tmarker = from-the-overlay\n",
        )
        .unwrap();
        let locations = cx.new(|_| Locations::new());
        locations.update(cx, |locations, _| {
            locations.register_env(repo.path().to_path_buf(), repo.env_overlay())
        });
        let backend = open(&locations, Location::Local(repo.path().to_path_buf()), cx)
            .await
            .unwrap();
        let output = backend
            .git(
                &["config", "--get", "dv-test.marker"],
                &dv_core::backend::GitOpts::default(),
            )
            .unwrap();
        assert_eq!(output.stdout_utf8().unwrap().trim(), "from-the-overlay");
    }

    #[gpui::test]
    async fn wsl_is_unavailable_and_missing_paths_say_so(cx: &mut TestAppContext) {
        let locations = cx.new(|_| Locations::new());
        let wsl = Location::Wsl {
            distro: "Ubuntu".into(),
            path: "/home/kyle/api".into(),
        };
        let error = open(&locations, wsl.clone(), cx)
            .await
            .err()
            .expect("no WSL backend yet");
        assert!(
            matches!(*error, BackendError::Unavailable { .. }),
            "{error:?}"
        );
        let dir = tempfile::tempdir().unwrap();
        let gone = Location::Local(dir.path().join("missing"));
        let error = open(&locations, gone, cx).await.err().unwrap();
        assert!(
            matches!(*error, BackendError::PathNotFound { .. }),
            "{error:?}"
        );
        locations.read_with(cx, |locations, _| {
            assert!(locations.is_settled());
            assert!(locations.cached(&wsl).is_none());
        });
    }

    #[gpui::test]
    async fn a_deleted_repo_is_opened_again_and_reported_missing(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api");
        let repo = FixtureRepo::persistent(&path);
        repo.write("a.txt", "a\n");
        repo.commit("base");
        let locations = cx.new(|_| Locations::new());
        locations.update(cx, |locations, _| {
            locations.register_env(repo.path().to_path_buf(), repo.env_overlay())
        });
        let location = Location::Local(repo.path().to_path_buf());
        open(&locations, location.clone(), cx)
            .await
            .expect("the repo opens");
        std::fs::remove_dir_all(repo.path()).unwrap();
        let error = open(&locations, location.clone(), cx)
            .await
            .err()
            .expect("gone");
        assert!(
            matches!(*error, BackendError::PathNotFound { .. }),
            "{error:?}"
        );
        locations.read_with(cx, |locations, _| {
            assert!(locations.cached(&location).is_none())
        });
    }

    #[gpui::test]
    async fn a_folder_whose_git_dir_went_is_opened_again_and_fails(cx: &mut TestAppContext) {
        let repo = FixtureRepo::new();
        repo.write("a.txt", "a\n");
        repo.commit("base");
        let locations = cx.new(|_| Locations::new());
        locations.update(cx, |locations, _| {
            locations.register_env(repo.path().to_path_buf(), repo.env_overlay())
        });
        let location = Location::Local(repo.path().to_path_buf());
        open(&locations, location.clone(), cx)
            .await
            .expect("the repo opens");
        // The folder stays; it is no longer a repository.
        std::fs::remove_dir_all(repo.path().join(".git")).unwrap();
        assert!(
            open(&locations, location.clone(), cx).await.is_err(),
            "not the cached backend over a folder without a repository"
        );
        locations.read_with(cx, |locations, _| {
            assert!(locations.cached(&location).is_none())
        });
    }
}
