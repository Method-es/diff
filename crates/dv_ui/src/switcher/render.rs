//! The switcher's panel (DESIGN §3: `overlay` over the scrim, radius 12, the overlay shadow):
//! the filter field, the repositories and their rows, and a footer with "Open folder…" and the
//! keys.

use gpui::{
    AnyElement, Context, ElementId, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, prelude::FluentBuilder as _, px,
};
use gpui_component::input::Input;

use super::items::{Entry, Item, ItemKind, RepoHeader, RepoSummary, TargetKind};
use super::{SWITCHER_WIDTH, Switcher};
use crate::automation::AutomationTargetExt as _;
use crate::components::{count_badge, icon, kbd, location_chip, skeleton, status_chip, totals};
use crate::icons::DvIcon;
use crate::keymap;
use crate::theme::{ActiveTheme as _, DvTheme, StyledText as _};

impl Render for Switcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.dv_theme().clone();
        let c = &theme.colors;
        let viewport = window.viewport_size();
        let max_height = (viewport.height - theme.sizes.title_bar_height - px(56.)).max(px(200.));
        div()
            .key_context(keymap::SWITCHER)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .w(SWITCHER_WIDTH)
            .max_h(max_height)
            .flex()
            .flex_col()
            .rounded(theme.radii.sheet)
            .border_1()
            .border_color(c.overlay_line)
            .bg(c.overlay)
            .shadow(theme.shadows.overlay.clone())
            .overflow_hidden()
            .occlude()
            .text_spec(&theme.text.body)
            .text_color(c.fg)
            .child(self.render_filter(&theme))
            .child(self.render_list(&theme, cx))
            .child(self.render_footer(&theme, cx))
            .automation_target("switcher")
    }
}

impl Switcher {
    fn render_filter(&self, theme: &DvTheme) -> AnyElement {
        let c = &theme.colors;
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(46.))
            .px(px(14.))
            .border_b_1()
            .border_color(c.line)
            .child(icon(DvIcon::Search, px(15.), c.fg3))
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.input)
                        .appearance(false)
                        .aria_label("Filter repositories and reviews")
                        .px_0()
                        .text_size(px(14.)),
                ),
            )
            .automation_target("switcher/filter")
    }

    fn render_list(&self, theme: &DvTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let list = div()
            .id("switcher-list")
            .track_scroll(&self.scroll)
            .flex_shrink(1.)
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .p(theme.spacing.sm);
        if self.listing.entries.is_empty() {
            let known = !self.reviews.read(cx).rows().is_empty();
            let (title, note) = if known {
                (
                    format!("No repositories or reviews match “{}”", self.query.trim()),
                    "Try part of a repository's name, a branch or a status.".to_string(),
                )
            } else {
                (
                    "No repositories yet".to_string(),
                    "Open a folder with a git repository to review its changes.".to_string(),
                )
            };
            return list
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(theme.spacing.xs)
                        .py(theme.spacing.xxl)
                        .child(div().text_color(c.fg2).child(title))
                        .child(
                            div()
                                .text_spec(&theme.text.secondary)
                                .text_color(c.fg3)
                                .child(note),
                        ),
                )
                .automation_target("switcher/empty");
        }
        let lines: Vec<AnyElement> = self
            .listing
            .entries
            .iter()
            .enumerate()
            .filter_map(|(line, entry)| match *entry {
                Entry::Repo(repo) => self
                    .listing
                    .repos
                    .get(repo)
                    .map(|header| repo_header(header, line == 0, theme)),
                Entry::Item(ix) => self
                    .listing
                    .items
                    .get(ix)
                    .map(|item| self.render_item(ix, item, theme, cx)),
            })
            .collect();
        list.children(lines).into_any_element()
    }

    fn render_item(
        &self,
        ix: usize,
        item: &Item,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let selected = ix == self.selected;
        let glyph = match &item.kind {
            ItemKind::Target(offer) => match offer.kind {
                TargetKind::Working => DvIcon::FileDiff,
                // Not a check: the source picker's check marks the current target.
                TargetKind::Staged => DvIcon::Package,
                TargetKind::Branch => DvIcon::GitBranch,
            },
            ItemKind::Review { .. } => DvIcon::GitCommit,
            ItemKind::OpenFolder => DvIcon::FolderOpen,
        };
        let right: Option<AnyElement> = match item.review() {
            Some(badge) => Some(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.md)
                    .child(status_chip(badge.status, theme))
                    .child(
                        div()
                            .text_spec(&theme.text.secondary)
                            .text_color(c.fg3)
                            .child(format!("round {}", badge.round)),
                    )
                    .when(badge.open_threads > 0, |this| {
                        this.child(
                            count_badge(theme)
                                .child(icon(DvIcon::MessageSquare, px(10.), c.accent_text))
                                .child(format!("{} open", badge.open_threads)),
                        )
                    })
                    .into_any_element(),
            ),
            None if matches!(item.kind, ItemKind::Target(_)) && selected => Some(
                div()
                    .text_spec(&theme.text.secondary)
                    .text_color(c.fg3)
                    .child("New review")
                    .into_any_element(),
            ),
            None => None,
        };
        let footer = item.kind == ItemKind::OpenFolder;
        let hover = c.control_hover;
        div()
            .id(ElementId::NamedInteger("switcher-item".into(), ix as u64))
            .flex()
            .items_center()
            .gap(theme.spacing.md)
            .h(px(30.))
            .pl(if footer { theme.spacing.md } else { px(30.) })
            .pr(theme.spacing.md)
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
            .child(icon(glyph, px(14.), if selected { c.fg } else { c.fg2 }))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .when(footer, |this| this.font_weight(FontWeight::MEDIUM))
                    .child(SharedString::from(item.label().to_string())),
            )
            .child(div().flex_1())
            .children(right)
            .automation_target(format!("switcher/{}", item.key))
    }

    fn render_footer(&self, theme: &DvTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let open_folder = self
            .listing
            .items
            .iter()
            .position(|item| item.kind == ItemKind::OpenFolder)
            .and_then(|ix| {
                let item = self.listing.items.get(ix)?;
                Some(self.render_item(ix, item, theme, cx))
            });
        let hint = |keys: Vec<&'static str>, what: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .children(keys.into_iter().map(|key| kbd(keymap::label(key), theme)))
                .child(what)
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.md)
            .h(px(44.))
            .px(theme.spacing.sm)
            .border_t_1()
            .border_color(c.line)
            .children(open_folder)
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.lg)
                    .pr(theme.spacing.sm)
                    .text_spec(&theme.text.secondary)
                    .text_color(c.fg3)
                    .child(hint(vec!["up", "down"], "move"))
                    .child(hint(vec!["enter"], "open"))
                    .child(hint(vec!["escape"], "close")),
            )
            .into_any_element()
    }
}

/// A repository's header: its name, its path dimmed, its location chip, and a line on its
/// uncommitted changes or why it can't be opened.
fn repo_header(header: &RepoHeader, first: bool, theme: &DvTheme) -> AnyElement {
    let c = &theme.colors;
    let summary: AnyElement = match &header.summary {
        RepoSummary::Loading => skeleton(px(160.), px(10.), theme),
        RepoSummary::Unreachable(reason) => {
            let text = match &header.distro {
                Some(distro) => format!("{distro} · {reason}"),
                None => reason.clone(),
            };
            problem(&text, c.yellow, theme)
        }
        RepoSummary::Error(message) => problem(message, c.red, theme),
        RepoSummary::Clean => div()
            .text_color(c.fg3)
            .child("Working tree clean")
            .into_any_element(),
        RepoSummary::Changes(working) => div()
            .flex()
            .items_center()
            .gap(theme.spacing.md)
            .child(div().text_color(c.fg2).child(format!(
                "uncommitted changes · {}",
                if working.files == 1 {
                    "1 file".to_string()
                } else {
                    format!("{} files", working.files)
                }
            )))
            .child(totals(
                working.additions,
                working.deletions,
                px(11.5),
                theme,
            ))
            .into_any_element(),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .pt(if first { theme.spacing.xs } else { px(10.) })
        .pb(theme.spacing.xs)
        .px(theme.spacing.md)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.spacing.md)
                .child(icon(DvIcon::Folder, px(14.), c.fg2))
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(SharedString::from(header.name.clone())),
                )
                .children(
                    header
                        .distro
                        .as_deref()
                        .map(|distro| location_chip(distro, theme)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis_start()
                        .text_spec(&theme.text.secondary)
                        .text_color(c.fg3)
                        .child(SharedString::from(header.path.clone())),
                ),
        )
        .child(
            div()
                .pl(px(22.))
                .h(px(16.))
                .flex()
                .items_center()
                .text_spec(&theme.text.secondary)
                .child(summary),
        )
        .automation_target(format!("switcher/repo/{}", header.key))
}

/// A one-line problem with its glyph.
fn problem(text: &str, hue: gpui::Hsla, theme: &DvTheme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(theme.spacing.sm)
        .min_w_0()
        .child(icon(DvIcon::TriangleAlert, px(12.), hue))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(theme.colors.fg2)
                .child(SharedString::from(text.to_string())),
        )
        .into_any_element()
}
