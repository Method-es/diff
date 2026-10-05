//! The switcher (PLAN §11 `switcher`, DESIGN §4.1; `Ctrl O`): every known repository with its
//! uncommitted changes, the targets it offers and its open reviews, in an overlay hung under the
//! title bar's switcher button over the scrim (DESIGN §3). Picking a row opens or resumes that
//! review; "Open folder…" adds a repository through the folder picker (`crate::system`).
//!
//! A filter field has the keyboard while it is open: typing filters ([`items::build`]), `↑`/`↓`
//! move the selection (wrapping), `Enter` picks, `Esc` closes. The switcher only reports what
//! was picked ([`SwitcherEvent`]); the shell opens it. Rows refresh as the reviews service
//! recomputes them (it does when the switcher opens), keeping the selected row.
//!
//! [`source`] is the title bar's other menu, the current repository's targets.
//!
//! Automation targets: `switcher`, `switcher/filter`, `switcher/repo/<repo>`, `switcher/<key>`
//! for each row ([`items::Item::key`]).

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

use crate::actions::switcher::{Confirm, Dismiss, SelectNext, SelectPrevious};
use crate::services::reviews::{OpenRequest, Reviews};
use items::Listing;

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
    /// Closed without picking (`Esc`).
    Dismiss,
}

/// The switcher (see the module docs).
pub struct Switcher {
    reviews: Entity<Reviews>,
    input: Entity<InputState>,
    query: String,
    listing: Listing,
    /// Into `listing.items`.
    selected: usize,
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
            cx.observe(&reviews, |this, _, cx| this.rebuild(cx)),
        ];
        let mut this = Self {
            reviews,
            input,
            query: String::new(),
            listing: Listing::default(),
            selected: 0,
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
        items::build(&reviews.rows(), &self.query)
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
        self.selected = keep
            .and_then(|key| self.listing.position(&key))
            .unwrap_or(0);
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

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.pick(self.selected, cx);
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(SwitcherEvent::Dismiss);
    }

    /// Picks row `ix` (Enter on it, or a click).
    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(item) = self.listing.items.get(ix) else {
            return;
        };
        self.selected = ix;
        match self.listing.request(ix) {
            Some((location, request)) => cx.emit(SwitcherEvent::Open { location, request }),
            None if item.kind == items::ItemKind::OpenFolder => cx.emit(SwitcherEvent::OpenFolder),
            None => {}
        }
    }
}
