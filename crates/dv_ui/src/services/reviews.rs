//! The reviews dv knows about (PLAN §11 `Reviews`): the known repositories
//! ([`known_repos`], persisted) and, per repository, what the switcher lists — its name and
//! location, its open and finished reviews (`ReviewStore::list`, newest first; an unreadable
//! log is a row with its error, PLAN §7), and a summary of its uncommitted changes
//! ("uncommitted changes · 7 files", from the `working` target's file list).
//!
//! A repository's row is computed in the background ([`Reviews::refresh`]): when it is added,
//! and whenever the switcher opens ([`Reviews::refresh_all`]). Nothing here watches a
//! repository: an open review's session does that for its own repository (PLAN §11), and a
//! WSL repository is never touched beyond what [`Locations`] reports, so listing never boots a
//! distro.
//!
//! [`Reviews::open_review`] opens or resumes a target's review (PLAN §2.4: at most one open
//! review per target) as the reviewer, records that the repository was used, and hands back
//! what a `ReviewSession` starts from.

pub mod known_repos;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use dv_core::backend::GitOpts;
use dv_core::diff::file_list;
use dv_core::review::{Author, ListEntry, LoadedReview, ReviewId, ReviewStore, ReviewSummary, ops};
use dv_core::{Backend, BackendError, GitRepo, Location, Target};
use futures::channel::oneshot;
use gpui::{App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, Task, WeakEntity};

use super::locations::Locations;
use super::persist::{JsonFile, ReadObject, read_object};
use crate::failure::Failure;
pub use known_repos::{KNOWN_REPOS_FILE, KnownRepo, KnownReposFile};

/// What [`Reviews`] tells its subscribers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewsEvent {
    /// The known repositories or a repository's row changed.
    Changed,
}

/// A repository's uncommitted changes: what the `working` target's file list holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkingSummary {
    pub files: usize,
    pub additions: u64,
    pub deletions: u64,
}

/// What a target's label needs to know about the repository: the branch checked out (or a
/// short commit id when `HEAD` is detached) and the default branch a `branch` target without a
/// base compares against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LabelContext {
    pub head: Option<String>,
    /// The branch checked out, `None` when `HEAD` is detached: what a `branch` target of the
    /// work in progress names (the switcher's and the source picker's "Branch" row).
    pub branch: Option<String>,
    pub default_base: Option<String>,
}

impl LabelContext {
    /// Reads them from `repo` (blocking). A part that can't be read stays `None`.
    pub fn read(repo: &GitRepo) -> Self {
        let branch = repo.current_branch().ok().flatten();
        let head = branch.clone().or_else(|| {
            repo.head()
                .ok()
                .flatten()
                .map(|oid| oid.as_str()[..7].to_string())
        });
        Self {
            head,
            branch,
            default_base: repo.default_branch().ok().flatten(),
        }
    }

    /// The `branch` target of the work checked out, compared with the default branch: `None`
    /// when `HEAD` is detached, nothing is known to compare with, or the branch is the default
    /// branch itself (nothing to compare).
    pub fn branch_target(&self) -> Option<Target> {
        let (branch, base) = (self.branch.as_ref()?, self.default_base.as_ref()?);
        (branch != base).then(|| Target::Branch {
            branch: branch.clone(),
            base: None,
        })
    }
}

/// A target as the title bar and the switcher name it (DESIGN §4.1: "working tree vs main"):
/// what is being reviewed and what it is compared against.
pub fn target_label(target: &Target, context: &LabelContext) -> String {
    let versus = |what: &str| match &context.head {
        Some(head) => format!("{what} vs {head}"),
        None => what.to_string(),
    };
    match target {
        Target::Working => versus("working tree"),
        Target::Staged => versus("staged"),
        Target::Branch { branch, base } => match base.as_ref().or(context.default_base.as_ref()) {
            Some(base) => format!("{branch} vs {base}"),
            None => branch.clone(),
        },
        Target::Commit { rev } => format!("commit {rev}"),
        Target::Range {
            from,
            to,
            three_dot,
        } => format!("{from}{}{to}", if *three_dot { "..." } else { ".." }),
        Target::Pr {
            owner,
            repo,
            number,
            ..
        } => format!("{owner}/{repo}#{number}"),
    }
}

/// What the stream's side labels call a target's two sides (DESIGN §4.1: `main` / `working
/// tree`): the old side, then the new.
pub fn side_labels(target: &Target, context: &LabelContext) -> (String, String) {
    let head = || context.head.clone().unwrap_or_else(|| "HEAD".to_string());
    match target {
        Target::Working => (head(), "working tree".to_string()),
        Target::Staged => (head(), "staged".to_string()),
        // The old side is the merge base with the base branch; the board names it by the branch.
        Target::Branch { branch, base } => (
            base.as_ref()
                .or(context.default_base.as_ref())
                .cloned()
                .unwrap_or_else(|| "merge base".to_string()),
            branch.clone(),
        ),
        Target::Commit { rev } => (format!("{rev}^"), rev.clone()),
        Target::Range { from, to, .. } => (from.clone(), to.clone()),
        Target::Pr { number, .. } => ("base".to_string(), format!("#{number}")),
    }
}

/// The reviewer's name for events they write: their git identity in that repository
/// (`user.name`), or `reviewer` when none is configured (PLAN §2.5). Blocking.
pub fn reviewer_identity(backend: &dyn Backend) -> Author {
    let name = backend
        .git(
            &["config", "--get", "user.name"],
            &GitOpts::default().with_ok_codes(&[0, 1]),
        )
        .ok()
        .and_then(|output| output.stdout_utf8().map(|name| name.trim().to_string()))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "reviewer".to_string());
    Author::reviewer(name)
}

/// One review in a repository's row.
#[derive(Clone, Debug, PartialEq)]
pub enum ReviewRow {
    Ok {
        summary: Box<ReviewSummary>,
        /// [`target_label`] of its target.
        label: String,
    },
    /// A log that didn't load (PLAN §7: one bad file never hides the others).
    Err { id: ReviewId, error: Failure },
}

impl ReviewRow {
    pub fn id(&self) -> &ReviewId {
        match self {
            Self::Ok { summary, .. } => &summary.id,
            Self::Err { id, .. } => id,
        }
    }
}

/// A known repository as the switcher lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct RepoRow {
    pub location: Location,
    /// The short name (`api-server`).
    pub name: String,
    /// The full location (`D:\code\api-server`, `Ubuntu:/home/kyle/api-server`).
    pub path: String,
    /// A refresh is running.
    pub loading: bool,
    /// Why the repository (or part of its row) couldn't be read, from the last refresh.
    pub error: Option<Failure>,
    /// Why the last refresh couldn't open the repository at all, in a few words ("WSL support
    /// arrives later", "no such directory: …"); `None` while it opens. Such a row lists no
    /// targets to review.
    pub unreachable: Option<String>,
    /// The worktree root as its reviews record it (`ReviewSummary::worktree`), once a refresh
    /// opened the repository: which `working` and `staged` reviews are this worktree's.
    pub toplevel: Option<String>,
    /// Its uncommitted changes; `None` until a refresh computed them, or when it couldn't.
    pub working: Option<WorkingSummary>,
    /// Its reviews, newest first.
    pub reviews: Vec<ReviewRow>,
    pub labels: LabelContext,
    /// When it was last used, for the order rows are listed in.
    pub last_used: u64,
}

impl RepoRow {
    /// The open review of `target` in this worktree, if the last refresh listed one (the rule
    /// `ops::open_review` resumes by).
    pub fn open_review_of(&self, target: &Target) -> Option<&ReviewSummary> {
        let toplevel = self.toplevel.as_deref()?;
        self.reviews.iter().find_map(|row| match row {
            ReviewRow::Ok { summary, .. }
                if summary.status.is_open() && ops::is_review_of(summary, target, toplevel) =>
            {
                Some(&**summary)
            }
            _ => None,
        })
    }

    /// Whether the last refresh could read the repository: it opened, and its changes were
    /// listed (or haven't been yet). One that can't be read offers no rows (DESIGN §4.1).
    pub fn is_readable(&self) -> bool {
        self.unreachable.is_none() && !(self.error.is_some() && self.working.is_none())
    }

    fn new(repo: &KnownRepo) -> Self {
        Self {
            location: repo.location.clone(),
            name: repo.location.short_name(),
            path: repo.location.display_name(),
            loading: false,
            error: None,
            unreachable: None,
            toplevel: None,
            working: None,
            reviews: Vec::new(),
            labels: LabelContext::default(),
            last_used: repo.last_used(),
        }
    }
}

/// What a background scan of a repository found.
struct Scan {
    working: Result<WorkingSummary, Failure>,
    reviews: Result<Vec<ReviewRow>, Failure>,
    labels: LabelContext,
    toplevel: String,
}

/// Why a refresh couldn't open a repository: the failure, and the few words a switcher row
/// shows for it.
struct Unopened {
    failure: Failure,
    reason: String,
}

impl Unopened {
    fn new(error: &BackendError) -> Self {
        let reason = match error {
            BackendError::Unavailable { reason, .. } => reason.clone(),
            other => other.to_string(),
        };
        Self {
            failure: Failure::from(error),
            reason,
        }
    }
}

/// What [`Reviews::open`] opens: a target's review (resumed when one is open) or a review by id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenRequest {
    Target(Target),
    Review(ReviewId),
}

/// A target's review, opened or resumed ([`Reviews::open_review`]).
pub struct OpenedReview {
    pub backend: Arc<dyn Backend>,
    pub loaded: LoadedReview,
    /// `false` when an open review of the target already existed.
    pub created: bool,
    /// Who the reviewer is in this repository.
    pub reviewer: Author,
    /// The repository's labels as the review opened, so its target is named in full
    /// ("working tree vs main") from the start rather than once its files are listed.
    pub labels: LabelContext,
}

/// The reviews service (see the module docs).
pub struct Reviews {
    locations: Entity<Locations>,
    known: KnownReposFile,
    file: Entity<JsonFile>,
    rows: HashMap<Location, RepoRow>,
    /// Refreshes running, by repository, with the generation each started at.
    refreshing: HashMap<Location, (u64, Task<()>)>,
    generation: u64,
    /// `add_repo` and `open_review` calls running.
    opening: usize,
}

impl EventEmitter<ReviewsEvent> for Reviews {}

impl Reviews {
    /// Reads `<data_dir>/known_repos.json` synchronously (it is small) and lists every known
    /// repository; their rows fill in as [`Reviews::refresh`] computes them.
    pub fn new(locations: Entity<Locations>, data_dir: &Path, cx: &mut Context<Self>) -> Self {
        let path = data_dir.join(KNOWN_REPOS_FILE);
        let (known, unsaveable) = match read_object(&path) {
            ReadObject::Missing => (KnownReposFile::default(), None),
            ReadObject::Object(object) => match KnownReposFile::from_object(object) {
                Ok((known, problems)) => {
                    for problem in problems {
                        log::warn!("{}: {problem}", path.display());
                    }
                    (known, None)
                }
                Err(problem) => {
                    log::warn!("{}: {problem}", path.display());
                    (
                        KnownReposFile::default(),
                        Some(format!(
                            "{} isn't a file dv can read ({problem}), so it's left as it is and \
                             changes aren't saved this session",
                            path.display()
                        )),
                    )
                }
            },
            ReadObject::Unreadable(reason) => {
                log::warn!("{reason}");
                (KnownReposFile::default(), Some(reason))
            }
        };
        let rows = known
            .repos()
            .map(|repo| (repo.location.clone(), RepoRow::new(repo)))
            .collect();
        let file = cx.new(|cx| JsonFile::new(path, unsaveable, cx));
        cx.observe(&file, |_, _, cx| cx.notify()).detach();
        Self {
            locations,
            known,
            file,
            rows,
            refreshing: HashMap::new(),
            generation: 0,
            opening: 0,
        }
    }

    /// The known repositories' rows, most recently used first.
    pub fn rows(&self) -> Vec<&RepoRow> {
        let mut rows: Vec<&RepoRow> = self.rows.values().collect();
        rows.sort_by(|a, b| b.last_used.cmp(&a.last_used).then(a.path.cmp(&b.path)));
        rows
    }

    pub fn row(&self, location: &Location) -> Option<&RepoRow> {
        self.rows.get(location)
    }

    pub fn known(&self) -> &KnownReposFile {
        &self.known
    }

    /// Why the known repositories can't be saved, or the last save failed.
    pub fn save_error<'a>(&self, cx: &'a App) -> Option<&'a str> {
        self.file.read(cx).last_error()
    }

    /// Nothing is refreshing, opening or waiting to be saved: automation's `settled`.
    pub fn is_settled(&self, cx: &App) -> bool {
        self.refreshing.is_empty() && self.opening == 0 && self.file.read(cx).is_settled()
    }

    /// Adds the repository at `location` (like `dv open <path>`): its worktree root becomes a
    /// known repository and its row is computed. A WSL location, which can't be opened yet, is
    /// added as given, so its row says why; any other location that doesn't open is refused.
    /// Resolves to the location as known.
    pub fn add_repo(
        &mut self,
        location: Location,
        cx: &mut Context<Self>,
    ) -> Task<Result<Location, Failure>> {
        self.opening += 1;
        let open = self
            .locations
            .update(cx, |locations, cx| locations.open(location.clone(), cx));
        detached(cx, async move |this, cx| {
            let opened = open.await;
            this.update(cx, |this, cx| {
                this.opening -= 1;
                cx.notify();
                let known = match opened {
                    Ok(backend) => backend.repo().location.clone(),
                    Err(error) if matches!(*error, BackendError::Unavailable { .. }) => location,
                    Err(error) => return Err(Failure::from(&*error)),
                };
                this.add_known(known.clone(), cx);
                this.refresh(known.clone(), cx);
                Ok(known)
            })
            .unwrap_or_else(|_| Err(closing()))
        })
    }

    /// Opens the review of `target` in the repository at `location`, or resumes its open one
    /// (`ops::open_review`), as the reviewer. The repository becomes known (and most recently
    /// used) and its row is refreshed.
    pub fn open_review(
        &mut self,
        location: Location,
        target: Target,
        cx: &mut Context<Self>,
    ) -> Task<Result<OpenedReview, Failure>> {
        self.open(location, OpenRequest::Target(target), cx)
    }

    /// [`Reviews::open_review`] for either kind of [`OpenRequest`]: a review asked for by id is
    /// loaded as it is (the switcher's rows for reviews of other targets).
    pub fn open(
        &mut self,
        location: Location,
        request: OpenRequest,
        cx: &mut Context<Self>,
    ) -> Task<Result<OpenedReview, Failure>> {
        self.opening += 1;
        let open = self
            .locations
            .update(cx, |locations, cx| locations.open(location, cx));
        detached(cx, async move |this, cx| {
            let result: Result<OpenedReview, Failure> = async {
                let backend = open.await.map_err(|error| Failure::from(&*error))?;
                let job = cx.background_spawn({
                    let backend = backend.clone();
                    async move { open_blocking(backend, request) }
                });
                let Opening {
                    loaded,
                    created,
                    reviewer,
                    labels,
                } = job.await?;
                Ok(OpenedReview {
                    backend,
                    loaded,
                    created,
                    reviewer,
                    labels,
                })
            }
            .await;
            this.update(cx, |this, cx| {
                this.opening -= 1;
                if let Ok(opened) = &result {
                    let known = opened.backend.repo().location.clone();
                    this.add_known(known.clone(), cx);
                    if this
                        .known
                        .mark_opened(&known, dv_core::review::event::now_ms())
                    {
                        this.save(cx);
                    }
                    if let Some(row) = this.rows.get_mut(&known) {
                        row.last_used = this.known.get(&known).map_or(0, KnownRepo::last_used);
                    }
                    this.refresh(known, cx);
                }
                cx.notify();
            })
            .ok();
            result
        })
    }

    /// Recomputes every known repository's row (the switcher opening).
    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        let locations: Vec<Location> = self.rows.keys().cloned().collect();
        for location in locations {
            self.refresh(location, cx);
        }
    }

    /// Recomputes `location`'s row in the background: its reviews, its uncommitted changes and
    /// what its target labels need. A refresh already running for it is superseded.
    pub fn refresh(&mut self, location: Location, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get_mut(&location) else {
            return;
        };
        row.loading = true;
        self.generation += 1;
        let generation = self.generation;
        let open = self
            .locations
            .update(cx, |locations, cx| locations.open(location.clone(), cx));
        let task = cx.spawn({
            let location = location.clone();
            async move |this, cx| {
                let scan = match open.await {
                    Ok(backend) => Ok(cx.background_spawn(async move { scan(backend) }).await),
                    Err(error) => Err(Unopened::new(&error)),
                };
                this.update(cx, |this, cx| {
                    this.finish_refresh(&location, generation, scan, cx)
                })
                .ok();
            }
        });
        self.refreshing.insert(location, (generation, task));
        cx.emit(ReviewsEvent::Changed);
        cx.notify();
    }

    fn finish_refresh(
        &mut self,
        location: &Location,
        generation: u64,
        scan: Result<Scan, Unopened>,
        cx: &mut Context<Self>,
    ) {
        if self
            .refreshing
            .get(location)
            .is_none_or(|(current, _)| *current != generation)
        {
            return;
        }
        self.refreshing.remove(location);
        let Some(row) = self.rows.get_mut(location) else {
            return;
        };
        row.loading = false;
        match scan {
            Ok(scan) => {
                row.labels = scan.labels;
                row.unreachable = None;
                row.toplevel = Some(scan.toplevel);
                row.error = scan.reviews.as_ref().err().cloned();
                row.reviews = scan.reviews.unwrap_or_default();
                row.working = match scan.working {
                    Ok(working) => Some(working),
                    Err(error) => {
                        row.error.get_or_insert(error);
                        None
                    }
                };
            }
            Err(Unopened { failure, reason }) => {
                row.error = Some(failure);
                row.unreachable = Some(reason);
                row.working = None;
                row.reviews.clear();
            }
        }
        cx.emit(ReviewsEvent::Changed);
        cx.notify();
    }

    fn add_known(&mut self, location: Location, cx: &mut Context<Self>) {
        let now = dv_core::review::event::now_ms();
        if self.known.add(location.clone(), now) {
            if let Some(repo) = self.known.get(&location) {
                self.rows.insert(location, RepoRow::new(repo));
            }
            self.save(cx);
            cx.emit(ReviewsEvent::Changed);
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let json = self.known.to_json();
        self.file.update(cx, |file, cx| file.save(json, cx));
    }
}

/// Runs `work` on the foreground as a detached task, so its bookkeeping (the `opening` count,
/// the known repos) happens even when the caller drops the returned task, which only carries
/// the result.
fn detached<T: Send + 'static>(
    cx: &mut Context<Reviews>,
    work: impl AsyncFnOnce(WeakEntity<Reviews>, &mut AsyncApp) -> Result<T, Failure> + 'static,
) -> Task<Result<T, Failure>> {
    let (sender, receiver) = oneshot::channel();
    cx.spawn(async move |this, cx| {
        let _ = sender.send(work(this, cx).await);
    })
    .detach();
    cx.background_spawn(async move { receiver.await.unwrap_or_else(|_| Err(closing())) })
}

fn closing() -> Failure {
    Failure::new("dv is closing")
}

/// What [`open_blocking`] read.
struct Opening {
    loaded: LoadedReview,
    created: bool,
    reviewer: Author,
    labels: LabelContext,
}

/// `ops::open_review` as the reviewer (or the review asked for by id), then the review's full
/// load (warnings included) and the repository's labels.
fn open_blocking(backend: Arc<dyn Backend>, request: OpenRequest) -> Result<Opening, Failure> {
    let reviewer = reviewer_identity(backend.as_ref());
    let labels = LabelContext::read(&GitRepo::new(backend.clone()));
    let store = ReviewStore::new(backend);
    let (id, created) = match request {
        OpenRequest::Target(target) => {
            let opened = ops::open_review(&store, target, reviewer.clone())
                .map_err(|e| Failure::from(&e))?;
            (opened.review.id, opened.created)
        }
        OpenRequest::Review(id) => (id, false),
    };
    let loaded = store.load(&id).map_err(|e| Failure::from(&e))?;
    Ok(Opening {
        loaded,
        created,
        reviewer,
        labels,
    })
}

/// Reads a repository's row (blocking): its reviews, its uncommitted changes and its labels.
fn scan(backend: Arc<dyn Backend>) -> Scan {
    let toplevel = backend.repo().toplevel.clone();
    let repo = GitRepo::new(backend.clone());
    let labels = LabelContext::read(&repo);
    let store = ReviewStore::new(backend);
    let reviews = store
        .list()
        .map(|entries| {
            entries
                .into_iter()
                .map(|entry| match entry {
                    ListEntry::Ok(summary) => ReviewRow::Ok {
                        label: target_label(&summary.target, &labels),
                        summary,
                    },
                    ListEntry::Err { id, error } => ReviewRow::Err {
                        id,
                        error: Failure::from(&error),
                    },
                })
                .collect()
        })
        .map_err(|error| Failure::from(&error));
    let working = Target::Working
        .resolve(&repo)
        .map_err(|error| Failure::from(&error))
        .and_then(|spec| file_list(&repo, &spec).map_err(|error| Failure::from(&error)))
        .map(|files| WorkingSummary {
            files: files.len(),
            additions: files.iter().map(|file| u64::from(file.additions)).sum(),
            deletions: files.iter().map(|file| u64::from(file.deletions)).sum(),
        });
    Scan {
        working,
        reviews,
        labels,
        toplevel,
    }
}
