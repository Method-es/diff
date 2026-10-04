//! The store on fixture repos (PLAN §7): logs in the common dir, list, delete with refs, the
//! snapshot cache.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::*;
use crate::backend::{
    Backend, BackendError, GitOpts, GitOutput, LocalBackend, RepoInfo, WatchHandle, WatchSink,
    WatchSpec,
};
use crate::testing::open_at;

fn log_file(repo: &FixtureRepo, id: &ReviewId) -> PathBuf {
    repo.path()
        .join(".git")
        .join("dv")
        .join("reviews")
        .join(format!("{id}.jsonl"))
}

fn snapshot_rel(id: &ReviewId) -> String {
    format!("reviews/{id}.state.json")
}

/// The snapshot cache as JSON.
fn snapshot(store: &ReviewStore, id: &ReviewId) -> serde_json::Value {
    let bytes = store
        .backend()
        .store_read(&snapshot_rel(id))
        .unwrap()
        .expect("a snapshot cache");
    serde_json::from_slice(&bytes).unwrap()
}

fn status(status: ReviewStatus) -> EventBody {
    EventBody::Status { status }
}

#[test]
fn create_append_load() {
    let (repo, store) = fixture_store();
    let review = store.create(Target::Working, reviewer()).unwrap();
    assert_eq!(review.target, Target::Working);
    assert_eq!(review.status, ReviewStatus::Open);
    assert_eq!(review.worktree, store.worktree());
    assert!(Path::new(&review.worktree).is_absolute());
    let file = log_file(&repo, &review.id);
    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert!(text.contains(r#""kind":"created""#), "{text}");

    let loaded = store.load(&review.id).unwrap();
    assert_eq!(loaded.review, review);
    assert!(loaded.warnings.is_empty() && loaded.newer_events == 0);

    let thread = ThreadId::generate();
    let appended = store
        .append(&review.id, thread_added(&thread, reviewer()))
        .unwrap();
    assert_eq!(appended.v, EVENT_VERSION);
    store
        .append(&review.id, status(ReviewStatus::ChangesRequested))
        .unwrap();
    let loaded = store.load(&review.id).unwrap();
    assert_eq!(loaded.review.status, ReviewStatus::ChangesRequested);
    assert_eq!(loaded.review.threads[0].id, thread);
    assert_eq!(std::fs::read_to_string(&file).unwrap().lines().count(), 3);

    let events = store.events(&review.id).unwrap().events;
    assert_eq!(events.len(), 3);
    assert_eq!(events[1], appended);
}

#[test]
fn unknown_reviews_and_appended_created_events_are_refused() {
    let (_repo, store) = fixture_store();
    let ghost = ReviewId::generate();
    assert!(matches!(
        store.load(&ghost),
        Err(StoreError::UnknownReview { .. })
    ));
    assert!(matches!(
        store.append(&ghost, status(ReviewStatus::Closed)),
        Err(StoreError::UnknownReview { .. })
    ));
    // Appending to a review that doesn't exist creates nothing.
    assert!(store.list().unwrap().is_empty());
    assert!(!store.delete(&ghost).unwrap());

    let review = store.create(Target::Staged, reviewer()).unwrap();
    assert!(matches!(
        store.append(&review.id, created(Target::Working)),
        Err(StoreError::CreatedEvent)
    ));
}

#[test]
fn list_shows_every_review_newest_first_and_a_bad_log_as_an_error() {
    let (repo, store) = fixture_store();
    let first = store.create(Target::Working, reviewer()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let second = store
        .create(
            Target::Commit {
                rev: "HEAD".to_string(),
            },
            reviewer(),
        )
        .unwrap();
    store
        .append(&second.id, thread_added(&ThreadId::generate(), reviewer()))
        .unwrap();

    // A log with garbage in it, an empty one and one without a created event: each listed
    // with its error, none hiding the good ones.
    let backend = store.backend();
    let garbage = ReviewId::generate();
    backend
        .store_append(&format!("reviews/{garbage}.jsonl"), b"garbage")
        .unwrap();
    let empty = ReviewId::generate();
    backend
        .store_write_atomic(&format!("reviews/{empty}.jsonl"), b"")
        .unwrap();
    let headless = ReviewId::generate();
    backend
        .store_append(
            &format!("reviews/{headless}.jsonl"),
            &Event::new(status(ReviewStatus::Closed)).to_line().unwrap(),
        )
        .unwrap();
    // Files that aren't `<review id>.jsonl` aren't reviews.
    for name in [
        "notes.txt",
        "r-NOTAULID.jsonl",
        "t-01j9z3k4m5n6p7q8r9s0t1v2w3.jsonl",
        "r-01j9z3k4m5n6p7q8r9s0t1v2w3.jsonl.bak",
    ] {
        backend
            .store_write_atomic(&format!("reviews/{name}"), b"{}")
            .unwrap();
    }
    std::fs::create_dir_all(log_file(&repo, &first.id).with_extension("d")).unwrap();

    let entries = store.list().unwrap();
    let mut expected: Vec<&ReviewId> = vec![&first.id, &second.id, &garbage, &empty, &headless];
    expected.sort();
    expected.reverse();
    let ids: Vec<&ReviewId> = entries.iter().map(ListEntry::id).collect();
    assert_eq!(ids, expected, "newest first");
    for entry in &entries {
        match entry {
            ListEntry::Ok(summary) => {
                assert!(summary.id == first.id || summary.id == second.id);
                if summary.id == second.id {
                    assert_eq!(summary.threads.open, 1);
                    assert_eq!(summary.warnings, 0);
                }
            }
            ListEntry::Err { id, error } => {
                assert!([&garbage, &empty, &headless].contains(&id), "{id}");
                assert!(
                    matches!(error, StoreError::Corrupt { .. }),
                    "{id}: {error:?}"
                );
                assert!(error.to_string().contains("unreadable"), "{error}");
            }
        }
    }
}

#[test]
fn a_summary_counts_warnings() {
    let (_repo, store) = fixture_store();
    let review = store.create(Target::Working, reviewer()).unwrap();
    store
        .backend()
        .store_append(&format!("reviews/{}.jsonl", review.id), b"{oops")
        .unwrap();
    store
        .append(&review.id, status(ReviewStatus::Approved))
        .unwrap();
    store
        .append(&review.id, status(ReviewStatus::Ready))
        .unwrap();
    let [ListEntry::Ok(summary)] = &store.list().unwrap()[..] else {
        panic!()
    };
    assert_eq!(summary.status, ReviewStatus::Approved);
    assert_eq!(summary.warnings, 2, "a malformed line and a refused status");
}

#[test]
fn events_from_a_newer_dv_are_skipped() {
    let (_repo, store) = fixture_store();
    let review = store.create(Target::Working, reviewer()).unwrap();
    let rel = format!("reviews/{}.jsonl", review.id);
    store
        .backend()
        .store_append(
            &rel,
            br#"{"v":2,"eid":"e-01j9z3k4m5n6p7q8r9s0t1v2w3","at":1,"kind":"status","status":"closed"}"#,
        )
        .unwrap();
    store
        .backend()
        .store_append(
            &rel,
            br#"{"v":1,"eid":"e-01j9z3k4m5n6p7q8r9s0t1v2w4","at":2,"kind":"emoji_added","emoji":"+1"}"#,
        )
        .unwrap();
    store
        .append(&review.id, status(ReviewStatus::Ready))
        .unwrap();
    let loaded = store.load(&review.id).unwrap();
    assert_eq!(loaded.newer_events, 2);
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(loaded.review.status, ReviewStatus::Ready);
}

#[test]
fn an_append_after_a_torn_line_survives() {
    let (repo, store) = fixture_store();
    let review = store.create(Target::Working, reviewer()).unwrap();
    // A crash cut the last write short: no newline at the end of the log.
    let torn = Event::new(status(ReviewStatus::Closed)).to_line().unwrap();
    let file = log_file(&repo, &review.id);
    let mut log = std::fs::read(&file).unwrap();
    log.extend(&torn[..20]);
    std::fs::write(&file, &log).unwrap();

    let thread = ThreadId::generate();
    store
        .append(&review.id, thread_added(&thread, reviewer()))
        .unwrap();
    let loaded = store.load(&review.id).unwrap();
    assert_eq!(loaded.review.threads.len(), 1, "the new event is intact");
    assert_eq!(loaded.review.status, ReviewStatus::Open);
    let [LogWarning::Malformed { line: 2, .. }] = &loaded.warnings[..] else {
        panic!("{:?}", loaded.warnings);
    };
    // The torn line was ended, not extended.
    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(text.lines().count(), 3);
    assert!(text.ends_with('\n'));
}

#[test]
fn the_snapshot_cache_answers_only_for_the_log_it_was_folded_from() {
    let (repo, store) = fixture_store();
    let review = store.create(Target::Working, reviewer()).unwrap();
    let id = &review.id;
    store.append(id, status(ReviewStatus::Ready)).unwrap();

    // Miss: no cache yet; the fold writes one covering both lines.
    let first = store.load(id).unwrap();
    assert!(!first.from_cache);
    let cache = snapshot(&store, id);
    assert_eq!(cache["stamp"]["lines"], 2);
    assert_eq!(
        cache["stamp"]["len"],
        std::fs::metadata(log_file(&repo, id)).unwrap().len()
    );

    // Hit: same log, so the cache answers. Doctor it to prove the state comes from it.
    let hit = store.load(id).unwrap();
    assert!(hit.from_cache);
    assert_eq!(hit.review, first.review);
    let mut doctored = cache.clone();
    doctored["review"]["status"] = "closed".into();
    store
        .backend()
        .store_write_atomic(&snapshot_rel(id), doctored.to_string().as_bytes())
        .unwrap();
    let from_cache = store.load(id).unwrap();
    assert!(from_cache.from_cache);
    assert_eq!(from_cache.review.status, ReviewStatus::Closed);

    // The log grew: the cache is stale, the log is folded again and the cache rewritten.
    store
        .append(id, status(ReviewStatus::ChangesRequested))
        .unwrap();
    let rebuilt = store.load(id).unwrap();
    assert!(!rebuilt.from_cache);
    assert_eq!(rebuilt.review.status, ReviewStatus::ChangesRequested);
    assert_eq!(snapshot(&store, id)["stamp"]["lines"], 3);
    assert!(store.load(id).unwrap().from_cache);

    // Same line count and length, different bytes (an edit in place): the hash catches it.
    let file = log_file(&repo, id);
    let text = std::fs::read_to_string(&file).unwrap();
    let edited = text.replace("changes_requested", "changes_requesteX");
    assert_eq!(edited.len(), text.len());
    std::fs::write(&file, edited).unwrap();
    let after_edit = store.load(id).unwrap();
    assert!(!after_edit.from_cache);
    assert_eq!(after_edit.review.status, ReviewStatus::Ready);
    assert_eq!(after_edit.warnings.len(), 1, "{:?}", after_edit.warnings);
    assert!(
        store.load(id).unwrap().from_cache,
        "warnings are cached too"
    );
    assert_eq!(store.load(id).unwrap().warnings, after_edit.warnings);

    // An unreadable cache, or one of another format, is rebuilt.
    for bad in [&b"not json"[..], br#"{"v":99}"#] {
        store
            .backend()
            .store_write_atomic(&snapshot_rel(id), bad)
            .unwrap();
        let loaded = store.load(id).unwrap();
        assert!(!loaded.from_cache);
        assert_eq!(loaded.review, after_edit.review);
        assert!(store.load(id).unwrap().from_cache);
    }

    // A cache can't stand in for another review's log.
    let other = store.create(Target::Staged, reviewer()).unwrap();
    store.load(&other.id).unwrap();
    let foreign = store
        .backend()
        .store_read(&snapshot_rel(id))
        .unwrap()
        .unwrap();
    let mut foreign: serde_json::Value = serde_json::from_slice(&foreign).unwrap();
    foreign["stamp"] = snapshot(&store, &other.id)["stamp"].clone();
    store
        .backend()
        .store_write_atomic(&snapshot_rel(&other.id), foreign.to_string().as_bytes())
        .unwrap();
    let loaded = store.load(&other.id).unwrap();
    assert!(!loaded.from_cache);
    assert_eq!(loaded.review.target, Target::Staged);
}

#[test]
fn delete_removes_the_log_the_cache_and_the_reviews_refs() {
    let (repo, store) = fixture_store();
    let doomed = store.create(Target::Working, reviewer()).unwrap();
    let kept = store.create(Target::Staged, reviewer()).unwrap();
    store.load(&doomed.id).unwrap();
    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    for name in [
        format!("refs/dv/reviews/{}/1", doomed.id),
        format!("refs/dv/reviews/{}/2", doomed.id),
        format!("refs/dv/reviews/{}/1", kept.id),
    ] {
        repo.git(&["update-ref", &name, &head]);
    }
    // One of them packed: deletion must reach packed refs too.
    repo.git(&["pack-refs", "--all"]);
    repo.git(&[
        "update-ref",
        &format!("refs/dv/reviews/{}/3", doomed.id),
        &head,
    ]);

    assert!(store.delete(&doomed.id).unwrap());
    assert!(!log_file(&repo, &doomed.id).exists());
    assert_eq!(
        store
            .backend()
            .store_read(&snapshot_rel(&doomed.id))
            .unwrap(),
        None
    );
    let refs = repo.git(&["for-each-ref", "--format=%(refname)", "refs/dv/"]);
    assert_eq!(refs, format!("refs/dv/reviews/{}/1\n", kept.id));
    let ids: Vec<ReviewId> = store
        .list()
        .unwrap()
        .iter()
        .map(|entry| entry.id().clone())
        .collect();
    assert_eq!(ids, std::slice::from_ref(&kept.id));
    assert!(matches!(
        store.load(&doomed.id),
        Err(StoreError::UnknownReview { .. })
    ));
    // Deleting again is harmless, and still clears refs a crashed delete left behind.
    repo.git(&[
        "update-ref",
        &format!("refs/dv/reviews/{}/9", doomed.id),
        &head,
    ]);
    assert!(!store.delete(&doomed.id).unwrap());
    let refs = repo.git(&["for-each-ref", "--format=%(refname)", "refs/dv/"]);
    assert_eq!(refs, format!("refs/dv/reviews/{}/1\n", kept.id));
}

#[test]
fn a_linked_worktrees_reviews_land_in_the_common_dir() {
    let (repo, main) = fixture_store();
    let worktree = repo.add_worktree("wt", "feat");
    let linked = ReviewStore::new(Arc::new(open_at(&repo, &worktree)));
    assert_ne!(linked.worktree(), main.worktree());

    let review = linked.create(Target::Working, agent()).unwrap();
    assert_eq!(review.worktree, linked.worktree());
    assert!(log_file(&repo, &review.id).is_file());
    assert!(
        !worktree.join(".git").is_dir(),
        "a linked worktree's .git is a file"
    );
    let [ListEntry::Ok(summary)] = &main.list().unwrap()[..] else {
        panic!()
    };
    assert_eq!(summary.id, review.id);
    assert_eq!(summary.worktree, linked.worktree());
    // Either worktree's store reads and writes the same log.
    main.append(&review.id, status(ReviewStatus::Ready))
        .unwrap();
    assert_eq!(
        linked.load(&review.id).unwrap().review.status,
        ReviewStatus::Ready
    );
}

#[test]
fn a_cached_review_is_the_folded_one() {
    use crate::review::ops::{add_thread, close_round, mark_viewed, record_submission, reply};
    let (_repo, store) = fixture_store();
    let target = Target::Pr {
        owner: "kylekz".to_string(),
        repo: "difftest".to_string(),
        number: 5,
        base_oid: oid('a'),
        head_oid: oid('b'),
    };
    let id = store.create(target, reviewer()).unwrap().id;
    let thread = add_thread(&store, &id, anchor("src/lib.rs", 2), "Nit", reviewer()).unwrap();
    reply(&store, &id, &thread, "Ok", Author::github("octocat")).unwrap_err();
    mark_viewed(&store, &id, "src/lib.rs", true).unwrap();
    close_round(&store, &id, oid('a'), oid('b'), Some(Verdict::Comment)).unwrap();
    mark_viewed(&store, &id, "src/main.rs", true).unwrap();
    store
        .backend()
        .store_append(&format!("reviews/{id}.jsonl"), b"{torn")
        .unwrap();
    record_submission(&store, &id, 42, "https://x.invalid/42", Verdict::Comment).unwrap();

    let folded = store.load(&id).unwrap();
    assert!(!folded.from_cache);
    let cached = store.load(&id).unwrap();
    assert!(cached.from_cache);
    assert_eq!(
        cached,
        LoadedReview {
            from_cache: true,
            ..folded.clone()
        }
    );
    let review = &cached.review;
    assert_eq!(review.status, ReviewStatus::Submitted);
    assert!(review.is_viewed(1, "src/lib.rs") && review.is_viewed(2, "src/main.rs"));
    assert_eq!(review.rounds.len(), 1);
    assert_eq!(review.threads.len(), 1);
    assert_eq!(cached.warnings.len(), 1);
}

/// A backend that deletes a review through another store right after the first read of one
/// store file: another process deleting the review at that moment.
struct DeletesOnRead {
    inner: LocalBackend,
    trigger: String,
    victim: Mutex<Option<(ReviewId, ReviewStore)>>,
}

impl DeletesOnRead {
    fn new(repo: &FixtureRepo, trigger: String, victim: &ReviewId) -> Self {
        let deleter = ReviewStore::new(Arc::new(open(repo)));
        Self {
            inner: open(repo),
            trigger,
            victim: Mutex::new(Some((victim.clone(), deleter))),
        }
    }
}

impl Backend for DeletesOnRead {
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
    fn store_append(&self, rel: &str, line: &[u8]) -> Result<(), BackendError> {
        self.inner.store_append(rel, line)
    }
    fn store_read(&self, rel: &str) -> Result<Option<Vec<u8>>, BackendError> {
        let read = self.inner.store_read(rel);
        if rel == self.trigger
            && let Some((id, store)) = self.victim.lock().unwrap().take()
        {
            assert!(store.delete(&id).unwrap(), "{id} existed");
        }
        read
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
    fn watch(&self, spec: WatchSpec, sink: WatchSink) -> Result<WatchHandle, BackendError> {
        self.inner.watch(spec, sink)
    }
}

fn log_rel(id: &ReviewId) -> String {
    format!("reviews/{id}.jsonl")
}

#[test]
fn a_cache_written_back_by_a_load_racing_a_delete_is_swept_by_list() {
    let (repo, store) = fixture_store();
    let review = store.create(Target::Working, reviewer()).unwrap();
    let racing = ReviewStore::new(Arc::new(DeletesOnRead::new(
        &repo,
        log_rel(&review.id),
        &review.id,
    )));
    // The load read the log just before the delete, so its fold still answers, and it writes
    // the cache after the delete removed it, with no log left to list.
    assert_eq!(racing.load(&review.id).unwrap().review.id, review.id);
    let cache = || {
        store
            .backend()
            .store_read(&snapshot_rel(&review.id))
            .unwrap()
    };
    assert!(cache().is_some(), "the race left a cache behind");
    assert!(store.list().unwrap().is_empty());
    assert_eq!(cache(), None, "list sweeps a cache whose log is gone");
}

#[test]
fn a_review_deleted_while_the_list_loads_is_left_out() {
    let (repo, store) = fixture_store();
    let older = store.create(Target::Working, reviewer()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let newer = store.create(Target::Staged, reviewer()).unwrap();
    // `list` loads the older review first (ids sort by creation time); reading its log deletes
    // the newer one, whose log `list` has already seen.
    let racing = ReviewStore::new(Arc::new(DeletesOnRead::new(
        &repo,
        log_rel(&older.id),
        &newer.id,
    )));
    let ids: Vec<ReviewId> = racing
        .list()
        .unwrap()
        .into_iter()
        .map(|entry| match entry {
            ListEntry::Ok(summary) => summary.id,
            ListEntry::Err { id, error } => panic!("{id} listed as broken: {error}"),
        })
        .collect();
    assert_eq!(ids, [older.id]);
}
