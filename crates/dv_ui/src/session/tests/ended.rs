//! A watch that ends by itself while the session holds its handle (a remote watch its new
//! server couldn't take back as it was, PLAN §5.2): the session starts it again, reads again
//! what changed meanwhile, and shows what the new watch covers.

use pretty_assertions::assert_eq;

use super::*;

type Slot = Arc<Mutex<Option<WatchSink>>>;

/// A fixture's backend whose watches a test ends: their sinks are dropped as a remote watch's
/// are when it can't subscribe again. Watches from number `degraded_from` (from 0) on come
/// back watching the store alone.
struct Ending {
    inner: Arc<dyn Backend>,
    sinks: Mutex<Vec<Slot>>,
    degraded_from: usize,
}

impl Ending {
    fn new(repo: &FixtureRepo, degraded_from: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: backend(repo),
            sinks: Mutex::new(Vec::new()),
            degraded_from,
        })
    }

    fn watches(&self) -> usize {
        self.sinks.lock().unwrap().len()
    }

    /// Ends every watch: their sinks go, so they deliver nothing more.
    fn end(&self) {
        for slot in self.sinks.lock().unwrap().iter() {
            drop(slot.lock().unwrap().take());
        }
    }
}

impl Backend for Ending {
    fn repo(&self) -> &RepoInfo {
        self.inner.repo()
    }

    fn git(&self, args: &[&str], opts: &GitOpts) -> Result<GitOutput, BackendError> {
        self.inner.git(args, opts)
    }

    fn worktree_read(&self, rel: &str) -> Result<Option<Vec<u8>>, BackendError> {
        self.inner.worktree_read(rel)
    }

    fn worktree_len(&self, rel: &str) -> Result<Option<u64>, BackendError> {
        self.inner.worktree_len(rel)
    }

    fn worktree_text_stats(&self, rel: &str) -> Result<Option<TextStats>, BackendError> {
        self.inner.worktree_text_stats(rel)
    }

    fn store_append(&self, rel: &str, line: &[u8]) -> Result<(), BackendError> {
        self.inner.store_append(rel, line)
    }

    fn store_read(&self, rel: &str) -> Result<Option<Vec<u8>>, BackendError> {
        self.inner.store_read(rel)
    }

    fn store_list(&self, dir: &str) -> Result<Vec<String>, BackendError> {
        self.inner.store_list(dir)
    }

    fn store_write_atomic(&self, rel: &str, bytes: &[u8]) -> Result<(), BackendError> {
        self.inner.store_write_atomic(rel, bytes)
    }

    fn store_remove(&self, rel: &str) -> Result<bool, BackendError> {
        self.inner.store_remove(rel)
    }

    fn copy_index_to_store(&self, rel: &str) -> Result<bool, BackendError> {
        self.inner.copy_index_to_store(rel)
    }

    fn watch(&self, spec: WatchSpec, sink: WatchSink) -> Result<WatchHandle, BackendError> {
        let slot: Slot = Arc::new(Mutex::new(Some(sink)));
        let forward = {
            let slot = slot.clone();
            Box::new(move |event| {
                if let Some(sink) = slot.lock().unwrap().as_mut() {
                    sink(event);
                }
            })
        };
        let handle = self.inner.watch(spec, forward)?;
        let mut sinks = self.sinks.lock().unwrap();
        let number = sinks.len();
        sinks.push(slot);
        Ok(if number >= self.degraded_from {
            handle.with_worktree_error("the store alone now".to_string())
        } else {
            handle
        })
    }

    fn spawn_stdio(
        &self,
        spec: &dv_core::backend::ProcessSpec,
        sink: dv_core::backend::ProcessSink,
    ) -> Result<dv_core::backend::ProcessHandle, BackendError> {
        self.inner.spawn_stdio(spec, sink)
    }
}

#[gpui::test]
fn a_watch_that_ends_by_itself_starts_again_and_says_what_it_covers(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture();
    let ending = Ending::new(&repo, 2);
    let session = session_over(ending.clone(), Target::Working, cx);
    wait(cx, &session, "the list and the watch", |session| {
        session.files().len() == 3 && session.watch.is_registered()
    });
    assert_eq!(ending.watches(), 1);

    // The watch ends; the agent writes meanwhile, which no watch reports.
    ending.end();
    repo.write("while-ended.txt", "x\n");
    wait(
        cx,
        &session,
        "the watch again, and what it missed",
        |session| session.watch.is_registered() && session.file("while-ended.txt").is_some(),
    );
    assert_eq!(ending.watches(), 2);
    session.read_with(cx, |session, _| assert_eq!(session.watch_notice(), None));
    // And it follows the worktree again.
    repo.write("after.txt", "x\n");
    wait(cx, &session, "a live edit", |session| {
        session.file("after.txt").is_some()
    });

    // Ended again, the new watch covers the store alone, and the session says so.
    ending.end();
    wait(cx, &session, "the degraded watch", |session| {
        session.watch.is_registered() && session.watch_notice().is_some()
    });
    assert_eq!(ending.watches(), 3);
    session.read_with(cx, |session, _| {
        assert_eq!(session.watch_notice(), Some("the store alone now"));
        assert_eq!(session.error(), None);
    });
}
