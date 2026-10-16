//! Where a spec's resolvers can be told about a server from outside the project (docs/PLAN.md
//! §18.4, §18.7): `settings.json`'s `lsp.<id>.command`, and the `DV_LSP_<ID>` override tests
//! and scenarios use, as `DV_GH` replaces `gh`.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::spec::LanguageServerSpec;

/// `settings.json`'s `lsp` object, read leniently: a value of the wrong shape is ignored, as
/// the rest of dv's settings are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LspSettings {
    /// `lsp.<id>.command` by spec id: the program and its arguments.
    commands: BTreeMap<String, Vec<String>>,
}

impl LspSettings {
    /// The settings in `lsp`'s value (`{"typescript": {"command": ["/x/tsc", "--lsp",
    /// "--stdio"]}}`).
    pub fn from_json(lsp: &Value) -> Self {
        let mut commands = BTreeMap::new();
        if let Some(servers) = lsp.as_object() {
            for (id, server) in servers {
                let Some(command) = server.get("command").and_then(Value::as_array) else {
                    continue;
                };
                let argv: Option<Vec<String>> = command
                    .iter()
                    .map(|arg| arg.as_str().map(str::to_string))
                    .collect();
                match argv {
                    Some(argv) if !argv.is_empty() => {
                        commands.insert(id.clone(), argv);
                    }
                    _ => log::warn!("settings: lsp.{id}.command isn't a list of strings"),
                }
            }
        }
        Self { commands }
    }

    /// `lsp.<id>.command` set directly, for tests and the app's own settings.
    pub fn with_command(mut self, id: &str, argv: Vec<String>) -> Self {
        self.commands.insert(id.to_string(), argv);
        self
    }

    /// The command configured for `id`: the program and its arguments.
    pub fn command(&self, id: &str) -> Option<&[String]> {
        self.commands.get(id).map(Vec::as_slice)
    }
}

/// `DV_LSP_<ID>` (and `DV_LSP_<ID>_ARGS`) from the environment: the server to run in place of
/// whatever the resolvers would find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    /// The variable it came from.
    pub var: String,
    /// The executable, an absolute path where the repository lives.
    pub program: String,
    /// `DV_LSP_<ID>_ARGS`, a JSON array of strings; `None` runs it with the spec's arguments.
    pub args: Option<Vec<String>>,
}

/// Why `DV_LSP_<ID>` can't be used; the server shows as missing, saying so.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OverrideError {
    #[error("{var} names {program}, which isn't an absolute path")]
    NotAbsolute { var: String, program: String },
    #[error("{var}_ARGS isn't a JSON array of strings: {value}")]
    BadArgs { var: String, value: String },
}

impl Override {
    /// The override for `spec` in an environment read through `env`: `None` when
    /// `DV_LSP_<ID>` is unset or empty, an error when it isn't an absolute path or
    /// `DV_LSP_<ID>_ARGS` isn't a JSON array of strings.
    pub fn from_env(
        spec: &LanguageServerSpec,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Option<Override>, OverrideError> {
        let var = spec.override_var();
        let Some(program) = env(&var).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        if !crate::resolve::is_absolute(&program) {
            return Err(OverrideError::NotAbsolute { var, program });
        }
        let args = match env(&format!("{var}_ARGS")).filter(|value| !value.trim().is_empty()) {
            None => None,
            Some(value) => Some(serde_json::from_str::<Vec<String>>(&value).map_err(|_| {
                OverrideError::BadArgs {
                    var: var.clone(),
                    value,
                }
            })?),
        };
        Ok(Some(Override { var, program, args }))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::languages::typescript;

    #[test]
    fn commands_are_read_leniently() {
        let settings = LspSettings::from_json(&json!({
            "typescript": {"command": ["/opt/tsc", "--lsp", "--stdio"]},
            "css": {"command": "vscode-css-language-server --stdio"},
            "tailwindcss": {"command": []},
            "runtime": "bun"
        }));
        assert_eq!(
            settings.command("typescript"),
            Some(&["/opt/tsc".to_string(), "--lsp".into(), "--stdio".into()][..])
        );
        assert_eq!(settings.command("css"), None);
        assert_eq!(settings.command("tailwindcss"), None);
        assert_eq!(LspSettings::from_json(&json!(null)), LspSettings::default());
    }

    #[test]
    fn the_override_and_its_arguments() {
        let spec = &typescript::SPEC;
        assert_eq!(spec.override_var(), "DV_LSP_TYPESCRIPT");
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                vars.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            }
        };
        assert_eq!(Override::from_env(spec, &env(&[])), Ok(None));
        assert_eq!(
            Override::from_env(spec, &env(&[("DV_LSP_TYPESCRIPT", "")])),
            Ok(None)
        );
        assert_eq!(
            Override::from_env(spec, &env(&[("DV_LSP_TYPESCRIPT", "/bin/mock_lsp")])),
            Ok(Some(Override {
                var: "DV_LSP_TYPESCRIPT".into(),
                program: "/bin/mock_lsp".into(),
                args: None
            }))
        );
        assert_eq!(
            Override::from_env(
                spec,
                &env(&[
                    ("DV_LSP_TYPESCRIPT", "/bin/mock_lsp"),
                    ("DV_LSP_TYPESCRIPT_ARGS", r#"["--scenario", "a b.json"]"#)
                ])
            )
            .map(|found| found.and_then(|found| found.args)),
            Ok(Some(vec!["--scenario".to_string(), "a b.json".into()]))
        );
        assert_eq!(
            Override::from_env(
                spec,
                &env(&[
                    ("DV_LSP_TYPESCRIPT", "/bin/mock_lsp"),
                    ("DV_LSP_TYPESCRIPT_ARGS", "--scenario x")
                ])
            ),
            Err(OverrideError::BadArgs {
                var: "DV_LSP_TYPESCRIPT".into(),
                value: "--scenario x".into()
            })
        );
        let relative = Override::from_env(spec, &env(&[("DV_LSP_TYPESCRIPT", "mock_lsp")]));
        assert_eq!(
            relative,
            Err(OverrideError::NotAbsolute {
                var: "DV_LSP_TYPESCRIPT".into(),
                program: "mock_lsp".into()
            })
        );
        assert_eq!(
            relative.unwrap_err().to_string(),
            "DV_LSP_TYPESCRIPT names mock_lsp, which isn't an absolute path"
        );
    }
}
