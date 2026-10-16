//! The References tab (PLAN §18.1 "Find references", DESIGN §4.1 "References"): a symbol's
//! references, or a go to definition's several places, grouped by file in path order, each row
//! its line's text highlighted with the symbol marked.
//!
//! **The rows.** The places come from the server; their lines are read in the background as
//! the server read them ([`read_rows`]): a worktree file from the working tree, a file outside
//! the worktree through `Backend::probe_path` (up to its read cap), and each file's lines
//! highlighted whole (`crate::syntax`). A row on a line the review changed carries the changed
//! marker: its line is an added line of the review's new side, with the same text (so a row
//! read against the working tree on a review whose new side isn't it is marked only where the
//! two agree); **In this review** keeps only those. The review's diffs of the files the rows
//! are in load for it (`ReviewSession::load_for_references`), whatever the stream's viewport.
//!
//! **The view.** [`ReferencesPanel`] fills the panel while its tab is chosen: a header (the
//! title, the symbol, the count, **In this review**, a close button), then the files, each its
//! path and its rows; a click on a row goes where go to definition goes ([`super::IntelEvent`]).
//!
//! Automation targets: `references` (the tab's body), `references/close`,
//! `references/in_review`, `references/row/<path>/<line>` (the first row on that line).

use std::collections::{BTreeMap, HashSet};

use dv_core::Backend;
use dv_core::backend::PathProbe;
use dv_core::diff::LineKind;
use dv_lsp::{SymbolLocation, UriTarget};
use gpui::{
    AnyElement, App, Context, Entity, FontWeight, HighlightStyle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, ScrollHandle, StatefulInteractiveElement as _,
    Styled as _, StyledText, Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
};

use super::{CodeIntel, SymbolAt};
use crate::automation::AutomationTargetExt as _;
use crate::components::{checkbox, icon, skeleton};
use crate::icons::DvIcon;
use crate::session::ReviewSession;
use crate::syntax::{Token, highlight_side};
use crate::theme::{ActiveTheme as _, DvTheme, StyledText as _};

/// What the list holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferencesKind {
    References,
    /// A go to definition that found several places.
    Definitions,
}

impl ReferencesKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::References => "References",
            Self::Definitions => "Definitions",
        }
    }

    /// `"references"` or `"definitions"`, as automation state spells it.
    pub fn key(self) -> &'static str {
        match self {
            Self::References => "references",
            Self::Definitions => "definitions",
        }
    }
}

/// One symbol's references (module docs).
pub struct References {
    pub kind: ReferencesKind,
    pub from: SymbolAt,
    /// The pane it was asked from.
    pub pane: super::Pane,
    /// The server read the asking file's text in place of the disk's (PLAN §18.2).
    pub against_worktree: bool,
    /// **In this review**: only the rows on lines the review changed.
    pub in_review: bool,
    /// The places, by file, once their lines are read.
    pub files: Option<Vec<RefFile>>,
    /// Why there are none to show.
    pub error: Option<String>,
    pub(super) reading: Option<Task<()>>,
}

impl References {
    pub(super) fn loading(
        kind: ReferencesKind,
        from: SymbolAt,
        pane: super::Pane,
        against_worktree: bool,
        in_review: bool,
    ) -> Self {
        Self {
            kind,
            from,
            pane,
            against_worktree,
            in_review,
            files: None,
            error: None,
            reading: None,
        }
    }

    /// The server hasn't answered, or the rows' lines aren't read yet.
    pub fn is_loading(&self) -> bool {
        self.files.is_none() && self.error.is_none()
    }

    /// Every place found.
    pub fn count(&self) -> usize {
        self.files
            .iter()
            .flatten()
            .map(|file| file.rows.len())
            .sum()
    }

    /// The worktree files the places are in.
    pub(super) fn worktree_paths(&self) -> Vec<String> {
        self.files
            .iter()
            .flatten()
            .filter_map(|file| match &file.target {
                UriTarget::Worktree(path) => Some(path.clone()),
                UriTarget::Outside { .. } => None,
            })
            .collect()
    }

    /// The rows shown, by file, with whether the review changed each (module docs): every row,
    /// or with **In this review** only the changed ones (and only files that keep a row).
    pub fn shown(&self, session: &ReviewSession) -> Vec<(&RefFile, Vec<(&RefRow, bool)>)> {
        self.files
            .iter()
            .flatten()
            .filter_map(|file| {
                let changed = changed_lines(file, session);
                let rows: Vec<(&RefRow, bool)> = file
                    .rows
                    .iter()
                    .map(|row| (row, changed.contains(&row.line)))
                    .filter(|(_, changed)| !self.in_review || *changed)
                    .collect();
                (!rows.is_empty()).then_some((file, rows))
            })
            .collect()
    }
}

/// One file's places.
pub struct RefFile {
    pub target: UriTarget,
    pub rows: Vec<RefRow>,
}

impl RefFile {
    /// Its path as the list shows it: worktree-relative, else the path on the backend's
    /// machine (or the URI, for one that isn't a file).
    pub fn label(&self) -> &str {
        match &self.target {
            UriTarget::Worktree(path) => path,
            UriTarget::Outside { uri, path } => path.as_deref().unwrap_or(uri),
        }
    }

    /// The worktree path, for a file inside the worktree.
    pub fn worktree_path(&self) -> Option<&str> {
        match &self.target {
            UriTarget::Worktree(path) => Some(path),
            UriTarget::Outside { .. } => None,
        }
    }
}

/// One place.
pub struct RefRow {
    pub location: SymbolLocation,
    /// Its line, 1-based.
    pub line: u32,
    /// The line's text, when it could be read.
    pub text: Option<String>,
    /// Its tokens, when the file has a grammar.
    pub tokens: Vec<Token>,
}

impl RefRow {
    /// The symbol's bytes in the line's text (a span over several lines ends at the line's
    /// end).
    pub fn symbol(&self) -> std::ops::Range<usize> {
        let len = self.text.as_ref().map_or(0, String::len);
        let range = &self.location.range;
        let start = (range.start.column as usize).min(len);
        let end = if range.end.line == range.start.line {
            (range.end.column as usize).clamp(start, len)
        } else {
            len
        };
        start..end
    }
}

/// The new-side lines of `file` the review added with the text the row shows (module docs).
fn changed_lines(file: &RefFile, session: &ReviewSession) -> HashSet<u32> {
    let Some(path) = file.worktree_path() else {
        return HashSet::new();
    };
    let Some(text) = session
        .file(path)
        .and(session.shown_diff(path))
        .and_then(|diff| diff.as_text())
    else {
        return HashSet::new();
    };
    let wanted: HashSet<u32> = file.rows.iter().map(|row| row.line).collect();
    text.lines
        .iter()
        .filter(|line| line.kind == LineKind::Added)
        .filter_map(|line| {
            let number = line.new_no?;
            let row = file.rows.iter().find(|row| row.line == number)?;
            (wanted.contains(&number) && row.text.as_deref() == Some(line.text.as_str()))
                .then_some(number)
        })
        .collect()
}

/// `locations` grouped by file in path order, each file's lines read and highlighted (module
/// docs). Blocking: runs on a background thread.
pub fn read_rows(backend: &dyn Backend, locations: Vec<SymbolLocation>) -> Vec<RefFile> {
    let mut by_file: BTreeMap<UriTarget, Vec<SymbolLocation>> = BTreeMap::new();
    for location in locations {
        by_file
            .entry(location.target.clone())
            .or_default()
            .push(location);
    }
    by_file
        .into_iter()
        .map(|(target, mut locations)| {
            locations.sort_by_key(|location| location.range);
            locations.dedup();
            let bytes = match &target {
                UriTarget::Worktree(path) => backend.worktree_read(path).ok().flatten(),
                UriTarget::Outside {
                    path: Some(path), ..
                } => match backend.probe_path(path) {
                    Ok(PathProbe::File {
                        contents: Some(bytes),
                        ..
                    }) => Some(bytes),
                    _ => None,
                },
                UriTarget::Outside { path: None, .. } => None,
            };
            let text = bytes.map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
            let lines: Vec<&str> = text
                .as_deref()
                .map(|text| {
                    text.split('\n')
                        .map(|line| line.strip_suffix('\r').unwrap_or(line))
                        .collect()
                })
                .unwrap_or_default();
            let name = match &target {
                UriTarget::Worktree(path) => path.clone(),
                UriTarget::Outside { uri, path } => path.clone().unwrap_or_else(|| uri.clone()),
            };
            let highlights = highlight_side(&name, &lines);
            let rows = locations
                .into_iter()
                .map(|location| {
                    let line = location.range.start.line + 1;
                    RefRow {
                        text: lines
                            .get(location.range.start.line as usize)
                            .map(|text| text.to_string()),
                        tokens: highlights
                            .as_ref()
                            .map(|side| side.line(line).to_vec())
                            .unwrap_or_default(),
                        line,
                        location,
                    }
                })
                .collect();
            RefFile { target, rows }
        })
        .collect()
}

/// The References tab's body (module docs).
pub struct ReferencesPanel {
    intel: Option<Entity<CodeIntel>>,
    scroll: ScrollHandle,
    _subscription: Option<Subscription>,
}

impl ReferencesPanel {
    pub fn new() -> Self {
        Self {
            intel: None,
            scroll: ScrollHandle::new(),
            _subscription: None,
        }
    }

    /// Shows `intel`'s list (the open review's), or none.
    pub fn set_intel(&mut self, intel: Option<Entity<CodeIntel>>, cx: &mut Context<Self>) {
        self._subscription = intel
            .as_ref()
            .map(|intel| cx.observe(intel, |_, _, cx| cx.notify()));
        self.intel = intel;
        self.scroll = ScrollHandle::new();
        cx.notify();
    }
}

impl Default for ReferencesPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for ReferencesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.dv_theme().clone();
        let body = div().id("references").flex_1().min_h_0().flex().flex_col();
        let Some(intel) = self.intel.clone() else {
            return body.automation_target("references");
        };
        let intel_ref = intel.read(cx);
        let Some(list) = intel_ref.references() else {
            return body.automation_target("references");
        };
        let session = intel_ref.session().read(cx);
        let header = render_header(list, &intel, &theme, cx);
        let content: AnyElement = if list.is_loading() {
            div()
                .flex()
                .flex_col()
                .gap(theme.spacing.md)
                .px(theme.spacing.lg)
                .py(theme.spacing.md)
                .children((0..4).map(|_| skeleton(px(180.), px(10.), &theme)))
                .into_any_element()
        } else if let Some(error) = &list.error {
            note(error.clone(), &theme)
        } else {
            let shown = list.shown(session);
            let asked = Some(super::Asked {
                at: list.from.clone(),
                pane: list.pane,
                worktree: !list.against_worktree,
                definition: false,
            });
            if shown.is_empty() {
                note(
                    if list.in_review && list.count() > 0 {
                        "None on lines this review changed".to_string()
                    } else {
                        format!("No {} found", list.kind.title().to_lowercase())
                    },
                    &theme,
                )
            } else {
                div()
                    .flex()
                    .flex_col()
                    .pb(theme.spacing.md)
                    .children(
                        shown
                            .into_iter()
                            .map(|(file, rows)| render_file(file, rows, &asked, &intel, &theme)),
                    )
                    .into_any_element()
            }
        };
        body.child(header)
            .child(
                div()
                    .id("references-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(content),
            )
            .automation_target("references")
    }
}

/// A quiet line in the tab's body.
fn note(text: String, theme: &DvTheme) -> AnyElement {
    div()
        .px(theme.spacing.lg)
        .py(theme.spacing.md)
        .text_spec(&theme.text.secondary)
        .text_color(theme.colors.fg3)
        .child(text)
        .into_any_element()
}

/// The tab's header: its title and the symbol with the count, then **In this review** and the
/// close button; "Read against the working tree" under it when that is so.
fn render_header(
    list: &References,
    intel: &Entity<CodeIntel>,
    theme: &DvTheme,
    cx: &App,
) -> AnyElement {
    let c = &theme.colors;
    let count = (!list.is_loading() && list.error.is_none()).then(|| list.count());
    let in_review = list.in_review;
    let toggle = {
        let intel = intel.clone();
        div()
            .id("references-in-review")
            .flex()
            .items_center()
            .gap(theme.spacing.sm)
            .cursor_pointer()
            .text_spec(&theme.text.secondary)
            .text_color(c.fg2)
            .child(checkbox(in_review, theme))
            .child("In this review")
            .on_click(move |_, _, cx| {
                intel.update(cx, |intel, cx| intel.set_in_review(!in_review, cx));
            })
            .automation_target("references/in_review")
    };
    let close = {
        let intel = intel.clone();
        div()
            .id("references-close")
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .size(theme.sizes.control_xsmall_height)
            .rounded(theme.radii.control)
            .cursor_pointer()
            .hover(|style| style.bg(c.control_hover))
            .child(icon(DvIcon::X, px(14.), c.fg2))
            .tooltip(crate::components::tooltip("Close the references"))
            .on_click(move |_, _, cx| {
                intel.update(cx, |intel, cx| intel.close_references(cx));
            })
            .automation_target("references/close")
    };
    let _ = cx;
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(theme.spacing.sm)
        .pt(theme.spacing.md)
        .px(theme.spacing.lg)
        .pb(theme.spacing.md)
        .border_b_1()
        .border_color(c.line)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.spacing.md)
                .child(
                    div()
                        .text_spec(&theme.text.title)
                        .text_color(c.fg)
                        .child(list.kind.title()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_spec(&theme.text.inline_code)
                        .text_color(c.accent_text)
                        .child(list.from.name.clone()),
                )
                .children(count.map(|count| div().text_color(c.fg3).child(count.to_string())))
                .child(div().flex_1())
                .child(close),
        )
        .child(toggle)
        .when(list.against_worktree, |this| {
            this.child(
                div()
                    .text_spec(&theme.text.secondary)
                    .text_color(c.fg3)
                    .child("Read against the working tree"),
            )
        })
        .into_any_element()
}

/// One file of the list: its path, then its rows.
fn render_file(
    file: &RefFile,
    rows: Vec<(&RefRow, bool)>,
    asked: &Option<super::Asked>,
    intel: &Entity<CodeIntel>,
    theme: &DvTheme,
) -> AnyElement {
    let c = &theme.colors;
    let label = file.label().to_string();
    let (dir, name) = match label.rsplit_once('/') {
        Some((dir, name)) => (format!("{dir}/"), name.to_string()),
        None => (String::new(), label.clone()),
    };
    let outside = file.worktree_path().is_none();
    let path = div()
        .flex()
        .items_center()
        .gap(theme.spacing.sm)
        .h(px(26.))
        .px(theme.spacing.lg)
        .mt(theme.spacing.sm)
        .child(icon(DvIcon::File, px(13.), c.fg3))
        .child(
            div()
                .min_w_0()
                .flex()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_spec(&theme.text.secondary)
                .child(div().min_w_0().truncate().text_color(c.fg3).child(dir))
                .child(
                    div()
                        .flex_none()
                        .text_color(c.fg)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name),
                ),
        )
        .when(outside, |this| {
            this.child(
                div()
                    .flex_none()
                    .text_spec(&theme.text.secondary)
                    .text_color(c.fg3)
                    .child("outside the repository"),
            )
        })
        .child(div().flex_1())
        .child(
            div()
                .flex_none()
                .text_spec(&theme.text.secondary)
                .text_color(c.fg3)
                .child(rows.len().to_string()),
        );
    let mut seen = HashSet::new();
    div()
        .flex()
        .flex_col()
        .child(path)
        .children(rows.into_iter().map(|(row, changed)| {
            let first = seen.insert(row.line);
            render_row(&label, row, changed, first, asked, intel, theme)
        }))
        .into_any_element()
}

/// One place: the changed marker, the line number, the line's text with the symbol marked.
fn render_row(
    label: &str,
    row: &RefRow,
    changed: bool,
    first_on_line: bool,
    asked: &Option<super::Asked>,
    intel: &Entity<CodeIntel>,
    theme: &DvTheme,
) -> AnyElement {
    let c = &theme.colors;
    let text = row.text.clone().unwrap_or_default();
    let trimmed = text.trim_start();
    let indent = text.len() - trimmed.len();
    let symbol = row.symbol();
    let colors: Vec<(std::ops::Range<usize>, gpui::Hsla)> = row
        .tokens
        .iter()
        .filter_map(|token| {
            let start = (token.range.start as usize).max(indent);
            let end = token.range.end as usize;
            (end > start && end <= text.len())
                .then(|| (start - indent..end - indent, token.kind.color(&c.syntax)))
        })
        .collect();
    let mark = (symbol.end > symbol.start && symbol.start >= indent)
        .then(|| symbol.start - indent..symbol.end - indent);
    let highlights = runs(trimmed.len(), &colors, mark, c.find_current);
    let shown = trimmed.to_string();
    let styled = StyledText::new(shown).with_highlights(highlights);
    let location = row.location.clone();
    let intel = intel.clone();
    let hover = c.control_hover;
    let id = format!("references/row/{label}/{}", row.line);
    let asked = asked.clone();
    // One element per place, two on one line apart by their column.
    let element_id = format!("{id}/{}", row.location.range.start.column);
    let element = div()
        .id(gpui::ElementId::Name(element_id.into()))
        .flex()
        .items_center()
        .h(px(24.))
        .pr(theme.spacing.lg)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .child(
            div()
                .flex_none()
                .w(px(3.))
                .h(px(16.))
                .mr(px(9.))
                .rounded(px(1.5))
                .when(changed, |this| this.bg(c.green)),
        )
        .child(
            div()
                .flex_none()
                .w(px(36.))
                .pr(theme.spacing.md)
                .flex()
                .justify_end()
                .text_spec(&theme.text.kbd)
                .text_color(c.fg3)
                .child(row.line.to_string()),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_spec(&theme.text.inline_code)
                .text_color(c.syntax.plain)
                .child(styled),
        )
        .on_click(move |_, _, cx| {
            let location = location.clone();
            let asked = asked.clone();
            intel.update(cx, |intel, cx| intel.go_to(location, asked, cx));
        });
    if first_on_line {
        element.automation_target(id)
    } else {
        element.into_any_element()
    }
}

/// The highlight runs of a `len`-byte line: each token's colour (`colors`, sorted, without
/// overlaps), and over `mark` (the symbol) a `mark_bg` background at 600, whatever token it is
/// in.
pub fn runs(
    len: usize,
    colors: &[(std::ops::Range<usize>, gpui::Hsla)],
    mark: Option<std::ops::Range<usize>>,
    mark_bg: gpui::Hsla,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    let mut cuts: Vec<usize> = vec![0, len];
    for (range, _) in colors {
        cuts.extend([range.start, range.end]);
    }
    if let Some(mark) = &mark {
        cuts.extend([mark.start, mark.end]);
    }
    cuts.retain(|&cut| cut <= len);
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .filter_map(|pair| {
            let piece = pair[0]..pair[1];
            let color = colors
                .iter()
                .find(|(range, _)| range.start <= piece.start && piece.end <= range.end)
                .map(|(_, color)| *color);
            let marked = mark
                .as_ref()
                .is_some_and(|mark| mark.start <= piece.start && piece.end <= mark.end);
            (color.is_some() || marked).then(|| {
                (
                    piece,
                    HighlightStyle {
                        color,
                        background_color: marked.then_some(mark_bg),
                        font_weight: marked.then_some(FontWeight::SEMIBOLD),
                        ..Default::default()
                    },
                )
            })
        })
        .collect()
}
