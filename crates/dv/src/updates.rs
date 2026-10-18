//! An installed dv's updates (docs/PLAN.md D9, §13.3): Velopack's `UpdateManager` over the
//! release feed, in the background, reported to `dv_ui::services::Updates`, whose status-bar
//! item restarts into the update.
//!
//! **The feed** is the one the build names as `DV_UPDATE_FEED`, read through Velopack's
//! `AutoSource`: a released build's is the repository on GitHub, read as its releases; a
//! locally packaged one's the folder `script/package --publish` writes (the packaging
//! checkout's `target\package\releases`); `settings.json`'s `update.feed` names another when
//! set (a folder or a URL; empty turns updates off) ([`feed`]).
//!
//! **When:** a few seconds after the app starts, then every [`CHECK_EVERY`]; never under
//! `--automation` (scenarios show the item through the seam), and never in a build that isn't
//! installed: `UpdateManager::new` fails there (no `Update.exe` above it) and updates are off,
//! silently. A newer release is downloaded (its delta when there is one) and then shown; one
//! downloaded before and not yet applied shows at once ([`check`]).
//!
//! **Restart to update** first counts the other dv processes running from the install
//! (`crate::processes`), which applying stops, and the service asks before it goes on when
//! there are any or this window holds unsent text (`dv_ui::services::updates`). Then it hands
//! the update to `Update.exe`, which waits for this process to quit, applies it and starts dv
//! again on what this window had open, named absolutely (`crate::launch::open_args`): the
//! restarted app starts in the install's folder, whatever folder this one started in
//! ([`restart`]). The app then quits as it always does (settings flushed).

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use dv_ui::services::Updater;
use dv_ui::{ActiveServices as _, Shell};
use gpui::{App, Entity};
use velopack::sources::AutoSource;
use velopack::{UpdateCheck, UpdateInfo, UpdateManager};

/// How long after startup the first check waits, so it doesn't compete with opening a review.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(10);

/// How often the feed is checked again.
pub const CHECK_EVERY: Duration = Duration::from_secs(4 * 60 * 60);

/// The feed this build names: `script/package`'s `DV_UPDATE_FEED`, `None` in a development
/// build.
const BUILT_FEED: Option<&str> = option_env!("DV_UPDATE_FEED");

/// The feed to check: `setting` (`update.feed`) when it is set, empty meaning none, else the
/// build's `built` (module docs).
pub fn feed(setting: Option<String>, built: Option<&str>) -> Option<String> {
    let feed = setting.or_else(|| built.map(str::to_string))?;
    let feed = feed.trim();
    (!feed.is_empty()).then(|| feed.to_string())
}

/// What updating needs from Velopack, apart so the decisions are tested without an install.
pub trait Releases: Send + Sync {
    /// The version downloaded before and not applied yet.
    fn pending(&self) -> Option<String>;
    /// The feed's newest release when it is newer than this copy (remembered for
    /// [`Releases::download`]). Blocks.
    fn newer(&self) -> Result<Option<String>, String>;
    /// Downloads what [`Releases::newer`] found. Blocks.
    fn download(&self) -> Result<(), String>;
    /// Has `Update.exe` wait for this process, apply the pending update and start dv with
    /// `args`.
    fn apply(&self, args: &[OsString]) -> Result<(), String>;
}

/// The version ready to restart into: one downloaded before, else the feed's newest when it is
/// newer, downloaded now. Blocks.
pub fn check(releases: &dyn Releases) -> Result<Option<String>, String> {
    if let Some(pending) = releases.pending() {
        return Ok(Some(pending));
    }
    match releases.newer()? {
        Some(version) => {
            releases.download()?;
            Ok(Some(version))
        }
        None => Ok(None),
    }
}

/// Restart to update (module docs): `Update.exe` waits for this process, applies the update
/// and starts dv with `args`; then `quit` ends the app.
pub fn restart(
    releases: &dyn Releases,
    args: &[OsString],
    quit: impl FnOnce(),
) -> Result<(), String> {
    if releases.pending().is_none() {
        return Err("the update isn't downloaded any more".to_string());
    }
    releases.apply(args)?;
    quit();
    Ok(())
}

/// [`Releases`] through Velopack's `UpdateManager`.
struct Velopack {
    manager: UpdateManager,
    found: Mutex<Option<UpdateInfo>>,
}

impl Releases for Velopack {
    fn pending(&self) -> Option<String> {
        self.manager
            .get_update_pending_restart()
            .map(|asset| asset.Version)
    }

    fn newer(&self) -> Result<Option<String>, String> {
        match self
            .manager
            .check_for_updates()
            .map_err(|e| e.to_string())?
        {
            UpdateCheck::UpdateAvailable(update) => {
                let version = update.TargetFullRelease.Version.clone();
                *lock(&self.found) = Some(*update);
                Ok(Some(version))
            }
            UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty => Ok(None),
        }
    }

    fn download(&self) -> Result<(), String> {
        let found = lock(&self.found).clone().ok_or("nothing to download")?;
        self.manager
            .download_updates(&found, None)
            .map_err(|error| error.to_string())
    }

    fn apply(&self, args: &[OsString]) -> Result<(), String> {
        let pending = self
            .manager
            .get_update_pending_restart()
            .ok_or("the update isn't downloaded any more")?;
        self.manager
            .wait_exit_then_apply_updates(&pending, false, true, args)
            .map_err(|error| error.to_string())
    }
}

/// `mutex`'s value, also after a panic elsewhere left it poisoned (it holds plain data).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What a restarted app opens: the open review's repository and review, absolutely (module
/// docs), else nothing.
fn reopen_args(shell: &Entity<Shell>, cx: &App) -> Vec<OsString> {
    let Some(session) = shell.read(cx).session() else {
        return Vec::new();
    };
    let session = session.read(cx);
    crate::launch::open_args(session.origin(), Some(session.review_id()))
}

/// Starts checking for updates, unless this is an automation run or not an installed copy
/// (module docs); `shell` is the main window's, whose open review a restart reopens.
pub fn start(automation: bool, shell: Entity<Shell>, cx: &mut App) {
    if automation {
        return;
    }
    let setting = cx.services().settings.read(cx).file().update_feed();
    let Some(feed) = feed(setting, BUILT_FEED) else {
        log::info!("updates: no feed");
        return;
    };
    let manager = match UpdateManager::new(AutoSource::new(&feed), None, None) {
        Ok(manager) => manager,
        Err(error) => {
            log::info!("updates: off ({error})");
            return;
        }
    };
    let Some(current) = crate::installer::installed_dir() else {
        log::info!("updates: off (not a packaged install)");
        return;
    };
    log::info!(
        "updates: dv {} checks {feed}",
        manager.get_current_version_as_string()
    );
    let releases: std::sync::Arc<dyn Releases> = std::sync::Arc::new(Velopack {
        manager,
        found: Mutex::new(None),
    });
    let updates = cx.services().updates.clone();
    updates.update(cx, |updates, cx| {
        let others = {
            let current: PathBuf = current.clone();
            Box::new(move || crate::processes::others_in(&current))
        };
        let restart = {
            let releases = releases.clone();
            Box::new(move |cx: &mut App| {
                let args = reopen_args(&shell, cx);
                restart(releases.as_ref(), &args, || cx.quit())
            })
        };
        updates.set_updater(Updater::Real { others, restart });
        cx.spawn(async move |this, cx| {
            let mut wait = FIRST_CHECK_AFTER;
            loop {
                cx.background_executor().timer(wait).await;
                wait = CHECK_EVERY;
                let releases = releases.clone();
                let checked = cx
                    .background_executor()
                    .spawn(async move { check(releases.as_ref()) })
                    .await;
                match checked {
                    Ok(Some(version)) => {
                        let shown = this.update(cx, |updates, cx| updates.set_ready(version, cx));
                        if shown.is_err() {
                            return;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => log::warn!("updates: {error}"),
                }
            }
        })
        .detach();
    });
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn the_setting_names_the_feed_else_the_build_does() {
        let built = Some(r"D:\Software\diff\target\package\releases");
        assert_eq!(feed(None, built).as_deref(), built);
        assert_eq!(
            feed(Some(r" E:\feed ".into()), built).as_deref(),
            Some(r"E:\feed")
        );
        assert_eq!(feed(Some(String::new()), built), None, "empty turns it off");
        assert_eq!(feed(None, None), None, "a development build has none");
        assert_eq!(feed(Some("  ".into()), None), None);
    }

    #[test]
    fn a_development_build_has_no_updates() {
        // Not installed: the manager can't be made, which turns updates off.
        assert!(UpdateManager::new(AutoSource::new(r"C:\no\such\feed"), None, None).is_err());
        assert_eq!(crate::installer::installed_dir(), None);
    }

    /// A feed and an install that only say what they're asked to.
    #[derive(Default)]
    struct Fake {
        pending: Mutex<Option<String>>,
        newer: Option<String>,
        fail: Option<String>,
        calls: Mutex<Vec<String>>,
    }

    impl Releases for Fake {
        fn pending(&self) -> Option<String> {
            lock(&self.calls).push("pending".into());
            lock(&self.pending).clone()
        }
        fn newer(&self) -> Result<Option<String>, String> {
            lock(&self.calls).push("newer".into());
            match &self.fail {
                Some(error) => Err(error.clone()),
                None => Ok(self.newer.clone()),
            }
        }
        fn download(&self) -> Result<(), String> {
            lock(&self.calls).push("download".into());
            *lock(&self.pending) = self.newer.clone();
            Ok(())
        }
        fn apply(&self, args: &[OsString]) -> Result<(), String> {
            let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
            lock(&self.calls).push(format!("apply {}", args.join(" ")));
            Ok(())
        }
    }

    #[test]
    fn a_check_downloads_a_newer_release_or_reports_the_one_waiting() {
        let fake = Fake {
            newer: Some("0.1.0-local.2".into()),
            ..Fake::default()
        };
        assert_eq!(check(&fake), Ok(Some("0.1.0-local.2".into())));
        assert_eq!(*lock(&fake.calls), ["pending", "newer", "download"]);
        // Downloaded already: no feed read, no second download.
        lock(&fake.calls).clear();
        assert_eq!(check(&fake), Ok(Some("0.1.0-local.2".into())));
        assert_eq!(*lock(&fake.calls), ["pending"]);

        let none = Fake::default();
        assert_eq!(check(&none), Ok(None));
        assert_eq!(*lock(&none.calls), ["pending", "newer"]);
        let offline = Fake {
            fail: Some("the feed's folder isn't there".into()),
            ..Fake::default()
        };
        assert_eq!(check(&offline), Err("the feed's folder isn't there".into()));
    }

    #[test]
    fn a_restart_applies_what_is_downloaded_with_the_open_review_then_quits() {
        let fake = Fake::default();
        let quit = Cell::new(false);
        let args: Vec<OsString> = ["open", r"D:\code\api", "--review", "r-1"]
            .iter()
            .map(OsString::from)
            .collect();
        // Nothing downloaded: an error, and the app keeps running.
        assert!(restart(&fake, &args, || quit.set(true)).is_err());
        assert!(!quit.get());
        *lock(&fake.pending) = Some("2.0.0".into());
        assert_eq!(restart(&fake, &args, || quit.set(true)), Ok(()));
        assert!(quit.get());
        assert_eq!(
            lock(&fake.calls).last().map(String::as_str),
            Some(r"apply open D:\code\api --review r-1")
        );
    }
}
