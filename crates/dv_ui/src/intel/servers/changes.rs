//! Changed files reaching the servers (PLAN §18.5, `super` module docs "Changed files").
//!
//! The review's watch reports the worktree paths that changed, or a rescan (anything may have).
//! The resolver forgets what an install can change, and every ready server hears of what
//! changed, one batch at a time in the watch's order (a later batch never overtakes an earlier
//! one):
//!
//! - **Its documents.** A server takes an open document's text from dv, not the disk, so each
//!   path that changed is closed on every client (on a rescan, every open document is), and the
//!   next request opens it afresh from what the pane shows.
//! - **Its watchers** (`workspace/didChangeWatchedFiles`). The paths any server's watchers ask
//!   about are looked at once per batch, whichever servers ask (each look is a call to the
//!   backend, a WSL one's a round trip), each told to the servers that watch it in one call per
//!   server: a deletion when it is gone; a creation when it didn't exist when last looked at
//!   and does now; a change when it did; and for a path never looked at before that is there,
//!   a creation and a change both (it may be new, which a server only takes up as a creation,
//!   or one it knows, which a change refreshes). On a rescan the paths are the documents it closed, the
//!   ones the servers are known to hold: closing them is enough for their text, and a server
//!   reads any other file from the disk when it next needs it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use dv_core::Location;
use dv_lsp::LspClient;
use dv_lsp::client::{CHANGE, CREATE, DELETE};
use gpui::{AppContext as _, Context};
use parking_lot::Mutex;

use super::{LanguageServers, ServerState, requeue};

/// Whether each path a server was told about existed when it was last looked at, per worktree.
pub(super) type Existed = Arc<Mutex<HashMap<String, bool>>>;

impl LanguageServers {
    /// The review's watch saw `paths` change (`rescan`: anything may have; module docs).
    pub fn files_changed(
        &mut self,
        worktree: &Location,
        paths: &[String],
        rescan: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return;
        };
        let forgot = entry.resolver.as_ref().is_some_and(|resolver| {
            if rescan {
                resolver.forget();
                true
            } else {
                resolver.files_changed(paths.iter().map(String::as_str))
            }
        });
        if forgot {
            requeue(entry);
        }
        let ready = entry
            .servers
            .values()
            .any(|server| server.state == ServerState::Ready);
        if !ready || (paths.is_empty() && !rescan) {
            if forgot {
                self.pump(worktree, cx);
            }
            return;
        }
        // One batch at a time, in the order the watch reported them: a server hears of a
        // file's changes in order.
        entry.announce_queue.push_back((paths.to_vec(), rescan));
        entry.announcing += 1;
        if !entry.announce_running {
            self.announce_next(worktree, cx);
        }
        if forgot {
            self.pump(worktree, cx);
        }
    }

    /// Tells the ready servers about the next batch queued, in the background; the one after
    /// starts once it is told.
    fn announce_next(&mut self, worktree: &Location, cx: &mut Context<Self>) {
        let Some(entry) = self.worktrees.get_mut(worktree) else {
            return;
        };
        let Some((paths, rescan)) = entry.announce_queue.pop_front() else {
            entry.announce_running = false;
            return;
        };
        entry.announce_running = true;
        let clients: Vec<Arc<LspClient>> = entry
            .servers
            .values()
            .filter(|server| server.state == ServerState::Ready)
            .filter_map(|server| server.client.as_ref()?.client().cloned())
            .collect();
        let backend = entry.backend.clone();
        let existed = entry.existed.clone();
        let job = cx.background_spawn(async move {
            announce(
                &clients,
                &paths,
                rescan,
                |path| {
                    // Gone, or there (a failed look counts as there: a change).
                    !matches!(backend.worktree_len(path), Ok(None))
                },
                &existed,
            );
        });
        let key = worktree.clone();
        cx.spawn(async move |this, cx| {
            job.await;
            this.update(cx, |this, cx| {
                if let Some(entry) = this.worktrees.get_mut(&key) {
                    entry.announcing = entry.announcing.saturating_sub(1);
                }
                this.announce_next(&key, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Tells `clients` about `paths` (module docs): closes their documents among them (every
/// document on a `rescan`, whose paths are then the ones announced), looks once at each path a
/// watcher asks about (`exists`), and sends each client its changes in one call. Blocking.
pub(super) fn announce(
    clients: &[Arc<LspClient>],
    paths: &[String],
    rescan: bool,
    exists: impl Fn(&str) -> bool,
    existed: &Existed,
) {
    let mut announced: Vec<String> = paths.to_vec();
    for client in clients {
        if rescan {
            for path in client.open_documents() {
                client.close_document(&path);
                announced.push(path);
            }
        } else {
            for path in paths {
                client.close_document(path);
            }
        }
    }
    announced.sort();
    announced.dedup();
    // Each path worth telling some server about, looked at once.
    let watched: Vec<Vec<String>> = clients
        .iter()
        .map(|client| client.watched(&announced))
        .collect();
    let looked: HashSet<&String> = watched.iter().flatten().collect();
    let kinds: HashMap<String, &'static [u8]> = {
        let mut existed = existed.lock();
        looked
            .into_iter()
            .map(|path| {
                let now = exists(path);
                let before = existed.insert(path.clone(), now);
                let kinds: &'static [u8] = match (before, now) {
                    (_, false) => &[DELETE],
                    (Some(false), true) => &[CREATE],
                    (Some(true), true) => &[CHANGE],
                    (None, true) => &[CREATE, CHANGE],
                };
                (path.clone(), kinds)
            })
            .collect()
    };
    for (client, watched) in clients.iter().zip(watched) {
        if watched.is_empty() {
            continue;
        }
        let changes: Vec<(String, u8)> = watched
            .into_iter()
            .flat_map(|path| {
                let kinds = kinds.get(&path).copied().unwrap_or_default();
                kinds.iter().map(move |kind| (path.clone(), *kind))
            })
            .collect();
        client.files_changed(&changes);
    }
}
