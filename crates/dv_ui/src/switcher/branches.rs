//! The switcher's branch picker (DESIGN §4.1 "Compare branches…", PLAN §2.2): one
//! repository's local branches, then the bases the picked one can be compared with, each pair
//! opening or resuming its `branch` review. It is a mode of the switcher ([`Mode`]): the same
//! panel, filter field and keys, its rows listed from the repository's row
//! (`RepoRow::branches`, read with its reviews).
//!
//! **Branches.** Every local branch but the default one, the most recently committed first,
//! each beside the base it is offered against — its stack base (`dv_core::git::branches`),
//! else the default branch — and that pair's open review. Picking one lists its bases.
//!
//! **Bases.** The stack base first ("stack base"), then the default branch ("default
//! branch"), then every other local branch, the most recently committed first. Picking one
//! opens the branch against it; against the default branch that is the target with no base,
//! the switcher's own Branch row's and `dv review new --branch`'s, so it resumes their review.

use dv_core::{Location, Target};

use super::items::{
    Entry, Item, ItemKind, Listing, ReviewBadge, TargetKind, TargetOffer, header, matches,
    repo_keys,
};
use crate::services::reviews::RepoRow;

/// What the switcher lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// The known repositories, their targets and reviews.
    #[default]
    Repos,
    /// The branch picker's first step: the branches of the repository at `location`.
    Branches { location: Location },
    /// Its second step: the bases `branch` can be compared with.
    Bases { location: Location, branch: String },
}

impl Mode {
    /// `"repos"`, `"branches"` or `"bases"`, for automation state.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Repos => "repos",
            Self::Branches { .. } => "branches",
            Self::Bases { .. } => "bases",
        }
    }

    /// The filter field's placeholder.
    pub fn placeholder(&self) -> String {
        match self {
            Self::Repos => super::PLACEHOLDER.to_string(),
            Self::Branches { .. } => "Pick a branch to review".to_string(),
            Self::Bases { branch, .. } => format!("Compare {branch} with…"),
        }
    }
}

/// Whether `row` has a branch to compare: one other than the default branch.
pub fn has_branches(row: &RepoRow) -> bool {
    row.branches
        .iter()
        .any(|branch| Some(branch.name.as_str()) != row.labels.default_base.as_deref())
}

/// The target of `branch` against `base`: with no base when `base` is the default branch.
pub fn branch_target(branch: &str, base: &str, default_base: Option<&str>) -> Target {
    Target::Branch {
        branch: branch.to_string(),
        base: (Some(base) != default_base).then(|| base.to_string()),
    }
}

/// The base `name` is offered against in the first step: its stack base, else the default
/// branch.
fn offered_base(row: &RepoRow, name: &str) -> Option<String> {
    row.branches
        .iter()
        .find(|branch| branch.name == name)
        .and_then(|branch| branch.stack_base.clone())
        .or_else(|| row.labels.default_base.clone())
}

/// The row of `location` among `rows`, with its key.
fn find<'a>(rows: &[&'a RepoRow], location: &Location) -> Option<(&'a RepoRow, String)> {
    let keys = repo_keys(rows);
    rows.iter()
        .zip(keys)
        .find(|(row, _)| &row.location == location)
        .map(|(row, key)| (*row, key))
}

/// A listing of one repository's `items`, under its header, those whose text holds every word
/// of `query` kept.
fn listing(row: &RepoRow, key: String, items: Vec<Item>, query: &str) -> Listing {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut listing = Listing {
        repos: vec![header(row, key)],
        entries: vec![Entry::Repo(0)],
        items: Vec::new(),
    };
    for mut item in items {
        let text = format!("{} {}", item.label(), item.detail().unwrap_or_default()).to_lowercase();
        if !matches(&text, &words) {
            continue;
        }
        item.repo = Some(0);
        listing.entries.push(Entry::Item(listing.items.len()));
        listing.items.push(item);
    }
    listing
}

/// The first step's listing for the repository at `location` (module docs, "Branches"); empty
/// when it isn't among `rows` or can't be read.
pub fn build_branches(rows: &[&RepoRow], location: &Location, query: &str) -> Listing {
    let Some((row, key)) = find(rows, location) else {
        return Listing::default();
    };
    if !row.is_readable() {
        return listing(row, key, Vec::new(), query);
    }
    let default_base = row.labels.default_base.as_deref();
    let items = row
        .branches
        .iter()
        .filter(|branch| Some(branch.name.as_str()) != default_base)
        .map(|branch| {
            let base = offered_base(row, &branch.name);
            let review = base.as_deref().and_then(|base| {
                row.open_review_of(&branch_target(&branch.name, base, default_base))
                    .map(ReviewBadge::of)
            });
            Item {
                key: format!("{key}/pick/{}", branch.name),
                repo: None,
                kind: ItemKind::Branch {
                    name: branch.name.clone(),
                    base,
                    review,
                },
            }
        })
        .collect();
    listing(row, key, items, query)
}

/// The second step's listing: the bases `branch` of the repository at `location` can be
/// compared with (module docs, "Bases").
pub fn build_bases(rows: &[&RepoRow], location: &Location, branch: &str, query: &str) -> Listing {
    let Some((row, key)) = find(rows, location) else {
        return Listing::default();
    };
    if !row.is_readable() {
        return listing(row, key, Vec::new(), query);
    }
    let default_base = row.labels.default_base.as_deref();
    let stack_base = row
        .branches
        .iter()
        .find(|candidate| candidate.name == branch)
        .and_then(|candidate| candidate.stack_base.as_deref());
    let mut bases: Vec<(&str, Option<&'static str>)> = Vec::new();
    if let Some(stack_base) = stack_base {
        bases.push((stack_base, Some("stack base")));
    }
    if let Some(default_base) = default_base
        && Some(default_base) != stack_base
    {
        bases.push((default_base, Some("default branch")));
    }
    for candidate in &row.branches {
        let name = candidate.name.as_str();
        if name != branch && bases.iter().all(|(listed, _)| *listed != name) {
            bases.push((name, None));
        }
    }
    let items = bases
        .into_iter()
        .map(|(base, note)| {
            let target = branch_target(branch, base, default_base);
            Item {
                key: format!("{key}/base/{base}"),
                repo: None,
                kind: ItemKind::Base {
                    offer: TargetOffer {
                        kind: TargetKind::Branch,
                        review: row.open_review_of(&target).map(ReviewBadge::of),
                        target,
                        label: base.to_string(),
                    },
                    note,
                },
            }
        })
        .collect();
    listing(row, key, items, query)
}
