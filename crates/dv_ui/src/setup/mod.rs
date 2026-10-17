//! The setup screen (PLAN §12, DESIGN §4.3): every environment's checks (`services::Setup`) as
//! one column of cards, which the first-run screen ([`first_run`]) and Settings › Setup
//! ([`settings::SettingsView`]) both show.
//!
//! Each environment is a card: its glyph, its name and how it runs ("This PC", "WSL · running",
//! "WSL · stopped" with **Start**), why its checks are short when they are, then one row per
//! check: its state's glyph, its title, its detail, and its fix — a button when dv applies it
//! ("Install", "Update", "Add to PATH"; while it runs, its progress), else the command to run
//! with a copy button (through the clipboard seam, `crate::system::write_clipboard`). A fix that
//! failed says why under its row, in `red`. **Check again** heads the column.
//!
//! Automation targets: `setup/check_again`, `setup/<environment>/start`,
//! `setup/<environment>/<check>/fix` and `…/copy`, `setup/<environment>/<check>` for a row.

pub mod settings;

#[cfg(test)]
mod tests;

use dv_cli::checks::{Check, Kind, State};
use gpui::{
    AnyElement, App, Entity, FontWeight, IntoElement, ParentElement as _, Pixels, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::automation::AutomationTargetExt as _;
use crate::components::{
    elide_path_middle, icon, small_button, small_icon_button, text_width, tooltip,
};
use crate::icons::DvIcon;
use crate::services::setup::{Applying, EnvState, Setup};
use crate::theme::{DvTheme, StyledText as _};

/// The column's width (the board's 860).
pub const COLUMN_WIDTH: f32 = 860.;

/// The title's line under it (the board's).
pub const SUBTITLE: &str = "dv runs where your code lives. Each environment is set up once, here.";

/// The column's heading: `title` with the line under it, and **Check again** on the right.
pub fn heading(
    title: &str,
    title_spec: &crate::theme::TextSpec,
    setup: &Entity<Setup>,
    theme: &DvTheme,
    cx: &App,
) -> AnyElement {
    let c = &theme.colors;
    let checking = setup.read(cx).is_checking();
    let again = {
        let setup = setup.clone();
        crate::components::ghost_button("setup-check-again", theme)
            .flex_none()
            .h(theme.sizes.control_small_height)
            .text_spec(&theme.text.secondary)
            .font_weight(FontWeight::MEDIUM)
            .text_color(c.fg2)
            .child(icon(DvIcon::RotateCw, px(13.), c.fg2))
            .child(if checking {
                "Checking…"
            } else {
                "Check again"
            })
            .on_click(move |_, _, cx| setup.update(cx, |setup, cx| setup.check_again(cx)))
            .automation_target("setup/check_again")
    };
    div()
        .flex()
        .items_end()
        .gap(theme.spacing.lg)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_spec(title_spec)
                        .text_color(c.fg)
                        .child(title.to_string()),
                )
                .child(div().text_color(c.fg2).child(SUBTITLE)),
        )
        .child(again)
        .into_any_element()
}

/// The room a card's rows have for their details: the column `width` less the card's and the
/// rows' insets, the glyph, the title's column and the gaps (DESIGN §4.3).
fn detail_room(width: Pixels, theme: &DvTheme) -> Pixels {
    width
        - theme.spacing.lg * 2.
        - px(4.)
        - theme.spacing.md * 2.
        - px(15.)
        - px(TITLE_WIDTH)
        - theme.spacing.md * 2.
}

/// The title column's width in a row.
const TITLE_WIDTH: f32 = 230.;

/// `detail` as it fits in `room`: whole, else its paths, longest first, cut by whole segments in
/// the middle (`elide_path_middle`) until it fits, so the sentence's start and each path's end
/// stay readable; what still doesn't fit truncates as it draws.
fn fit_detail(detail: &str, room: Pixels, window: &Window, theme: &DvTheme) -> String {
    let spec = &theme.text.secondary;
    let fits = |text: &str| text_width(window, text, spec) <= room;
    if fits(detail) {
        return detail.to_string();
    }
    // The longest path first, then the next, until the sentence fits.
    let mut paths: Vec<String> = detail
        .split_whitespace()
        .map(|word| word.trim_matches(|ch: char| matches!(ch, '(' | ')' | ',' | ';')))
        .filter(|word| word.contains(['\\', '/']))
        .map(str::to_string)
        .collect();
    paths.sort_by_key(|path| std::cmp::Reverse(path.len()));
    let mut shown = detail.to_string();
    for path in paths {
        if fits(&shown) {
            break;
        }
        let elided = elide_path_middle(&path, |candidate| {
            fits(&shown.replacen(path.as_str(), candidate, 1))
        });
        shown = shown.replacen(path.as_str(), &elided, 1);
    }
    shown
}

/// Every environment's card (module docs), or a line saying the first check is running, for a
/// column `width` wide.
pub fn environments(
    setup: &Entity<Setup>,
    theme: &DvTheme,
    width: Pixels,
    window: &Window,
    cx: &App,
) -> Vec<AnyElement> {
    let all = setup.read(cx).environments();
    if all.is_empty() {
        let c = &theme.colors;
        return vec![
            div()
                .py(theme.spacing.lg)
                .text_color(c.fg3)
                .child("Checking this machine…")
                .automation_target("setup/checking"),
        ];
    }
    let room = detail_room(width, theme);
    all.iter()
        .map(|state| card(state, setup, theme, room, window, cx))
        .collect()
}

/// One environment's card.
fn card(
    state: &EnvState,
    setup: &Entity<Setup>,
    theme: &DvTheme,
    room: Pixels,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let c = &theme.colors;
    let env = &state.env;
    let name = env.name.clone();
    let (glyph, place) = match (env.kind, state.here) {
        (Kind::Wsl, false) if env.running => (DvIcon::Terminal, "WSL · running"),
        (Kind::Wsl, false) => (DvIcon::Terminal, "WSL · stopped"),
        _ => (DvIcon::Monitor, "This PC"),
    };
    let dot = div().flex_none().size(px(7.)).rounded_full().map(|this| {
        if env.running {
            this.bg(c.green)
        } else {
            this.border_1().border_color(c.fg3)
        }
    });
    let start = (!env.running).then(|| {
        let setup = setup.clone();
        let distro = name.clone();
        small_button(
            SharedString::from(format!("setup-start-{name}")),
            if state.starting {
                "Starting…"
            } else {
                "Start"
            },
            theme,
        )
        .on_click(move |_, _, cx| setup.update(cx, |setup, cx| setup.start(&distro, cx)))
        .automation_target(format!("setup/{name}/start"))
    });
    let header = div()
        .flex()
        .items_center()
        .gap(theme.spacing.md)
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(px(30.))
                .rounded(px(7.))
                .bg(c.blue_soft)
                .child(icon(glyph, px(16.), c.blue)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(name.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .text_spec(&theme.text.secondary)
                        .text_color(c.fg2)
                        .child(dot)
                        .child(place)
                        .when(state.checking, |this| {
                            this.child(div().text_color(c.fg3).child("· checking…"))
                        }),
                ),
        )
        .children(start);
    let mut lines: Vec<AnyElement> = Vec::new();
    if let Some(error) = &state.start_error {
        lines.push(
            div()
                .text_spec(&theme.text.secondary)
                .text_color(c.red)
                .child(format!("Couldn't start {name}: {error}"))
                .into_any_element(),
        );
    }
    if let Some(note) = &env.note {
        lines.push(
            div()
                .text_spec(&theme.text.secondary)
                .text_color(c.fg2)
                .child(note.clone())
                .automation_target(format!("setup/{name}/note")),
        );
    }
    let rows: Vec<AnyElement> = env
        .checks
        .iter()
        .enumerate()
        .map(|(index, check)| row(&name, check, index == 0, setup, theme, room, window, cx))
        .collect();
    div()
        .flex()
        .flex_col()
        .gap(theme.spacing.md)
        .p(theme.spacing.lg)
        .pt(theme.spacing.md)
        .rounded(theme.radii.card)
        .border_1()
        .border_color(c.line)
        .bg(c.layer)
        .child(header)
        .children(lines)
        .when(!rows.is_empty(), |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c.line)
                    .overflow_hidden()
                    .children(rows),
            )
        })
        .automation_target(format!("setup/{name}"))
}

/// The glyph and its colour for `state`.
fn state_glyph(state: State, theme: &DvTheme) -> (DvIcon, gpui::Hsla) {
    let c = &theme.colors;
    match state {
        State::Ok => (DvIcon::CircleCheck, c.green),
        State::Missing => (DvIcon::Circle, c.yellow),
        State::OutOfDate => (DvIcon::TriangleAlert, c.yellow),
        State::Failed => (DvIcon::CircleX, c.red),
        State::Skipped => (DvIcon::CircleMinus, c.fg3),
    }
}

/// One check's row (module docs).
#[allow(clippy::too_many_arguments)]
fn row(
    env: &str,
    check: &Check,
    first: bool,
    setup: &Entity<Setup>,
    theme: &DvTheme,
    room: Pixels,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let c = &theme.colors;
    let (glyph, color) = state_glyph(check.state, theme);
    let applying = setup.read(cx).applying(env, &check.id);
    // A fix's failure goes once its row reads ok.
    let failure = setup
        .read(cx)
        .failure(env, &check.id)
        .filter(|_| check.state.needs_fixing())
        .map(str::to_string);
    let secondary = &theme.text.secondary;
    let medium = crate::theme::TextSpec {
        weight: FontWeight::MEDIUM,
        ..secondary.clone()
    };
    // What the fix takes of the row, so the detail knows its room.
    let fix_width = match (&check.fix, applying) {
        _ if !check.state.needs_fixing() => px(0.),
        (_, Some(applying)) => text_width(window, &progress_text(applying), secondary),
        (Some(fix), None) => match (&fix.action, &fix.command) {
            (Some(action), _) => text_width(window, action, &medium) + theme.spacing.md * 2.,
            (None, Some(command)) => {
                (text_width(window, command, &theme.text.inline_code) + px(12.)).min(px(250.))
                    + theme.sizes.control_small_height
                    + theme.spacing.sm
            }
            (None, None) => px(0.),
        },
        (None, None) => px(0.),
    } + theme.spacing.md;
    let detail = fit_detail(&check.detail, room - fix_width, window, theme);
    let target = format!("setup/{env}/{}", check.id);
    let fix: Option<AnyElement> = match (&check.fix, applying) {
        _ if !check.state.needs_fixing() => None,
        (_, Some(applying)) => Some(
            div()
                .flex_none()
                .text_spec(&theme.text.secondary)
                .text_color(c.fg3)
                .child(progress_text(applying))
                .into_any_element(),
        ),
        (Some(fix), None) => match (&fix.action, &fix.command) {
            (Some(action), _) => {
                let setup = setup.clone();
                let (env, id) = (env.to_string(), check.id.clone());
                Some(
                    small_button(
                        SharedString::from(format!("setup-fix-{env}-{id}")),
                        action.clone(),
                        theme,
                    )
                    .on_click(move |_, _, cx| {
                        setup.update(cx, |setup, cx| setup.apply(&env, &id, cx))
                    })
                    .automation_target(format!("{target}/fix")),
                )
            }
            (None, Some(command)) => Some(command_to_copy(command, &target, theme)),
            (None, None) => None,
        },
        (None, None) => None,
    };
    let detail_color = match check.state {
        State::Failed => c.red,
        _ => c.fg2,
    };
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .px(theme.spacing.md)
        .py(px(7.))
        .when(!first, |this| this.border_t_1().border_color(c.line))
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.spacing.md)
                .min_h(px(24.))
                .child(icon(glyph, px(15.), color))
                .child(
                    div()
                        .flex_none()
                        .w(px(TITLE_WIDTH))
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .child(check.title.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_spec(&theme.text.secondary)
                        .text_color(detail_color)
                        .child(detail),
                )
                .children(fix),
        )
        .children(failure.map(|failure| {
            div()
                .pl(px(25.))
                .text_spec(&theme.text.secondary)
                .text_color(c.red)
                .child(format!("Didn't work: {failure}"))
        }))
        .automation_target(target)
}

/// "Installing 3.1 / 7.4 MB…", or "Working…" before an install reports its bytes.
fn progress_text(applying: Applying) -> String {
    if applying.total == 0 {
        "Working…".to_string()
    } else {
        format!(
            "Installing {:.1} / {:.1} MB…",
            applying.done as f64 / 1_000_000.0,
            applying.total as f64 / 1_000_000.0
        )
    }
}

/// The command a check's fix is, to run by hand, with a copy button.
fn command_to_copy(command: &str, target: &str, theme: &DvTheme) -> AnyElement {
    let text = command.to_string();
    div()
        .flex_none()
        .max_w(px(280.))
        .flex()
        .items_center()
        .gap(theme.spacing.sm)
        // As `inline_code`, cut with an ellipsis when it is long; the copy is whole.
        .child(
            div()
                .min_w_0()
                .px(px(5.))
                .py(px(2.))
                .rounded(theme.radii.small)
                .bg(theme.colors.icode_bg)
                .text_color(theme.colors.fg)
                .text_spec(&theme.text.inline_code)
                .truncate()
                .child(command.to_string()),
        )
        .child(
            small_icon_button(
                SharedString::from(format!("{target}-copy")),
                DvIcon::Copy,
                theme,
            )
            .tooltip(tooltip("Copy the command"))
            .on_click(move |_, _, cx| crate::system::write_clipboard(text.clone(), cx))
            .automation_target(format!("{target}/copy")),
        )
        .into_any_element()
}

/// The first-run screen (DESIGN §4.3) the shell shows in place of its columns.
pub mod first_run {
    use gpui::{
        AnyElement, App, Entity, FocusHandle, InteractiveElement as _, ParentElement as _,
        StatefulInteractiveElement as _, Styled as _, div, px,
    };

    use crate::automation::AutomationTargetExt as _;
    use crate::components::{button, kbd_on_accent, primary_button};
    use crate::keymap;
    use crate::services::setup::{FirstRunEnd, Setup};
    use crate::theme::{DvTheme, StyledText as _};

    /// "Set up dv", the environments, and the footer with **Skip for now** and **Continue
    /// `Enter`**, over the panel surface (module docs of `super`). `focus` has the keyboard
    /// while it shows, its key context [`keymap::FIRST_RUN`] binding `Enter`.
    pub fn view(
        setup: &Entity<Setup>,
        focus: &FocusHandle,
        theme: &DvTheme,
        window: &gpui::Window,
        cx: &App,
    ) -> AnyElement {
        let c = &theme.colors;
        let width =
            window.viewport_size().width.min(px(super::COLUMN_WIDTH)) - theme.spacing.xl * 2.;
        let finish = |how: FirstRunEnd| {
            let setup = setup.clone();
            move |_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut App| {
                setup.update(cx, |setup, cx| setup.finish_first_run(how, cx))
            }
        };
        let footer = div()
            .flex()
            .items_center()
            .gap(theme.spacing.md)
            .mt(theme.spacing.xl)
            .pt(px(14.))
            .border_t_1()
            .border_color(c.line)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_spec(&theme.text.secondary)
                    .text_color(c.fg2)
                    .child("Change any of this later in Settings."),
            )
            .child(
                button("setup-skip", theme)
                    .child("Skip for now")
                    .on_click(finish(FirstRunEnd::Skip))
                    .automation_target("setup/skip"),
            )
            .child(
                primary_button("setup-continue", true, theme)
                    .child("Continue")
                    .child(kbd_on_accent(keymap::label("enter"), theme))
                    .on_click(finish(FirstRunEnd::Continue))
                    .automation_target("setup/continue"),
            );
        div()
            .id("first-run")
            .key_context(keymap::FIRST_RUN)
            .track_focus(focus)
            .flex_1()
            .min_h_0()
            .w_full()
            .bg(c.panel)
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(super::COLUMN_WIDTH))
                    .px(theme.spacing.xl)
                    .pt(px(30.))
                    .pb(theme.spacing.xxl)
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.md)
                    .child(div().mb(px(10.)).child(super::heading(
                        "Set up dv",
                        &theme.text.display,
                        setup,
                        theme,
                        cx,
                    )))
                    .children(super::environments(setup, theme, width, window, cx))
                    .child(footer),
            )
            .automation_target("setup")
    }
}
