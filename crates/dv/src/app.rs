//! GUI startup: the data dir, then gpui with dv's assets, fonts, services and theme, the main
//! window and, with `--automation`, the automation driver.

use std::{cell::Cell, path::PathBuf, process::ExitCode, rc::Rc};

use anyhow::{Result, bail};
use dv_ui::platform::MaterialSupport;
use dv_ui::services::ServicesConfig;
use dv_ui::shell::MainWindow;
use dv_ui::system::SystemShell;
use gpui::{App, QuitMode, px, size};

use crate::cli::{Cli, WindowSize};
use crate::data_dir::Source;

/// Starts the app, blocks until it quits and returns the process exit code.
///
/// Errors before the app starts are returned. A failure during startup inside the app is logged
/// where it happens and reported as [`ExitCode::FAILURE`]: `Application::run` can't hand an
/// error back, and on macOS it never returns after a quit.
pub fn run(cli: Cli) -> Result<ExitCode> {
    let automation = cli.automation_enabled();
    if automation && !cfg!(feature = "automation") {
        bail!("--automation and --automation-pipe need a build with the `automation` feature");
    }

    let place = crate::data_dir::resolve(cli.data_dir.clone(), automation)?;
    let data_dir = crate::data_dir::prepare(&place)?;
    log::info!(
        "data directory: {} ({})",
        data_dir.display(),
        place.source.as_str()
    );
    let temporary = (place.source == Source::Automation).then(|| data_dir.clone());
    let requested = cli.window_size.unwrap_or(WindowSize::DEFAULT);
    let window_size = requested.at_least_min();
    if window_size != requested {
        log::warn!("--window-size {requested} is below the minimum; using {window_size}");
    }
    let startup = Startup {
        data_dir,
        automation,
        automation_pipe: cli.automation_pipe.clone(),
        background: cli.automation_background,
        window_size,
    };

    let startup_failed = Rc::new(Cell::new(false));
    gpui_platform::application()
        .with_assets(dv_ui::Assets)
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run({
            let startup_failed = startup_failed.clone();
            move |cx| start_or_quit(&startup_failed, cx, |cx| start(startup, cx))
        });
    // An automation run's temporary data dir goes with it; everything in it was written before
    // the event loop ended. (On macOS `run` never returns and the OS's temp cleanup takes it.)
    if let Some(dir) = temporary
        && let Err(error) = std::fs::remove_dir_all(&dir)
    {
        log::warn!("couldn't remove {}: {error}", dir.display());
    }
    Ok(if startup_failed.get() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Runs `start`. On failure logs the error, sets `failed` and quits the app, so [`run`] can exit
/// non-zero once the event loop ends.
fn start_or_quit(failed: &Cell<bool>, cx: &mut App, start: impl FnOnce(&mut App) -> Result<()>) {
    if let Err(error) = start(cx) {
        log::error!("startup failed: {error:#}");
        failed.set(true);
        cx.quit();
    }
}

/// Everything [`start`] needs from the command line.
struct Startup {
    data_dir: PathBuf,
    automation: bool,
    automation_pipe: Option<String>,
    /// `--automation-background`.
    background: bool,
    window_size: WindowSize,
}

fn start(startup: Startup, cx: &mut App) -> Result<()> {
    let Startup {
        data_dir,
        automation,
        automation_pipe,
        background,
        window_size,
    } = startup;
    dv_ui::init(cx)?;
    // A scripted run opens no browser, Explorer window or folder picker: it records them.
    dv_ui::system::install(
        if automation {
            SystemShell::scripted()
        } else {
            SystemShell::Real
        },
        cx,
    );
    dv_ui::services::init(ServicesConfig { data_dir }, cx);
    // The settings' material and mode, before the first frame.
    dv_ui::appearance::init(MaterialSupport::probe(), cx);

    set_background(background);
    let main = dv_ui::shell::open_main_window(
        size(px(window_size.width as f32), px(window_size.height as f32)),
        cx,
    )?;
    if automation {
        start_automation(main, automation_pipe, cx)?;
    }
    // A background run must never take the foreground from the person at the machine.
    if !dv_ui::automation::is_background() {
        cx.activate(true);
    }
    Ok(())
}

#[cfg(feature = "automation")]
fn set_background(background: bool) {
    gpui_automation::set_background(background);
}

#[cfg(not(feature = "automation"))]
fn set_background(_background: bool) {
    // `--automation-background` needs `--automation`, which `run` refuses in this build before
    // the app starts.
}

#[cfg(feature = "automation")]
fn start_automation(main: MainWindow, pipe: Option<String>, cx: &mut App) -> Result<()> {
    crate::automation::start(main.handle.into(), main.shell, pipe, cx)
}

#[cfg(not(feature = "automation"))]
fn start_automation(_main: MainWindow, _pipe: Option<String>, _cx: &mut App) -> Result<()> {
    // `run` refuses `--automation` in this build before the app starts.
    Ok(())
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn a_failed_start_is_recorded(cx: &mut TestAppContext) {
        let failed = Cell::new(false);
        cx.update(|cx| start_or_quit(&failed, cx, |_| anyhow::bail!("no fonts")));
        assert!(failed.get());
    }

    #[gpui::test]
    fn a_clean_start_records_no_failure(cx: &mut TestAppContext) {
        let failed = Cell::new(false);
        let mut ran = false;
        cx.update(|cx| {
            start_or_quit(&failed, cx, |_| {
                ran = true;
                Ok(())
            })
        });
        assert!(ran);
        assert!(!failed.get());
    }
}
