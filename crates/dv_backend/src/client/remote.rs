//! [`RemoteBackend`]: `dv_core::Backend` for one worktree, over a [`Connection`] (docs/PLAN.md
//! §5.1). Every method is one request, answered by the server's `LocalBackend`.
//!
//! The server knows the worktree by the handle its `open` returned, which means nothing to the
//! next server once the connection has reconnected. So a [`Worktree`] keeps its handle with the
//! server generation that issued it, and a call on a newer server opens the worktree there
//! again first (PLAN §5.2): lazily, on the first call that needs it. A worktree gone meanwhile
//! surfaces as that call's error. The same holds after the connection closed its server for
//! idleness: the next call starts one, and opens the worktree there.

use std::sync::{Arc, Weak};

use dv_core::backend::{
    Backend, BackendError, GitOpts, GitOutput, RepoInfo, TextStats, WatchHandle, WatchSink,
    WatchSpec,
};
use parking_lot::Mutex;
use serde::de::DeserializeOwned;

use super::Connection;
use super::link::{CallError, Link};
use crate::proto::{Bytes, Op, OpenReply, WireGitOutput};

/// One worktree's `Backend` through a connection's servers. Its watches run on until their
/// handles drop; the server forgets the worktree (`close`) once the backend and its watches
/// are all gone.
pub struct RemoteBackend {
    connection: Arc<Connection>,
    worktree: Arc<Worktree>,
    info: RepoInfo,
}

/// A worktree as the servers know it (module docs). Shared by its backend and its watches;
/// the last of them to go sends `close`.
pub(super) struct Worktree {
    connection: Weak<Connection>,
    /// The worktree's root as the server names it, which `open` is sent with again.
    path: String,
    env: Vec<(String, Option<String>)>,
    handle: Mutex<Handle>,
}

/// A server's handle for the worktree, and that server's generation.
#[derive(Debug, Clone, Copy)]
struct Handle {
    generation: u64,
    repo: u32,
}

impl Worktree {
    pub(super) fn new(
        connection: Weak<Connection>,
        path: String,
        env: Vec<(String, Option<String>)>,
        generation: u64,
        repo: u32,
    ) -> Self {
        Self {
            connection,
            path,
            env,
            handle: Mutex::new(Handle { generation, repo }),
        }
    }

    /// The worktree's root as the server names it.
    pub(super) fn path(&self) -> &str {
        &self.path
    }

    /// The worktree's handle on `link`'s server, opening it there first when an earlier
    /// server issued the handle. Concurrent callers open it once.
    pub(super) fn handle_on(&self, link: &Link) -> Result<u32, CallError> {
        let mut handle = self.handle.lock();
        if handle.generation == link.generation() {
            return Ok(handle.repo);
        }
        let reply: OpenReply = link.call(
            Op::Open {
                path: self.path.clone(),
                env: self.env.clone(),
            },
            None,
        )?;
        *handle = Handle {
            generation: link.generation(),
            repo: reply.repo,
        };
        Ok(reply.repo)
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        let handle = *self.handle.get_mut();
        if let Some(connection) = self.connection.upgrade() {
            connection.notify(handle.generation, Op::Close { repo: handle.repo });
        }
    }
}

impl RemoteBackend {
    pub(super) fn new(
        connection: Arc<Connection>,
        worktree: Arc<Worktree>,
        info: RepoInfo,
    ) -> Self {
        Self {
            connection,
            worktree,
            info,
        }
    }

    /// The connection this worktree is reached through.
    pub fn connection(&self) -> &Arc<Connection> {
        &self.connection
    }

    /// Sends the op `op` makes of the worktree's handle on the live server, and waits for its
    /// answer.
    fn call<T: DeserializeOwned>(&self, op: impl FnOnce(u32) -> Op) -> Result<T, BackendError> {
        let _busy = self.connection.busy();
        let link = self.connection.link()?;
        let repo = self
            .worktree
            .handle_on(&link)
            .map_err(|error| self.connection.error(error))?;
        link.call(op(repo), None)
            .map_err(|error| self.connection.error(error))
    }
}

impl Backend for RemoteBackend {
    fn repo(&self) -> &RepoInfo {
        &self.info
    }

    fn git(&self, args: &[&str], opts: &GitOpts) -> Result<GitOutput, BackendError> {
        let output: WireGitOutput = self.call(|repo| Op::Git {
            repo,
            args: args.iter().map(|arg| arg.to_string()).collect(),
            opts: opts.clone().into(),
        })?;
        Ok(output.into())
    }

    fn worktree_read(&self, rel: &str) -> Result<Option<Vec<u8>>, BackendError> {
        let bytes: Option<Bytes> = self.call(|repo| Op::WorktreeRead {
            repo,
            rel: rel.to_string(),
        })?;
        Ok(bytes.map(|bytes| bytes.0))
    }

    fn worktree_len(&self, rel: &str) -> Result<Option<u64>, BackendError> {
        self.call(|repo| Op::WorktreeLen {
            repo,
            rel: rel.to_string(),
        })
    }

    fn worktree_text_stats(&self, rel: &str) -> Result<Option<TextStats>, BackendError> {
        self.call(|repo| Op::WorktreeTextStats {
            repo,
            rel: rel.to_string(),
        })
    }

    fn store_append(&self, rel: &str, line: &[u8]) -> Result<(), BackendError> {
        self.call(|repo| Op::StoreAppend {
            repo,
            rel: rel.to_string(),
            line: line.to_vec(),
        })
    }

    fn store_read(&self, rel: &str) -> Result<Option<Vec<u8>>, BackendError> {
        let bytes: Option<Bytes> = self.call(|repo| Op::StoreRead {
            repo,
            rel: rel.to_string(),
        })?;
        Ok(bytes.map(|bytes| bytes.0))
    }

    fn store_list(&self, dir: &str) -> Result<Vec<String>, BackendError> {
        self.call(|repo| Op::StoreList {
            repo,
            dir: dir.to_string(),
        })
    }

    fn store_write_atomic(&self, rel: &str, bytes: &[u8]) -> Result<(), BackendError> {
        self.call(|repo| Op::StoreWriteAtomic {
            repo,
            rel: rel.to_string(),
            bytes: bytes.to_vec(),
        })
    }

    fn store_remove(&self, rel: &str) -> Result<bool, BackendError> {
        self.call(|repo| Op::StoreRemove {
            repo,
            rel: rel.to_string(),
        })
    }

    fn copy_index_to_store(&self, rel: &str) -> Result<bool, BackendError> {
        self.call(|repo| Op::CopyIndexToStore {
            repo,
            rel: rel.to_string(),
        })
    }

    fn watch(&self, spec: WatchSpec, sink: WatchSink) -> Result<WatchHandle, BackendError> {
        self.connection
            .watch(&self.worktree, &self.info.toplevel, spec, sink)
    }
}

impl std::fmt::Debug for RemoteBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteBackend")
            .field("connection", &self.connection.name())
            .field("toplevel", &self.info.toplevel)
            .finish_non_exhaustive()
    }
}
