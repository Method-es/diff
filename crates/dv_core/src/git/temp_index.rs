//! Temporary copies of a worktree's index in the store (`<git-common-dir>/dv/tmp/index-<ulid>`),
//! for git calls that need an index other than the user's, which dv never writes (PLAN §2.4):
//! round snapshots stage into one (PLAN §2.3), and a diff whose old side holds files the working
//! tree has untracked names them in one (PLAN §6 step 1).
//!
//! A copy is byte for byte, modification time included ([`Backend::copy_index_to_store`]), so
//! git's cached stat data still saves it rereading unchanged files. The copy and its `.lock` are
//! removed when the [`TempIndex`] drops, also when a step fails; a process killed in between
//! can't remove them, so each new copy first sweeps those older than [`STALE_TEMP`] (their ULID
//! names carry their creation time).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::backend::{Backend, BackendError};

/// The store directory temp indexes live in, relative to the store root.
const TMP_DIR: &str = "tmp";

/// A temp index older than this was left behind by a process that never finished (module
/// docs); one in use, even another dv's snapshot of a large repository, is far younger.
pub const STALE_TEMP: Duration = Duration::from_secs(60 * 60);

/// Settings for the calls that write a temp index:
/// - `core.splitIndex=false`: a split index's shared part lives in the git dir, and git writes
///   a new one there when it writes a split index, even one named by `GIT_INDEX_FILE`; a
///   whole index in the temp file keeps every write in the store.
/// - `core.safecrlf=false`: under `core.safecrlf=true` git refuses to stage a file whose line
///   endings it would convert irreversibly; that guards a commit's checkout, and nothing is
///   ever checked out of a temp index.
pub(crate) const TEMP_INDEX_CONFIG: [&str; 4] =
    ["-c", "core.splitIndex=false", "-c", "core.safecrlf=false"];

/// A copy of the worktree's index in the store, removed with its lock on drop (module docs).
/// Git calls use it through `GitOpts::with_index_file(index.rel())`.
pub(crate) struct TempIndex<'a> {
    backend: &'a dyn Backend,
    rel: String,
}

impl<'a> TempIndex<'a> {
    /// Copies the worktree's index (an empty one when it has none yet) into a new temp file,
    /// after sweeping stale ones.
    pub(crate) fn copy(backend: &'a dyn Backend) -> Result<Self, BackendError> {
        sweep_stale(backend);
        let index = Self {
            backend,
            rel: format!(
                "{TMP_DIR}/index-{}",
                ulid::Ulid::generate().to_string().to_ascii_lowercase()
            ),
        };
        backend.copy_index_to_store(&index.rel)?;
        Ok(index)
    }

    /// The copy's store path.
    pub(crate) fn rel(&self) -> &str {
        &self.rel
    }
}

impl Drop for TempIndex<'_> {
    fn drop(&mut self) {
        for rel in [self.rel.clone(), format!("{}.lock", self.rel)] {
            if let Err(err) = self.backend.store_remove(&rel) {
                log::warn!("removing the temp index {rel}: {err}");
            }
        }
    }
}

/// Removes the temp indexes (and their locks) that processes which never finished left in the
/// store, by the time their ULID names carry. A failure to list or remove one only logs: it
/// costs disk space, never the work at hand.
fn sweep_stale(backend: &dyn Backend) {
    let names = match backend.store_list(TMP_DIR) {
        Ok(names) => names,
        Err(err) => {
            log::warn!("listing the store's temp files: {err}");
            return;
        }
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    for name in names {
        let Some(id) = name
            .strip_prefix("index-")
            .map(|rest| rest.strip_suffix(".lock").unwrap_or(rest))
        else {
            continue;
        };
        let Ok(id) = ulid::Ulid::from_string(id) else {
            continue;
        };
        if now.saturating_sub(u128::from(id.timestamp_ms())) < STALE_TEMP.as_millis() {
            continue;
        }
        let rel = format!("{TMP_DIR}/{name}");
        if let Err(err) = backend.store_remove(&rel) {
            log::warn!("removing the stale temp index {rel}: {err}");
        }
    }
}
