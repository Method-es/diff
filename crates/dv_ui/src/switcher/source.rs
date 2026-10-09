//! The source picker (DESIGN §4.1, the title bar's `[source ▾]`): a menu of the open review's
//! repository's targets — uncommitted changes, staged changes and the branch against the
//! default branch ([`super::items::target_offers`]) — each opening or resuming that target's
//! review. The current review's target is checked.
//!
//! It has the keyboard itself while open: `↑`/`↓` (wrapping), `Enter`, `Esc`.
//!
//! Automation targets: `source_picker`, `source_picker/<working|staged|branch>`.

use dv_core::{Location, Target};
use gpui::{
    AnyElement, Context, ElementId, EventEmitter, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};

use super::items::{TargetOffer, target_offers};
use crate::actions::source_picker::{Confirm, Dismiss, SelectNext, SelectPrevious};
use crate::automation::AutomationTargetExt as _;
use crate::components::{count_badge, icon, status_chip};
use crate::icons::DvIcon;
use crate::keymap;
use crate::services::reviews::Reviews;
use crate::theme::{ActiveTheme as _, DvTheme, StyledText as _};

/// The menu's width.
pub const SOURCE_PICKER_WIDTH: gpui::Pixels = gpui::px(340.);

/// What was picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourcePickerEvent {
    Open { location: Location, target: Target },
    Dismiss,
}

/// The source picker (see the module docs).
pub struct SourcePicker {
    focus_handle: FocusHandle,
    reviews: gpui::Entity<Reviews>,
    location: Location,
    /// The open review's target, checked in the menu; `None` when the open review isn't this
    /// repository's own (one homed in another worktree, PLAN §2.4).
    current: Option<Target>,
    offers: Vec<TargetOffer>,
    selected: usize,
    _subscription: Subscription,
}

impl EventEmitter<SourcePickerEvent> for SourcePicker {}

impl Focusable for SourcePicker {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SourcePicker {
    /// The menu for the repository at `location`, whose review of `current` is open; the
    /// selection starts on `current`.
    pub fn new(
        reviews: gpui::Entity<Reviews>,
        location: Location,
        current: Option<Target>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.observe(&reviews, |this, _, cx| this.rebuild(cx));
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            reviews,
            location,
            current,
            offers: Vec::new(),
            selected: 0,
            _subscription: subscription,
        };
        this.offers = this.build(cx);
        this.selected = this
            .offers
            .iter()
            .position(|offer| this.current.as_ref() == Some(&offer.target))
            .unwrap_or(0);
        this
    }

    pub fn offers(&self) -> &[TargetOffer] {
        &self.offers
    }

    pub fn selected(&self) -> Option<&TargetOffer> {
        self.offers.get(self.selected)
    }

    fn build(&self, cx: &gpui::App) -> Vec<TargetOffer> {
        self.reviews
            .read(cx)
            .row(&self.location)
            .map(target_offers)
            .unwrap_or_default()
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let keep = self.selected().map(|offer| offer.kind);
        let offers = self.build(cx);
        if offers != self.offers {
            self.offers = offers;
            self.selected = keep
                .and_then(|kind| self.offers.iter().position(|offer| offer.kind == kind))
                .unwrap_or(0);
            cx.notify();
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        if !self.offers.is_empty() {
            self.selected = (self.selected + 1) % self.offers.len();
            cx.notify();
        }
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.offers.len();
        if len > 0 {
            self.selected = (self.selected + len - 1) % len;
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.pick(self.selected, cx);
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(SourcePickerEvent::Dismiss);
    }

    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(offer) = self.offers.get(ix) {
            self.selected = ix;
            cx.emit(SourcePickerEvent::Open {
                location: self.location.clone(),
                target: offer.target.clone(),
            });
        }
    }

    fn render_offer(
        &self,
        ix: usize,
        offer: &TargetOffer,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let selected = ix == self.selected;
        let current = self.current.as_ref() == Some(&offer.target);
        let hover = c.control_hover;
        div()
            .id(ElementId::NamedInteger("source-offer".into(), ix as u64))
            .flex()
            .items_center()
            .gap(theme.spacing.md)
            .h(px(30.))
            .px(theme.spacing.md)
            .rounded(theme.radii.control)
            .cursor_pointer()
            .map(|this| {
                if selected {
                    this.bg(c.sel)
                } else {
                    this.hover(move |style| style.bg(hover))
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| this.pick(ix, cx)))
            .child(div().flex_none().w(px(14.)).when(current, |this| {
                this.child(icon(DvIcon::Check, px(13.), c.accent_text))
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(offer.label.clone())),
            )
            .children(offer.review.as_ref().map(|badge| {
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.sm)
                    .child(status_chip(badge.status, theme))
                    .when(badge.open_threads > 0, |this| {
                        this.child(count_badge(theme).child(badge.open_threads.to_string()))
                    })
            }))
            .automation_target(format!("source_picker/{}", offer.kind.key()))
    }
}

impl Render for SourcePicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.dv_theme().clone();
        let c = &theme.colors;
        let rows: Vec<AnyElement> = self
            .offers
            .clone()
            .iter()
            .enumerate()
            .map(|(ix, offer)| self.render_offer(ix, offer, &theme, cx))
            .collect();
        div()
            .key_context(keymap::SOURCE_PICKER)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .w(SOURCE_PICKER_WIDTH)
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(theme.spacing.xs)
            .rounded(theme.radii.card)
            .border_1()
            .border_color(c.overlay_line)
            .bg(c.overlay)
            .shadow(theme.shadows.overlay.clone())
            .occlude()
            .text_spec(&theme.text.body)
            .text_color(c.fg)
            .child(
                div()
                    .px(theme.spacing.md)
                    .pt(theme.spacing.sm)
                    .pb(theme.spacing.xs)
                    .text_spec(&theme.text.secondary)
                    .text_color(c.fg3)
                    .child(format!("Review in {}", self.location.short_name())),
            )
            .children(rows)
            .automation_target("source_picker")
    }
}
