//! dv's automation host (PLAN §14): the `state` document e2e scenarios assert on, the `settled`
//! predicate and dv's own commands. Built only with the `automation` feature and started only
//! with `--automation` / `--automation-pipe`. `tests/e2e/README.md` "dv's `state`" documents
//! every key; `gpui_automation` adds `window` and `windows` itself.
//!
//! `state`:
//! - `app`: `"dv"`;
//! - `data_dir`: the data dir this run uses;
//! - `settings`: `material` and `mode` as set, `resolved_material` and `resolved_mode` as on
//!   screen, `available_materials` (what the platform can show now) and `path` (the settings
//!   file);
//! - `shell`: `panel_visible`, `empty_state` (`"no_review"` or `null`) and `window_material`
//!   (the material last applied to the window);
//! - `system`: what a scripted run was asked to open (`requests`, oldest first) and the picker
//!   answers still queued (`answers_left`).
//!
//! Commands: `set_setting {key, value}` changes `material` (`mica`, `acrylic`, `opaque`) or
//! `mode` (`system`, `dark`, `light`) through the settings service, exactly as the UI would: it
//! saves to the data dir and the window repaints in the new theme before the reply.

use anyhow::{Context as _, Result, anyhow, bail};
use dv_ui::services::Mode;
use dv_ui::system::SystemRequest;
use dv_ui::{ActiveServices as _, Material, Shell};
use gpui::{App, AppContext as _, Context, Entity, Window};
use gpui_automation::AutomationHost;
use serde_json::{Value, json};

/// The entity `gpui_automation` drives. It holds the shell it describes, no state of its own.
pub struct DvAutomation {
    shell: Entity<Shell>,
}

impl DvAutomation {
    pub fn new(shell: Entity<Shell>) -> Self {
        Self { shell }
    }
}

impl AutomationHost for DvAutomation {
    fn state(&self, _window: &Window, cx: &App) -> Value {
        json!({
            "app": "dv",
            "data_dir": cx.services().data_dir().display().to_string(),
            "settings": settings_state(cx),
            "shell": shell_state(self.shell.read(cx)),
            "system": system_state(cx),
        })
    }

    fn settled(&self, cx: &App) -> bool {
        cx.services().is_settled(cx)
    }

    fn custom(
        &mut self,
        cmd: &str,
        args: Value,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Result<Value>> {
        match cmd {
            "set_setting" => Some(set_setting(&args, cx)),
            _ => None,
        }
    }
}

/// `state.settings`.
fn settings_state(cx: &App) -> Value {
    let settings = cx.services().settings.read(cx);
    let resolved = dv_ui::appearance::resolved(cx);
    let available: Vec<&str> = dv_ui::appearance::support(cx)
        .available()
        .into_iter()
        .map(Material::key)
        .collect();
    json!({
        "material": settings.material().key(),
        "mode": settings.mode().key(),
        "resolved_material": resolved.material.key(),
        "resolved_mode": resolved.appearance.key(),
        "available_materials": available,
        "path": settings.path().display().to_string(),
        "save_error": settings.last_error(),
    })
}

/// `state.shell`.
fn shell_state(shell: &Shell) -> Value {
    json!({
        "panel_visible": shell.panel_visible(),
        "empty_state": shell.empty_state().map(|empty| empty.key()),
        "window_material": shell.applied_material().key(),
    })
}

/// `state.system`.
fn system_state(cx: &App) -> Value {
    let (requests, answers_left) = dv_ui::system::requests(cx);
    let requests: Vec<Value> = requests
        .iter()
        .map(|request| match request {
            SystemRequest::OpenUrl(url) => json!({"kind": request.kind(), "url": url}),
            SystemRequest::Reveal(path) => {
                json!({"kind": request.kind(), "path": path.display().to_string()})
            }
            SystemRequest::PickFolder { prompt } => {
                json!({"kind": request.kind(), "prompt": prompt.as_ref()})
            }
        })
        .collect();
    json!({"requests": requests, "answers_left": answers_left})
}

/// A `set_setting` request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingChange {
    Material(Material),
    Mode(Mode),
}

impl SettingChange {
    fn parse(args: &Value) -> Result<Self> {
        let key = args
            .get("key")
            .and_then(Value::as_str)
            .context("set_setting needs \"key\": \"material\" or \"mode\"")?;
        let value = args
            .get("value")
            .and_then(Value::as_str)
            .with_context(|| format!("set_setting {key:?} needs a string \"value\""))?;
        match key {
            "material" => Material::from_key(value)
                .map(Self::Material)
                .ok_or_else(|| {
                    anyhow!("material must be \"mica\", \"acrylic\" or \"opaque\", not {value:?}")
                }),
            "mode" => Mode::from_key(value).map(Self::Mode).ok_or_else(|| {
                anyhow!("mode must be \"system\", \"dark\" or \"light\", not {value:?}")
            }),
            other => bail!("unknown setting {other:?}; dv has \"material\" and \"mode\""),
        }
    }
}

/// `set_setting {key, value}`: the change through the settings service; replies with
/// `state.settings` after it.
fn set_setting(args: &Value, cx: &mut Context<DvAutomation>) -> Result<Value> {
    let change = SettingChange::parse(args)?;
    let settings = cx.services().settings.clone();
    settings.update(cx, |settings, cx| match change {
        SettingChange::Material(material) => settings.set_material(material, cx),
        SettingChange::Mode(mode) => settings.set_mode(mode, cx),
    });
    Ok(json!({"settings": settings_state(cx)}))
}

/// Starts automation on the main window.
pub fn start(
    window: gpui::AnyWindowHandle,
    shell: Entity<Shell>,
    pipe: Option<String>,
    cx: &mut App,
) -> Result<()> {
    let host = cx.new(|_| DvAutomation::new(shell));
    let options = gpui_automation::Options {
        pipe,
        ..Default::default()
    };
    gpui_automation::start(window, host, cx, options)
}

#[cfg(test)]
mod tests {
    use dv_ui::platform::MaterialSupport;
    use dv_ui::services::ServicesConfig;
    use dv_ui::system::SystemShell;
    use gpui::{TestAppContext, VisualTestContext};
    use pretty_assertions::assert_eq;

    use super::*;

    fn open(
        cx: &mut TestAppContext,
    ) -> (
        tempfile::TempDir,
        Entity<DvAutomation>,
        &mut VisualTestContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().to_path_buf();
        cx.update(|cx| {
            dv_ui::init(cx).unwrap();
            dv_ui::system::install(SystemShell::scripted(), cx);
            dv_ui::services::init(ServicesConfig { data_dir }, cx);
            dv_ui::appearance::init(MaterialSupport::ALL, cx);
        });
        let (shell, cx) = cx.add_window_view(Shell::new);
        let host = cx.update(|_, cx| cx.new(|_| DvAutomation::new(shell)));
        (dir, host, cx)
    }

    fn state(host: &Entity<DvAutomation>, cx: &mut VisualTestContext) -> Value {
        cx.update(|window, cx| host.read(cx).state(window, cx))
    }

    #[test]
    fn setting_changes_parse() {
        let parse = |args: Value| SettingChange::parse(&args);
        assert_eq!(
            parse(json!({"key": "material", "value": "acrylic"})).unwrap(),
            SettingChange::Material(Material::Acrylic)
        );
        assert_eq!(
            parse(json!({"key": "mode", "value": "light"})).unwrap(),
            SettingChange::Mode(Mode::Light)
        );
        for (args, error) in [
            (json!({}), "needs \"key\""),
            (json!({"key": "material"}), "needs a string \"value\""),
            (
                json!({"key": "material", "value": "glass"}),
                "material must be",
            ),
            (json!({"key": "mode", "value": "auto"}), "mode must be"),
            (json!({"key": "theme", "value": "dark"}), "unknown setting"),
        ] {
            let message = parse(args.clone()).unwrap_err().to_string();
            assert!(message.contains(error), "{args}: {message}");
        }
    }

    #[gpui::test]
    fn state_describes_the_empty_shell(cx: &mut TestAppContext) {
        let (dir, host, cx) = open(cx);
        let state = state(&host, cx);
        assert_eq!(state["app"], "dv");
        assert_eq!(state["data_dir"], dir.path().display().to_string());
        assert_eq!(
            state["shell"],
            json!({
                "panel_visible": true,
                "empty_state": "no_review",
                "window_material": Material::platform_default().key(),
            })
        );
        assert_eq!(state["settings"]["mode"], "system");
        assert_eq!(
            state["settings"]["available_materials"],
            json!(["mica", "acrylic", "opaque"])
        );
        assert_eq!(state["system"], json!({"requests": [], "answers_left": 0}));
        assert!(cx.update(|_, cx| host.read(cx).settled(cx)));
    }

    #[gpui::test]
    fn set_setting_changes_the_theme_and_saves(cx: &mut TestAppContext) {
        let (dir, host, cx) = open(cx);
        let reply = cx.update(|window, cx| {
            host.update(cx, |host, cx| {
                host.custom(
                    "set_setting",
                    json!({"key": "material", "value": "acrylic"}),
                    window,
                    cx,
                )
            })
        });
        let reply = reply.expect("a dv command").unwrap();
        assert_eq!(reply["settings"]["material"], "acrylic");
        assert_eq!(reply["settings"]["resolved_material"], "acrylic");
        cx.update(|window, cx| {
            host.update(cx, |host, cx| {
                host.custom(
                    "set_setting",
                    json!({"key": "mode", "value": "light"}),
                    window,
                    cx,
                )
            })
        })
        .expect("a dv command")
        .unwrap();
        cx.run_until_parked();

        let state = state(&host, cx);
        assert_eq!(state["settings"]["resolved_mode"], "light");
        assert_eq!(state["shell"]["window_material"], "acrylic");
        assert!(cx.update(|_, cx| host.read(cx).settled(cx)));
        let saved: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            saved,
            json!({"v": 1, "material": "acrylic", "mode": "light"})
        );

        // Unknown commands are the driver's to report.
        let unknown = cx.update(|window, cx| {
            host.update(cx, |host, cx| host.custom("nope", json!({}), window, cx))
        });
        assert!(unknown.is_none());
    }
}
