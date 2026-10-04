//! App-wide services (PLAN §11), each a gpui entity shared by every window, installed once as
//! the [`Services`] global by [`init`] before the first window opens. Views reach them through
//! [`ActiveServices::services`].
//!
//! - [`Settings`] (`settings`): the window material and the light/dark mode, saved in the data
//!   dir as they change.
//!
//! The `System` seam (opening URLs, revealing files, the folder picker) is the separate
//! [`crate::system`] global, because automation swaps it for a recorder.

pub mod settings;

use std::path::{Path, PathBuf};

use gpui::{App, AppContext as _, Entity, Global};

pub use settings::{Mode, Settings, SettingsEvent, SettingsFile};

/// What [`init`] needs from the binary.
#[derive(Clone, Debug)]
pub struct ServicesConfig {
    /// Where dv keeps its files (`--data-dir`, `DV_DATA_DIR`, else the platform's; PLAN §13.3).
    pub data_dir: PathBuf,
}

/// The app's services.
pub struct Services {
    pub settings: Entity<Settings>,
    data_dir: PathBuf,
}

impl Global for Services {}

impl Services {
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Nothing is loading or waiting to be saved: automation's `settled`.
    pub fn is_settled(&self, cx: &App) -> bool {
        self.settings.read(cx).is_settled()
    }
}

/// Builds the services (reading the settings synchronously) and installs them as a global.
pub fn init(config: ServicesConfig, cx: &mut App) {
    let ServicesConfig { data_dir } = config;
    let path = data_dir.join(settings::SETTINGS_FILE);
    let settings = cx.new(|cx| Settings::load(path, cx));
    cx.set_global(Services { settings, data_dir });
}

/// Access to the installed [`Services`].
pub trait ActiveServices {
    /// The app's services. Panics if [`init`] hasn't run, which is a startup bug.
    fn services(&self) -> &Services;

    /// The services, if installed.
    fn try_services(&self) -> Option<&Services>;
}

impl ActiveServices for App {
    fn services(&self) -> &Services {
        self.global::<Services>()
    }

    fn try_services(&self) -> Option<&Services> {
        self.try_global::<Services>()
    }
}
