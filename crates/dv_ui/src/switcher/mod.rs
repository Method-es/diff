//! The switcher (PLAN §11 `switcher`, DESIGN §4.1; `Ctrl O`): every known repository with its
//! uncommitted changes, the targets it offers and its open reviews, in an overlay hung under the
//! title bar's switcher button over the scrim (DESIGN §3). Picking a row opens or resumes that
//! review; "Open folder…" adds a repository through the folder picker (`crate::system`); a
//! stopped WSL distro's **Start** row starts it, and a failed connection's **Retry** row
//! connects again, the switcher staying open while it lists the repository, which then has
//! the selection. Meanwhile the Start or Retry row stays, inert, with the selection: picking
//! it again does nothing. A repository's header has a remove button (`Shift Delete` removes the
//! selected row's): it leaves the list, its reviews staying in the repository, and the
//! selection moves to the repository that took its place.
//!
//! A filter field has the keyboard while it is open: typing filters ([`items::build`]), `↑`/`↓`
//! move the selection (wrapping), `Enter` picks, `Shift Delete` removes, `Esc` closes. The switcher only reports what
//! was picked ([`SwitcherEvent`]); the shell opens it. Rows refresh as the reviews service
//! recomputes them (it does when the switcher opens, and when a connection a row goes through
//! starts reconnecting, stops or fails, or comes back), keeping the selected row.
//!
//! A repository's "Compare branches…" row turns the switcher into the branch picker
//! ([`branches`]): its branches, then the bases the picked one can be compared with, each pair
//! opening or resuming its `branch` review. There `Esc` steps back (bases → branches → the
//! repositories, when the picker was entered from them) before it closes.
//!
//! [`source`] is the title bar's other menu, the current repository's targets.
//!
//! Automation targets: `switcher`, `switcher/filter`, `switcher/repo/<repo>` and its
//! `switcher/repo/<repo>/remove`, `switcher/<key>` for each row ([`items::Item::key`]).

pub mod branches;
pub mod items;
mod render;
pub mod source;

#[cfg(test)]
mod tests;

use dv_core::Location;
use dv_core::review::ReviewId;
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, Focusable as _, ScrollHandle, Subscription,
    Window,
};
use gpui_component::input::{InputEvent, InputState};

use crate::actions::switcher::{Confirm, Dismiss, RemoveRepo, SelectNext, SelectPrevious};
use crate::services::reviews::{OpenRequest, Reviews};
use branches::Mode;
use items::{ItemKind, Listing};

/// The panel's width (one-off, like the board's sheets: wide enough for a path and a review).
pub const SWITCHER_WIDTH: gpui::Pixels = gpui::px(560.);
/// The filter field's placeholder.
pub const PLACEHOLDER: &str = "Filter repositories and reviews";

/// What was picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitcherEvent {
    /// Open or resume this review.
    Open {
        location: Location,
        request: OpenRequest,
    },
    /// "Open folder…".
    OpenFolder,
    /// **Start** on the repository at `location`, whose WSL distro isn't running, or **Retry**
    /// on one whose connection failed: both connect explicitly (`Reviews::start`).
    Start { location: Location },
    /// Remove the repository at `location` from the list (`Reviews::remove_repo`).
    Remove { location: Location },
    /// Closed without picking (`Esc`).
    Dismiss,
}

/// The switcher (see the module docs).
pub struct Switcher {
    reviews: Entity<Reviews>,
    input: Entity<InputState>,
    query: String,
    mode: Mode,
    /// Whether `Esc` in the branch picker's first step returns to the repositories: it was
    /// entered from their "Compare branches…" row, not opened on it (the source picker's).
    back_to_repos: bool,
    listing: Listing,
    /// Into `listing.items`.
    selected: usize,
    /// The repository a **Start** or **Retry** was picked on: its first row takes the
    /// selection once it lists one.
    started: Option<Location>,
    /// The repository that takes a removed one's place: its first row takes the selection
    /// once the removed one has left the list.
    after_remove: Option<Location>,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SwitcherEvent> for Switcher {}

impl Switcher {
    /// A switcher over the known repositories, its selection on `focus`'s first row (the
    /// repository `dv open` named), else on the open review `current`, else on the first row.
    pub fn new(
        reviews: Entity<Reviews>,
        focus: Option<&Location>,
        current: Option<&ReviewId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(PLACEHOLDER));
        let subscriptions = vec![
            cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query_changed(cx);
                }
            }),
            // The reviews service lists a row again when its connection goes down or comes
            // back, whether or not the switcher is open.
            cx.observe(&reviews, |this, _, cx| this.rebuild(cx)),
        ];
        let mut this = Self {
            reviews,
            input,
            query: String::new(),
            mode: Mode::Repos,
            back_to_repos: false,
            listing: Listing::default(),
            selected: 0,
            started: None,
            after_remove: None,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        this.listing = this.build(cx);
        this.selected = focus
            .and_then(|location| this.listing.first_of(location))
            .or_else(|| current.and_then(|id| this.listing.review_row(id)))
            .unwrap_or(0);
        this.scroll_to_selected();
        this
    }

    /// Puts the keyboard in the filter field.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Whether the filter field has the keyboard.
    pub fn is_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.input.read(cx).focus_handle(cx).is_focused(window)
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn listing(&self) -> &Listing {
        &self.listing
    }

    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    /// Opens the branch picker on the repository at `location` (the source picker's "Compare
    /// branches…"): `Esc` in its first step then closes the switcher.
    pub fn compare_branches(
        &mut self,
        location: Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.back_to_repos = false;
        self.set_mode(Mode::Branches { location }, None, window, cx);
    }

    /// Shows `mode`'s rows with the filter cleared, the selection on `select` (a row's key)
    /// when it is listed, else on the first row.
    fn set_mode(
        &mut self,
        mode: Mode,
        select: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mode = mode;
        self.query.clear();
        let placeholder = self.mode.placeholder();
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(placeholder, window, cx);
        });
        self.listing = self.build(cx);
        self.selected = select
            .and_then(|key| self.listing.position(&key))
            .unwrap_or(0);
        self.scroll_to_selected();
        cx.notify();
    }

    /// The selected row's key.
    pub fn selected_key(&self) -> Option<&str> {
        self.listing
            .items
            .get(self.selected)
            .map(|item| item.key.as_str())
    }

    /// Selects the row `key`; `false` when it isn't listed.
    pub fn select_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        match self.listing.position(key) {
            Some(ix) => {
                self.select(ix, cx);
                true
            }
            None => false,
        }
    }

    fn build(&self, cx: &gpui::App) -> Listing {
        let reviews = self.reviews.read(cx);
        let rows = reviews.rows();
        match &self.mode {
            Mode::Repos => items::build(&rows, &self.query),
            Mode::Branches { location } => branches::build_branches(&rows, location, &self.query),
            Mode::Bases { location, branch } => {
                branches::build_bases(&rows, location, branch, &self.query)
            }
        }
    }

    /// Lists the rows again (the reviews changed, or the filter), keeping the selected row
    /// when it is still there; a new filter selects its first row.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let keep = self.selected_key().map(str::to_string);
        let listing = self.build(cx);
        if listing == self.listing {
            return;
        }
        self.listing = listing;
        // While it starts its repository lists its Start or Retry alone, inert, which keeps
        // the selection; then its first row has it: a target once it lists, or Start or Retry
        // again when the start failed.
        let started = self
            .started
            .as_ref()
            .and_then(|location| self.listing.first_ready_of(location));
        if started.is_some() {
            self.started = None;
        }
        let kept = keep.and_then(|key| self.listing.position(&key));
        let replaced = match kept {
            Some(_) => None,
            None => self
                .after_remove
                .take()
                .and_then(|location| self.listing.first_of(&location)),
        };
        self.selected = started.or(kept).or(replaced).unwrap_or(0);
        self.scroll_to_selected();
        cx.notify();
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_string();
        if query == self.query {
            return;
        }
        self.query = query;
        self.listing = self.build(cx);
        self.selected = 0;
        self.scroll_to_selected();
        cx.notify();
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = ix.min(self.listing.items.len().saturating_sub(1));
        self.scroll_to_selected();
        cx.notify();
    }

    fn scroll_to_selected(&self) {
        if let Some(entry) = self.listing.entry_of(self.selected) {
            self.scroll.scroll_to_item(entry);
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.listing.items.len();
        if len > 0 {
            self.select((self.selected + 1) % len, cx);
        }
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.listing.items.len();
        if len > 0 {
            self.select((self.selected + len - 1) % len, cx);
        }
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        self.pick(self.selected, window, cx);
    }

    fn remove_selected(&mut self, _: &RemoveRepo, _: &mut Window, cx: &mut Context<Self>) {
        let repo = self
            .listing
            .items
            .get(self.selected)
            .and_then(|item| item.repo);
        if let Some(repo) = repo {
            self.remove(repo, cx);
        }
    }

    /// Asks for the repository `listing.repos[repo]` to be removed, the one after it (else the
    /// one before) to have the selection once it has gone. Only the repositories' list removes:
    /// the branch picker's header is the repository it compares in.
    fn remove(&mut self, repo: usize, cx: &mut Context<Self>) {
        if self.mode != Mode::Repos {
            return;
        }
        let repos = &self.listing.repos;
        let Some(location) = repos.get(repo).map(|header| header.location.clone()) else {
            return;
        };
        self.after_remove = repos
            .get(repo + 1)
            .or_else(|| repo.checked_sub(1).and_then(|before| repos.get(before)))
            .map(|header| header.location.clone());
        cx.emit(SwitcherEvent::Remove { location });
    }

    /// `Esc`: a step back in the branch picker (module docs), else closes.
    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        let repo_key = self.listing.repos.first().map(|repo| repo.key.clone());
        match self.mode.clone() {
            Mode::Bases { location, branch } => {
                let select = repo_key.map(|key| format!("{key}/pick/{branch}"));
                self.set_mode(Mode::Branches { location }, select, window, cx);
            }
            Mode::Branches { .. } if self.back_to_repos => {
                let select = repo_key.map(|key| format!("{key}/compare"));
                self.set_mode(Mode::Repos, select, window, cx);
            }
            Mode::Branches { .. } | Mode::Repos => cx.emit(SwitcherEvent::Dismiss),
        }
    }

    /// Picks row `ix` (Enter on it, or a click); a start that runs already does nothing.
    fn pick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.listing.items.get(ix) else {
            return;
        };
        match &item.kind {
            ItemKind::CompareBranches => {
                if let Some(location) = self.listing.location_of(ix).cloned() {
                    self.back_to_repos = true;
                    self.set_mode(Mode::Branches { location }, None, window, cx);
                }
                return;
            }
            ItemKind::Branch { name, .. } => {
                let branch = name.clone();
                if let Some(location) = self.listing.location_of(ix).cloned() {
                    self.set_mode(Mode::Bases { location, branch }, None, window, cx);
                }
                return;
            }
            _ => {}
        }
        self.selected = ix;
        if item.is_starting() {
            cx.notify();
            return;
        }
        if let Some(location) = self.listing.start(ix) {
            self.started = Some(location.clone());
            cx.emit(SwitcherEvent::Start { location });
            return;
        }
        match self.listing.request(ix) {
            Some((location, request)) => cx.emit(SwitcherEvent::Open { location, request }),
            None if item.kind == items::ItemKind::OpenFolder => cx.emit(SwitcherEvent::OpenFolder),
            None => {}
        }
    }
}
