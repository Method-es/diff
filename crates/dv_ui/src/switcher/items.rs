//! What the switcher lists, as plain data built from the reviews service's rows ([`build`]):
//! a header per repository, then the rows that can be picked.
//!
//! A repository that opens offers its targets — uncommitted changes, staged changes and, when a
//! branch other than the default one is checked out, that branch against the default — each
//! beside its open review when it has one (picking it resumes that review, PLAN §2.4), then its
//! other open reviews. A repository that can't be opened lists none and says why. "Open folder…"
//! closes the list, whatever the filter.
//!
//! The filter keeps the rows whose repository name, path and label hold every word typed,
//! ignoring case; a repository whose header matches but offers nothing still shows its header.

use dv_core::review::{ReviewId, ReviewStatus, ReviewSummary};
use dv_core::{Location, Target};

use crate::components::status_label;
use crate::services::reviews::{OpenRequest, RepoRow, ReviewRow, WorkingSummary, target_label};

/// The targets every repository offers (the source picker's menu too).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    Working,
    Staged,
    Branch,
}

impl TargetKind {
    /// `"working"`, `"staged"` or `"branch"`: row keys and automation state.
    pub fn key(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Staged => "staged",
            Self::Branch => "branch",
        }
    }
}

/// An open review as a row shows it: its status, round and open threads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewBadge {
    pub id: ReviewId,
    pub status: ReviewStatus,
    pub round: u32,
    pub open_threads: u32,
}

impl ReviewBadge {
    fn of(summary: &ReviewSummary) -> Self {
        Self {
            id: summary.id.clone(),
            status: summary.status,
            round: summary.current_round,
            open_threads: summary.threads.open,
        }
    }
}

/// One of a repository's targets, as the switcher and the source picker offer it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetOffer {
    pub kind: TargetKind,
    pub target: Target,
    /// "Uncommitted changes", "Staged changes", "Branch feat vs main".
    pub label: String,
    /// Its open review in this worktree, if any.
    pub review: Option<ReviewBadge>,
}

/// The targets `row`'s repository offers: none when it can't be opened or read
/// ([`RepoRow::is_readable`]), since a review there could only fail, or write a store into a
/// folder that is no longer a repository.
pub fn target_offers(row: &RepoRow) -> Vec<TargetOffer> {
    if !row.is_readable() {
        return Vec::new();
    }
    let mut offers = vec![
        (
            TargetKind::Working,
            Target::Working,
            "Uncommitted changes".to_string(),
        ),
        (
            TargetKind::Staged,
            Target::Staged,
            "Staged changes".to_string(),
        ),
    ];
    if let Some(target) = row.labels.branch_target() {
        let label = format!("Branch {}", target_label(&target, &row.labels));
        offers.push((TargetKind::Branch, target, label));
    }
    offers
        .into_iter()
        .map(|(kind, target, label)| TargetOffer {
            review: row.open_review_of(&target).map(ReviewBadge::of),
            kind,
            target,
            label,
        })
        .collect()
}

/// What a pickable row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemKind {
    /// Review one of the repository's targets.
    Target(TargetOffer),
    /// Resume an open review of another target.
    Review { label: String, badge: ReviewBadge },
    /// The folder picker, to add a repository.
    OpenFolder,
}

/// A pickable row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// A stable name for the row: `<repo>/working`, `<repo>/staged`, `<repo>/branch`,
    /// `<repo>/review/<id>` or `open_folder`, where `<repo>` is the repository's name (its full
    /// path when two known repositories share a name).
    pub key: String,
    /// Its repository, an index into [`Listing::repos`].
    pub repo: Option<usize>,
    pub kind: ItemKind,
}

impl Item {
    /// What the row reads.
    pub fn label(&self) -> &str {
        match &self.kind {
            ItemKind::Target(offer) => &offer.label,
            ItemKind::Review { label, .. } => label,
            ItemKind::OpenFolder => "Open folder…",
        }
    }

    /// The open review the row stands for or resumes.
    pub fn review(&self) -> Option<&ReviewBadge> {
        match &self.kind {
            ItemKind::Target(offer) => offer.review.as_ref(),
            ItemKind::Review { badge, .. } => Some(badge),
            ItemKind::OpenFolder => None,
        }
    }

    /// `"target"`, `"review"` or `"open_folder"`, for automation state.
    pub fn kind_key(&self) -> &'static str {
        match self.kind {
            ItemKind::Target(_) => "target",
            ItemKind::Review { .. } => "review",
            ItemKind::OpenFolder => "open_folder",
        }
    }
}

/// The second line of a repository's header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoSummary {
    /// Nothing computed yet.
    Loading,
    /// It can't be opened; the few words why.
    Unreachable(String),
    /// Part of the row couldn't be read.
    Error(String),
    Clean,
    Changes(WorkingSummary),
}

/// A repository's header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoHeader {
    pub location: Location,
    /// The name rows are keyed by (see [`Item::key`]).
    pub key: String,
    pub name: String,
    pub path: String,
    /// The distro of a WSL repository, for its location chip.
    pub distro: Option<String>,
    pub summary: RepoSummary,
}

/// A line of the list, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Repo(usize),
    Item(usize),
}

/// What the switcher shows for a filter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub repos: Vec<RepoHeader>,
    /// The pickable rows, in order, "Open folder…" last.
    pub items: Vec<Item>,
    /// The scrolling list's lines (headers and rows, "Open folder…" left out: it sits in the
    /// footer).
    pub entries: Vec<Entry>,
}

impl Listing {
    /// Where `key`'s row is.
    pub fn position(&self, key: &str) -> Option<usize> {
        self.items.iter().position(|item| item.key == key)
    }

    /// The line `item` is on.
    pub fn entry_of(&self, item: usize) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| *entry == Entry::Item(item))
    }

    /// What picking `item` opens, with its repository; `None` for "Open folder…".
    pub fn request(&self, item: usize) -> Option<(Location, OpenRequest)> {
        let item = self.items.get(item)?;
        let location = self.repos.get(item.repo?)?.location.clone();
        let request = match &item.kind {
            ItemKind::Target(offer) => OpenRequest::Target(offer.target.clone()),
            ItemKind::Review { badge, .. } => OpenRequest::Review(badge.id.clone()),
            ItemKind::OpenFolder => return None,
        };
        Some((location, request))
    }

    /// The first row of the repository at `location`.
    pub fn first_of(&self, location: &Location) -> Option<usize> {
        let repo = self
            .repos
            .iter()
            .position(|repo| &repo.location == location)?;
        self.items.iter().position(|item| item.repo == Some(repo))
    }

    /// The row that stands for the open review `id`.
    pub fn review_row(&self, id: &ReviewId) -> Option<usize> {
        self.items
            .iter()
            .position(|item| item.review().is_some_and(|badge| &badge.id == id))
    }
}

/// The `key` of every row of `rows`: their names, or their paths where names repeat.
fn repo_keys(rows: &[&RepoRow]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            let shared = rows.iter().filter(|other| other.name == row.name).count() > 1;
            if shared {
                row.path.clone()
            } else {
                row.name.clone()
            }
        })
        .collect()
}

fn summary(row: &RepoRow) -> RepoSummary {
    if let Some(reason) = &row.unreachable {
        return RepoSummary::Unreachable(reason.clone());
    }
    match (row.working, &row.error) {
        (Some(working), _) if working.files == 0 => RepoSummary::Clean,
        (Some(working), _) => RepoSummary::Changes(working),
        (None, Some(error)) => RepoSummary::Error(error.message.clone()),
        (None, None) => RepoSummary::Loading,
    }
}

/// Whether every word of `query` is in `haystack` (already lowercase).
fn matches(haystack: &str, words: &[String]) -> bool {
    words.iter().all(|word| haystack.contains(word.as_str()))
}

/// The listing for `rows` (most recently used first) under `query`.
pub fn build(rows: &[&RepoRow], query: &str) -> Listing {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let keys = repo_keys(rows);
    let mut listing = Listing::default();
    for (row, key) in rows.iter().zip(keys) {
        let distro = match &row.location {
            Location::Wsl { distro, .. } => Some(distro.clone()),
            Location::Local(_) => None,
        };
        let repo_text = format!("{} {}", row.name, row.path).to_lowercase();
        let mut items: Vec<Item> = target_offers(row)
            .into_iter()
            .map(|offer| Item {
                key: format!("{key}/{}", offer.kind.key()),
                repo: None,
                kind: ItemKind::Target(offer),
            })
            .collect();
        let offered: Vec<ReviewId> = items
            .iter()
            .filter_map(|item| item.review().map(|badge| badge.id.clone()))
            .collect();
        // A repository that can't be read offers no rows at all (DESIGN §4.1).
        let reviews = if row.is_readable() {
            row.reviews.as_slice()
        } else {
            &[]
        };
        for review in reviews {
            let ReviewRow::Ok { summary, label } = review else {
                continue;
            };
            if !summary.status.is_open() || offered.contains(&summary.id) {
                continue;
            }
            items.push(Item {
                key: format!("{key}/review/{}", summary.id),
                repo: None,
                kind: ItemKind::Review {
                    label: label.clone(),
                    badge: ReviewBadge::of(summary),
                },
            });
        }
        items.retain(|item| {
            let status = item
                .review()
                .map(|badge| status_label(badge.status))
                .unwrap_or_default();
            let text = format!("{repo_text} {} {status}", item.label()).to_lowercase();
            matches(&text, &words)
        });
        if items.is_empty() && !matches(&repo_text, &words) {
            continue;
        }
        let repo = listing.repos.len();
        listing.repos.push(RepoHeader {
            location: row.location.clone(),
            key,
            name: row.name.clone(),
            path: row.path.clone(),
            distro,
            summary: summary(row),
        });
        listing.entries.push(Entry::Repo(repo));
        for mut item in items {
            item.repo = Some(repo);
            listing.entries.push(Entry::Item(listing.items.len()));
            listing.items.push(item);
        }
    }
    listing.items.push(Item {
        key: "open_folder".to_string(),
        repo: None,
        kind: ItemKind::OpenFolder,
    });
    listing
}
