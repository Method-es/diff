//! Scenario files: JSONL where each line is an automation command, possibly
//! carrying runner-only assertion keys, or a runner-only step (reading the
//! `mock_gh` or `mock_lsp` log, or restarting the app). See tests/e2e/README.md.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_automation::matcher::{Expectation, compact};
use serde::Deserialize;
use serde_json::{Map, Value};

/// Keys the runner consumes; they are never sent to the app.
const RUNNER_KEYS: &[&str] = &[
    "expect",
    "expect_result",
    "expect_error",
    "runner_timeout_ms",
    "save",
];

/// Variables the runner defines itself; `save` can't take their names.
pub const BUILTIN_VARS: &[&str] = &[
    "data_dir",
    "out_dir",
    "workspace",
    "dv_gh",
    "dv_mock_gh",
    "dv_mock_gh_log",
    "dv_mock_lsp",
    "dv_mock_lsp_log",
];

/// Default window size, matching dv's default window.
pub const DEFAULT_WINDOW_SIZE: &str = "1440x900";

/// dv's data dir (docs/PLAN.md §13.3). The runner sets it to the scenario's
/// data dir, as well as passing `--data-dir`, so no scenario reaches the
/// owner's `%APPDATA%\dv` (PLAN §14.1).
pub const DATA_DIR_VAR: &str = "DV_DATA_DIR";

/// What dv runs in place of `gh` (PLAN D7). The runner always points it at
/// `mock_gh`, so no scenario can reach GitHub.
pub const GH_VAR: &str = "DV_GH";

/// The `mock_gh` scenario file (PLAN §14.2): the header's `gh`, unset
/// without one.
pub const MOCK_GH_VAR: &str = "DV_MOCK_GH";

/// Where `mock_gh` appends one JSON line per call; `gh_log` steps read it.
pub const MOCK_GH_LOG_VAR: &str = "DV_MOCK_GH_LOG";

/// What dv runs in place of a language server's resolvers (docs/PLAN.md §18.4): `DV_LSP_<ID>`,
/// with `DV_LSP_<ID>_ARGS`. The runner removes every inherited one, so a scenario reaches no
/// real language server unless its header's `lsp` (or `env`) names one.
pub const LSP_VAR_PREFIX: &str = "DV_LSP_";

/// `mock_lsp`'s own variables, which the runner removes too: it passes the scenario and the
/// log as arguments.
pub const MOCK_LSP_VAR_PREFIX: &str = "DV_MOCK_LSP";

/// The scratch home dv installs into and runs from inside a WSL distro (docs/PLAN.md §5.2).
/// The runner removes an inherited one, so a scenario reaches WSL only through dv's stand-in
/// unless its header's `env` sets it (an exploratory run against a real distro). The runner
/// passes the value on as it is: dv alone decides what it means (`dv_backend::wsl::sandbox`),
/// and anything but a scratch home leaves it on its stand-in.
pub const WSL_HOME_VAR: &str = "DV_WSL_HOME";

/// git stops looking for a repository at these directories. The runner sets
/// it to the scenario's output dir: the data dir (the app's working
/// directory) sits inside this checkout's `target/`, and a repository
/// lookup from there must never find dv's own checkout.
pub const GIT_CEILING_VAR: &str = "GIT_CEILING_DIRECTORIES";

/// Environment the runner sets itself; a header `env` may not override it.
const RUNNER_ENV: &[&str] = &[
    DATA_DIR_VAR,
    GH_VAR,
    MOCK_GH_VAR,
    MOCK_GH_LOG_VAR,
    GIT_CEILING_VAR,
    "E2E_OUT_DIR",
    "E2E_DATA_DIR",
];

/// The optional first line: `{"scenario": {...}}`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    /// Extra app arguments after the runner's own flags.
    #[serde(default)]
    pub app_args: Vec<String>,
    /// A `mock_gh` scenario file, relative to the scenario file: the runner
    /// points `DV_MOCK_GH` at it, so `mock_gh` answers the app's `gh` calls
    /// from it.
    pub gh: Option<String>,
    /// `WxH` in logical pixels (default 1440x900).
    pub window_size: Option<String>,
    /// Extra environment for the app (and the `mock_gh` it starts).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// `mock_lsp` scenario files by language server id (`{"typescript":
    /// "fixtures/lsp/hover.json"}`), relative to the scenario file: the runner points
    /// `DV_LSP_<ID>` at `mock_lsp` playing that file, logging to `${dv_mock_lsp_log}`.
    #[serde(default)]
    pub lsp: BTreeMap<String, String>,
    /// Free text shown in verbose output.
    pub description: Option<String>,
    /// Operating systems the scenario runs on, as `std::env::consts::OS`
    /// names; empty means all. Others skip it (platform-specific chrome).
    #[serde(default)]
    pub platforms: Vec<String>,
    /// Run this scenario's app in the foreground (focused, in front) even
    /// when the runner runs in the background: for behaviour gpui only has in
    /// an active window, such as focus and blur listeners or the caret and
    /// selection of text inputs.
    #[serde(default)]
    pub foreground: bool,
}

/// `std::env::consts::OS` values a header's `platforms` may name.
const PLATFORMS: &[&str] = &["windows", "linux", "macos"];

/// One line of a scenario.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// 1-based line number in the file, also used as the request id.
    pub line: usize,
    pub action: Action,
    /// Checked against a fresh `state` after the action. Kept as JSON
    /// (validated at load) so `${...}` variables can be substituted per run.
    pub expect: Option<Value>,
    /// Checked against the action's own result: a command's `result`, the
    /// `gh_log` calls, or a restart's `ready` event.
    pub expect_result: Option<Value>,
    /// The command must fail with an error containing this text.
    pub expect_error: Option<String>,
    /// Screenshot name, when the runner chooses the path.
    pub screenshot: Option<String>,
    /// Explicit reply timeout from `runner_timeout_ms`.
    pub timeout: Option<Duration>,
    /// `save`: variables to set from the action's result, as (name, JSON
    /// pointer) pairs; later lines use them as `${name}`.
    pub save: Vec<(String, String)>,
}

/// What a step does before its `expect` is checked.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// An automation command sent to the app (runner keys removed).
    Command(Map<String, Value>),
    /// `{"gh_log": {}}`: the calls `mock_gh` has logged so far, read by the
    /// runner from `DV_MOCK_GH_LOG`.
    GhLog,
    /// `{"lsp_log": {}}`: what every `mock_lsp` the scenario started has logged so far (the
    /// messages it read, its starts and crashes), read by the runner.
    LspLog,
    /// Quit the app (unless a `quit` step already did) and start it again
    /// with the same data dir, `mock_gh` scenario and environment.
    Restart(Restart),
    /// Nothing: the line only asserts on `state`.
    Check,
}

/// `{"gh_log": {}}`: no options yet; unknown ones are errors.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct GhLogOptions {}

/// `{"restart": {"app_args"?: [...], "data_dir"?: bool}}`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Restart {
    /// Replaces the header's `app_args` for this launch and every later one.
    pub app_args: Option<Vec<String>>,
    /// `false` launches without `--data-dir` (this and every later launch; `true` puts it back),
    /// so the app finds its data dir as a normal start does: through `DV_DATA_DIR`, which the
    /// runner always points at the scenario's own data dir.
    pub data_dir: Option<bool>,
}

impl Step {
    /// Expectations with the run's variables substituted.
    pub fn expectations(raw: &Option<Value>, vars: &[(&str, String)]) -> Result<Vec<Expectation>> {
        let Some(raw) = raw else {
            return Ok(Vec::new());
        };
        let mut value = raw.clone();
        substitute(&mut value, vars);
        Expectation::parse_list(&value).map_err(|err| anyhow!(err))
    }

    /// The automation command this step sends, if it sends one.
    pub fn cmd(&self) -> Option<&str> {
        self.request()
            .and_then(|request| request.get("cmd"))
            .and_then(Value::as_str)
    }

    pub fn request(&self) -> Option<&Map<String, Value>> {
        match &self.action {
            Action::Command(request) => Some(request),
            _ => None,
        }
    }

    /// The window selector this step's command addresses, reused for the
    /// `state` fetched to check `expect`.
    pub fn window(&self) -> Option<&Value> {
        self.request().and_then(|request| request.get("window"))
    }

    /// How long to wait for the reply: a base allowance plus whatever the
    /// command itself is asked to wait for.
    pub fn reply_timeout(&self, base: Duration) -> Duration {
        if let Some(timeout) = self.timeout {
            return timeout;
        }
        let Some(request) = self.request() else {
            return base;
        };
        let millis = |key: &str| request.get(key).and_then(Value::as_u64).unwrap_or(0);
        let own = millis("ms")
            .max(millis("timeout_ms"))
            .max(millis("hold_ms") + millis("step_ms") * millis("steps").max(1));
        base + Duration::from_millis(own)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    pub name: String,
    pub path: PathBuf,
    pub header: Header,
    pub steps: Vec<Step>,
}

impl Scenario {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .ok_or_else(|| anyhow!("{} has no file name", path.display()))?;
        Self::parse(&name, path, &text).with_context(|| format!("in {}", path.display()))
    }

    pub fn parse(name: &str, path: &Path, text: &str) -> Result<Self> {
        let mut header = None;
        let mut steps = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line = index + 1;
            let trimmed = raw.trim_start_matches('\u{feff}').trim();
            if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') {
                continue;
            }
            let value: Value = serde_json::from_str(trimmed)
                .with_context(|| format!("line {line}: invalid JSON"))?;
            let Value::Object(mut object) = value else {
                bail!("line {line}: expected a JSON object");
            };
            if let Some(scenario) = object.remove("scenario") {
                if !object.is_empty() || header.is_some() || !steps.is_empty() {
                    bail!("line {line}: the scenario header must be the first line, on its own");
                }
                header = Some(
                    serde_json::from_value::<Header>(scenario)
                        .with_context(|| format!("line {line}: bad scenario header"))?,
                );
                continue;
            }
            steps.push(parse_step(line, object).with_context(|| format!("line {line}"))?);
        }
        if steps.is_empty() {
            bail!("the scenario has no steps");
        }
        let header = header.unwrap_or_default();
        if let Some(size) = &header.window_size {
            gpui_automation::parse_window_size(size)?;
        }
        if header.gh.as_deref().is_some_and(|gh| gh.trim().is_empty()) {
            bail!("the scenario header's gh must name a mock_gh scenario file");
        }
        if let Some((id, _)) = header.lsp.iter().find(|(id, file)| {
            file.trim().is_empty()
                || id.is_empty()
                || !id
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        }) {
            bail!(
                "the scenario header's lsp maps a language server id (lower case) to a mock_lsp \
                 scenario file; {id:?} doesn't"
            );
        }
        if let Some(name) = header.env.keys().find(|name| {
            RUNNER_ENV
                .iter()
                .any(|owned| owned.eq_ignore_ascii_case(name))
        }) {
            bail!("the scenario header's env can't set {name}: the runner sets it");
        }
        if let Some(unknown) = header
            .platforms
            .iter()
            .find(|os| !PLATFORMS.contains(&os.as_str()))
        {
            bail!(
                "unknown platform {unknown:?} in the scenario header; expected one of {}",
                PLATFORMS.join(", ")
            );
        }
        check_step_order(&steps)?;
        Ok(Self {
            name: name.to_string(),
            path: path.to_path_buf(),
            header,
            steps,
        })
    }

    /// Whether the scenario runs on `os` (a `std::env::consts::OS` value).
    pub fn runs_on(&self, os: &str) -> bool {
        self.header.platforms.is_empty() || self.header.platforms.iter().any(|p| p == os)
    }

    pub fn window_size(&self) -> &str {
        self.header
            .window_size
            .as_deref()
            .unwrap_or(DEFAULT_WINDOW_SIZE)
    }

    /// The header's `gh` file, resolved against the scenario file's directory.
    pub fn gh_scenario(&self) -> Option<PathBuf> {
        let gh = self.header.gh.as_deref()?;
        Some(self.dir().join(gh))
    }

    /// The header's `lsp` files, by language server id, resolved against the scenario file's
    /// directory.
    pub fn lsp_scenarios(&self) -> Vec<(String, PathBuf)> {
        self.header
            .lsp
            .iter()
            .map(|(id, file)| (id.clone(), self.dir().join(file)))
            .collect()
    }

    /// The directory the scenario file is in.
    fn dir(&self) -> &Path {
        self.path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
    }
}

fn parse_step(line: usize, mut object: Map<String, Value>) -> Result<Step> {
    let expect = take_expectations(&mut object, "expect")?;
    let expect_result = take_expectations(&mut object, "expect_result")?;
    let expect_error = match object.remove("expect_error") {
        None => None,
        Some(Value::String(text)) => Some(text),
        Some(other) => bail!("expect_error must be a string, got {}", compact(&other)),
    };
    let timeout = match object.remove("runner_timeout_ms") {
        None => None,
        Some(value) => Some(Duration::from_millis(value.as_u64().ok_or_else(|| {
            anyhow!("runner_timeout_ms must be a non-negative integer")
        })?)),
    };
    let save = parse_save(object.remove("save"))?;
    if !save.is_empty() && expect_error.is_some() {
        bail!("save needs a result, and expect_error means there is none");
    }
    debug_assert!(RUNNER_KEYS.iter().all(|key| !object.contains_key(*key)));
    if expect_error.is_some() && expect_result.is_some() {
        bail!("expect_error and expect_result exclude each other");
    }

    let step = |action| Step {
        line,
        action,
        expect: expect.clone(),
        expect_result: expect_result.clone(),
        expect_error: expect_error.clone(),
        screenshot: None,
        timeout,
        save: save.clone(),
    };

    if let Some(options) = object.remove("gh_log") {
        ensure_alone(&object, "gh_log")?;
        if expect_error.is_some() {
            bail!("a gh_log step can't expect an error");
        }
        if timeout.is_some() {
            bail!("runner_timeout_ms doesn't apply to a gh_log step (it reads a file)");
        }
        serde_json::from_value::<GhLogOptions>(options).context("bad gh_log")?;
        return Ok(step(Action::GhLog));
    }
    if let Some(options) = object.remove("lsp_log") {
        ensure_alone(&object, "lsp_log")?;
        if expect_error.is_some() {
            bail!("an lsp_log step can't expect an error");
        }
        if timeout.is_some() {
            bail!("runner_timeout_ms doesn't apply to an lsp_log step (it reads a file)");
        }
        serde_json::from_value::<GhLogOptions>(options).context("bad lsp_log")?;
        return Ok(step(Action::LspLog));
    }
    if let Some(restart) = object.remove("restart") {
        ensure_alone(&object, "restart")?;
        if expect_error.is_some() {
            bail!("a restart can't expect an error");
        }
        if timeout.is_some() {
            bail!("runner_timeout_ms doesn't apply to a restart (it uses --ready-timeout-ms)");
        }
        let restart = serde_json::from_value(restart).context("bad restart")?;
        return Ok(step(Action::Restart(restart)));
    }
    if object.is_empty() {
        if expect.is_none() {
            bail!("a line needs a cmd, a gh_log, a restart or an expect");
        }
        if expect_error.is_some() || expect_result.is_some() || !save.is_empty() {
            bail!("expect_error, expect_result and save need a cmd, a gh_log or a restart");
        }
        return Ok(step(Action::Check));
    }

    let cmd = match object.get("cmd") {
        Some(Value::String(cmd)) if !cmd.is_empty() => cmd.clone(),
        Some(other) => bail!("cmd must be a non-empty string, got {}", compact(other)),
        None => bail!("missing \"cmd\""),
    };
    if object.contains_key("id") {
        bail!("don't set \"id\": the runner numbers requests by line");
    }
    let mut screenshot = None;
    if cmd == "screenshot" {
        match (object.remove("name"), object.contains_key("path")) {
            (Some(Value::String(name)), false) => screenshot = Some(sanitize_name(&name)?),
            (Some(_), true) => bail!("give a screenshot either a name or a path, not both"),
            (Some(other), false) => {
                bail!("screenshot name must be a string, got {}", compact(&other))
            }
            (None, true) => {}
            (None, false) => bail!("a screenshot needs a name (or an explicit path)"),
        }
    }
    if cmd == "quit" && expect.is_some() {
        bail!("quit ends the app, so there is no state to expect afterwards");
    }
    Ok(Step {
        screenshot,
        ..step(Action::Command(object))
    })
}

/// `{"name": "/json/pointer", ...}`: variable names are identifiers other
/// than the runner's own; pointers are RFC 6901 (`""` is the whole result).
fn parse_save(save: Option<Value>) -> Result<Vec<(String, String)>> {
    let Some(save) = save else {
        return Ok(Vec::new());
    };
    let Value::Object(map) = save else {
        bail!(
            "save must map variable names to JSON pointers, got {}",
            compact(&save)
        );
    };
    map.into_iter()
        .map(|(name, pointer)| {
            let valid_name = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !valid_name {
                bail!("save: {name:?} is not a variable name (letters, digits and _)");
            }
            if BUILTIN_VARS.contains(&name.as_str()) {
                bail!("save: ${{{name}}} is the runner's own variable");
            }
            match pointer {
                Value::String(pointer) if pointer.is_empty() || pointer.starts_with('/') => {
                    Ok((name, pointer))
                }
                other => bail!("save: {name} needs a JSON pointer, got {}", compact(&other)),
            }
        })
        .collect()
}

/// A runner-only step (`gh_log`, `restart`) takes no command arguments.
fn ensure_alone(object: &Map<String, Value>, key: &str) -> Result<()> {
    if object.contains_key("cmd") {
        bail!("{key} and cmd exclude each other: put them on separate lines");
    }
    if !object.is_empty() {
        let extra: Vec<&str> = object.keys().map(String::as_str).collect();
        bail!("unknown key(s) on a {key} line: {}", extra.join(", "));
    }
    Ok(())
}

/// Once the app has quit only a restart brings it back; `gh_log` steps
/// without `expect` may come in between (the log outlives the app).
fn check_step_order(steps: &[Step]) -> Result<()> {
    let mut quit = false;
    for step in steps {
        match &step.action {
            Action::Restart(_) => quit = false,
            Action::GhLog | Action::LspLog if step.expect.is_none() => {}
            _ if quit => bail!(
                "line {}: after quit only a restart (or a gh_log without expect) may follow",
                step.line
            ),
            _ => quit = step.cmd() == Some("quit"),
        }
    }
    Ok(())
}

fn take_expectations(object: &mut Map<String, Value>, key: &str) -> Result<Option<Value>> {
    match object.remove(key) {
        None => Ok(None),
        Some(value) => {
            Expectation::parse_list(&value).map_err(|err| anyhow!("{key}: {err}"))?;
            Ok(Some(value))
        }
    }
}

/// Screenshot names become file names, so keep them portable.
fn sanitize_name(name: &str) -> Result<String> {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let clean = clean.trim_matches(['-', '.']).to_string();
    if clean.is_empty() {
        bail!("screenshot name {name:?} has no usable characters");
    }
    Ok(clean)
}

/// Replace `${name}` variables (`${data_dir}`, `${out_dir}`, `${workspace}`,
/// the `mock_gh` ones and those set by `save`) in every string of a request,
/// so scenarios can refer to the per-run directories.
pub fn substitute(value: &mut Value, vars: &[(&str, String)]) {
    match value {
        Value::String(text) => {
            if text.contains("${") {
                for (name, replacement) in vars {
                    *text = text.replace(&format!("${{{name}}}"), replacement);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| substitute(item, vars)),
        Value::Object(map) => map.values_mut().for_each(|item| substitute(item, vars)),
        _ => {}
    }
}

/// Expand scenario arguments: files are taken as-is, directories contribute
/// their `*.jsonl` files, and `*`/`?` wildcards in the file-name part are
/// expanded here because Windows shells don't.
pub fn discover(args: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for arg in args {
        let text = arg.to_string_lossy();
        if text.contains(['*', '?']) {
            let dir = arg
                .parent()
                .filter(|dir| !dir.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let pattern = arg
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .ok_or_else(|| anyhow!("{text}: wildcards are only supported in file names"))?;
            if dir.to_string_lossy().contains(['*', '?']) {
                bail!("{text}: wildcards are only supported in file names");
            }
            let mut matched = list_dir(dir, |name| wildcard_match(&pattern, name))?;
            if matched.is_empty() {
                bail!("{text} matches no files");
            }
            found.append(&mut matched);
        } else if arg.is_dir() {
            let mut matched = list_dir(arg, |name| name.ends_with(".jsonl"))?;
            if matched.is_empty() {
                bail!("{} contains no .jsonl scenarios", arg.display());
            }
            found.append(&mut matched);
        } else if arg.is_file() {
            found.push(arg.clone());
        } else {
            bail!("{} does not exist", arg.display());
        }
    }
    found.dedup();
    Ok(found)
}

fn list_dir(dir: &Path, keep: impl Fn(&str) -> bool) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_file() && keep(&name) {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

/// `*` matches any run of characters, `?` exactly one. Case-insensitive,
/// like the Windows file system.
pub fn wildcard_match(pattern: &str, name: &str) -> bool {
    fn go(pattern: &[char], name: &[char]) -> bool {
        match pattern.split_first() {
            None => name.is_empty(),
            Some(('*', rest)) => (0..=name.len()).any(|skip| go(rest, &name[skip..])),
            Some(('?', rest)) => !name.is_empty() && go(rest, &name[1..]),
            Some((c, rest)) => name
                .split_first()
                .is_some_and(|(n, name)| n.eq_ignore_ascii_case(c) && go(rest, name)),
        }
    }
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    go(&pattern, &name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn parse(text: &str) -> Result<Scenario> {
        Scenario::parse("demo", Path::new("demo.jsonl"), text)
    }

    fn expectations(raw: &Option<Value>) -> Vec<Expectation> {
        Step::expectations(raw, &[]).unwrap()
    }

    #[test]
    fn parses_header_comments_and_steps() {
        let scenario = parse(
            r#"
// a comment
{"scenario": {"gh": "fixtures/gh.json", "window_size": "1080x1920", "app_args": ["--x"], "env": {"A": "1"}}}
# another comment
{"cmd": "wait_ready"}
{"cmd": "click", "target": "a", "expect": {"pointer": "/n", "equals": 1}}
{"expect": [{"pointer": "/n", "gte": 1}, {"pointer": "/m", "exists": false}]}
"#,
        )
        .unwrap();
        assert_eq!(scenario.header.gh.as_deref(), Some("fixtures/gh.json"));
        assert_eq!(scenario.window_size(), "1080x1920");
        assert_eq!(scenario.header.app_args, ["--x"]);
        assert_eq!(scenario.header.env.get("A").map(String::as_str), Some("1"));
        assert_eq!(scenario.steps.len(), 3);
        assert_eq!(scenario.steps[0].line, 5);
        assert_eq!(scenario.steps[0].cmd(), Some("wait_ready"));
        assert_eq!(expectations(&scenario.steps[1].expect).len(), 1);
        assert_eq!(
            Value::Object(scenario.steps[1].request().cloned().unwrap()),
            json!({"cmd": "click", "target": "a"}),
            "runner keys are stripped from the request"
        );
        assert_eq!(scenario.steps[2].action, Action::Check);
        assert_eq!(expectations(&scenario.steps[2].expect).len(), 2);
    }

    #[test]
    fn gh_files_resolve_beside_the_scenario() {
        let text = "{\"scenario\": {\"gh\": \"fixtures/gh/pr.json\"}}\n{\"cmd\": \"state\"}";
        let scenario = Scenario::parse("demo", Path::new("tests/e2e/demo.jsonl"), text).unwrap();
        assert_eq!(
            scenario.gh_scenario(),
            Some(Path::new("tests/e2e").join("fixtures/gh/pr.json"))
        );
        let bare = Scenario::parse("demo", Path::new("demo.jsonl"), text).unwrap();
        assert_eq!(
            bare.gh_scenario(),
            Some(Path::new(".").join("fixtures/gh/pr.json"))
        );
        assert_eq!(parse(r#"{"cmd": "state"}"#).unwrap().gh_scenario(), None);
        let err = parse("{\"scenario\": {\"gh\": \" \"}}\n{\"cmd\": \"state\"}").unwrap_err();
        assert!(
            format!("{err:#}").contains("must name a mock_gh scenario file"),
            "{err:#}"
        );
    }

    #[test]
    fn save_takes_names_and_pointers() {
        let scenario =
            parse(r#"{"cmd": "state", "save": {"review": "/reviews/0/id", "all": ""}}"#).unwrap();
        let mut save = scenario.steps[0].save.clone();
        save.sort();
        assert_eq!(
            save,
            [
                ("all".to_string(), String::new()),
                ("review".to_string(), "/reviews/0/id".to_string())
            ]
        );
        assert_eq!(
            scenario.steps[0].request().unwrap().get("save"),
            None,
            "never sent to the app"
        );
        for (line, error) in [
            (r#"{"cmd": "state", "save": ["x"]}"#, "must map"),
            (
                r#"{"cmd": "state", "save": {"a-b": "/x"}}"#,
                "not a variable name",
            ),
            (
                r#"{"cmd": "state", "save": {"data_dir": "/x"}}"#,
                "runner's own",
            ),
            (
                r#"{"cmd": "state", "save": {"dv_mock_gh_log": "/x"}}"#,
                "runner's own",
            ),
            (
                r#"{"cmd": "state", "save": {"x": "x"}}"#,
                "needs a JSON pointer",
            ),
            (
                r#"{"cmd": "fail", "expect_error": "x", "save": {"x": "/x"}}"#,
                "needs a result",
            ),
            (
                r#"{"expect": {"pointer": "", "exists": true}, "save": {"x": "/x"}}"#,
                "need a cmd",
            ),
        ] {
            let err = format!("{:#}", parse(line).unwrap_err());
            assert!(err.contains(error), "{line}: {err}");
        }
    }

    #[test]
    fn headers_can_ask_for_the_foreground() {
        let scenario =
            parse("{\"scenario\": {\"foreground\": true}}\n{\"cmd\": \"state\"}").unwrap();
        assert!(scenario.header.foreground);
        let scenario = parse("{\"cmd\": \"state\"}").unwrap();
        assert!(!scenario.header.foreground);
    }

    #[test]
    fn platforms_limit_where_a_scenario_runs() {
        let everywhere = parse(r#"{"cmd": "state"}"#).unwrap();
        assert!(everywhere.runs_on("windows") && everywhere.runs_on("linux"));
        let windows =
            parse("{\"scenario\": {\"platforms\": [\"windows\"]}}\n{\"cmd\": \"state\"}").unwrap();
        assert!(windows.runs_on("windows"));
        assert!(!windows.runs_on("linux"));
    }

    #[test]
    fn header_defaults_and_validation() {
        let scenario = parse(r#"{"cmd": "state"}"#).unwrap();
        assert_eq!(scenario.header, Header::default());
        assert_eq!(scenario.window_size(), DEFAULT_WINDOW_SIZE);

        let err = parse("{\"cmd\": \"state\"}\n{\"scenario\": {}}").unwrap_err();
        assert!(
            format!("{err:#}").contains("must be the first line"),
            "{err:#}"
        );
        let err = parse(r#"{"scenario": {"mock": true}}"#).unwrap_err();
        assert!(
            format!("{err:#}").contains("bad scenario header"),
            "{err:#}"
        );
        let err = parse("{\"scenario\": {\"platforms\": [\"win32\"]}}\n{\"cmd\": \"state\"}")
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("unknown platform \"win32\""),
            "{err:#}"
        );
        let err =
            parse("{\"scenario\": {\"window_size\": \"big\"}}\n{\"cmd\": \"state\"}").unwrap_err();
        assert!(format!("{err:#}").contains("WIDTHxHEIGHT"), "{err:#}");
    }

    #[test]
    fn errors_name_the_line() {
        let cases = [
            ("{\"cmd\": \"state\"}\nnot json", "line 2: invalid JSON"),
            ("[1]", "line 1: expected a JSON object"),
            ("{\"x\": 1}", "line 1: missing \"cmd\""),
            ("{\"cmd\": \"state\", \"id\": 4}", "don't set \"id\""),
            (
                "{\"cmd\": \"state\", \"expect\": {\"pointer\": \"/a\"}}",
                "has no check",
            ),
            (
                "{\"expect_error\": \"x\", \"expect\": {\"pointer\": \"/a\", \"exists\": true}}",
                "need a cmd",
            ),
            ("{}", "a line needs a cmd"),
            ("", "no steps"),
        ];
        for (text, message) in cases {
            let err = format!("{:#}", parse(text).unwrap_err());
            assert!(err.contains(message), "{text:?}: {err}");
        }
    }

    #[test]
    fn screenshots_are_named_by_the_runner() {
        let scenario = parse(
            r#"{"cmd": "screenshot", "name": "empty shell/1"}
{"cmd": "screenshot", "path": "x.png"}"#,
        )
        .unwrap();
        assert_eq!(
            scenario.steps[0].screenshot.as_deref(),
            Some("empty-shell-1")
        );
        assert!(scenario.steps[0].request().unwrap().get("name").is_none());
        assert_eq!(scenario.steps[1].screenshot, None);
        for text in [
            r#"{"cmd": "screenshot"}"#,
            r#"{"cmd": "screenshot", "name": "a", "path": "b"}"#,
            r#"{"cmd": "screenshot", "name": "///"}"#,
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
        // `name` means something else on other commands and is left alone.
        let scenario = parse(r#"{"cmd": "window", "name": "second"}"#).unwrap();
        assert_eq!(
            scenario.steps[0].request().unwrap().get("name"),
            Some(&json!("second"))
        );
    }

    #[test]
    fn reply_timeouts_cover_the_command_own_wait() {
        let base = Duration::from_secs(30);
        let scenario = parse(
            r#"{"cmd": "wait", "ms": 5000}
{"cmd": "wait_for", "pointer": "/a", "exists": true, "timeout_ms": 20000}
{"cmd": "drag", "from": [1, 1], "to": [9, 9], "steps": 10, "step_ms": 100, "hold_ms": 600}
{"cmd": "state", "runner_timeout_ms": 1234}
{"expect": {"pointer": "/a", "exists": true}}"#,
        )
        .unwrap();
        let timeouts: Vec<u128> = scenario
            .steps
            .iter()
            .map(|step| step.reply_timeout(base).as_millis())
            .collect();
        assert_eq!(timeouts, [35_000, 50_000, 31_600, 1234, 30_000]);
    }

    #[test]
    fn expect_error_and_expect_result_parse() {
        let scenario = parse(
            r#"{"cmd": "click", "target": "nope", "expect_error": "no target"}
{"cmd": "echo", "a": 1, "expect_result": {"pointer": "/a", "equals": 1}}"#,
        )
        .unwrap();
        assert_eq!(scenario.steps[0].expect_error.as_deref(), Some("no target"));
        assert_eq!(expectations(&scenario.steps[1].expect_result).len(), 1);
        assert!(
            parse(r#"{"cmd": "x", "expect_error": 3}"#).is_err(),
            "expect_error must be text"
        );
    }

    #[test]
    fn expectations_are_substituted_per_run() {
        let scenario =
            parse(r#"{"cmd": "state", "expect": {"pointer": "/dir", "equals": "${data_dir}"}}"#)
                .unwrap();
        let parsed =
            Step::expectations(&scenario.steps[0].expect, &[("data_dir", "C:/x".into())]).unwrap();
        assert!(parsed[0].evaluate(&json!({"dir": "C:/x"})).is_ok());
    }

    #[test]
    fn substitution_reaches_nested_strings() {
        let mut value = json!({"paths": ["${data_dir}/a.png"], "note": "${out_dir}", "n": 1});
        substitute(
            &mut value,
            &[("data_dir", "C:/tmp".into()), ("out_dir", "out".into())],
        );
        assert_eq!(
            value,
            json!({"paths": ["C:/tmp/a.png"], "note": "out", "n": 1})
        );
    }

    #[test]
    fn gh_log_steps_parse() {
        let scenario = parse(
            r#"{"gh_log": {}}
{"gh_log": {}, "expect_result": {"pointer": "", "len": 2}, "save": {"first": "/0/argv/0"}}
{"gh_log": {}, "expect": {"pointer": "/app", "equals": "dv"}}"#,
        )
        .unwrap();
        assert!(
            scenario
                .steps
                .iter()
                .all(|step| step.action == Action::GhLog)
        );
        assert_eq!(scenario.steps[0].cmd(), None);
        assert_eq!(expectations(&scenario.steps[1].expect_result).len(), 1);
        assert_eq!(
            scenario.steps[1].save,
            [("first".to_string(), "/0/argv/0".to_string())]
        );
        assert!(scenario.steps[2].expect.is_some());
    }

    #[test]
    fn bad_gh_log_steps_are_rejected() {
        let cases = [
            (r#"{"gh_log": {"path": "x"}}"#, "bad gh_log"),
            (r#"{"gh_log": true}"#, "bad gh_log"),
            (r#"{"gh_log": {}, "cmd": "state"}"#, "exclude each other"),
            (r#"{"gh_log": {}, "window": 1}"#, "unknown key(s)"),
            (
                r#"{"gh_log": {}, "expect_error": "x"}"#,
                "can't expect an error",
            ),
            (r#"{"gh_log": {}, "runner_timeout_ms": 5}"#, "reads a file"),
        ];
        for (line, message) in cases {
            let err = format!("{:#}", parse(line).unwrap_err());
            assert!(err.contains(message), "{line}: {err}");
        }
    }

    #[test]
    fn restarts_parse() {
        let scenario = parse(
            r#"{"cmd": "state"}
{"restart": {}}
{"restart": {"app_args": ["--counter", "7"]}, "expect": {"pointer": "/counter", "equals": 7}, "expect_result": {"pointer": "/event", "equals": "ready"}}
{"restart": {"data_dir": false}}"#,
        )
        .unwrap();
        assert_eq!(
            scenario.steps[1].action,
            Action::Restart(Restart::default())
        );
        assert_eq!(
            scenario.steps[2].action,
            Action::Restart(Restart {
                app_args: Some(vec!["--counter".into(), "7".into()]),
                data_dir: None,
            })
        );
        assert!(scenario.steps[2].expect.is_some());
        assert_eq!(
            scenario.steps[3].action,
            Action::Restart(Restart {
                app_args: None,
                data_dir: Some(false),
            })
        );
        for (text, message) in [
            (r#"{"restart": {"args": []}}"#, "bad restart"),
            (r#"{"restart": true}"#, "bad restart"),
            (
                r#"{"restart": {}, "expect_error": "x"}"#,
                "can't expect an error",
            ),
            (
                r#"{"restart": {}, "runner_timeout_ms": 5}"#,
                "--ready-timeout-ms",
            ),
            (r#"{"restart": {}, "cmd": "quit"}"#, "exclude each other"),
        ] {
            let err = format!("{:#}", parse(text).unwrap_err());
            assert!(err.contains(message), "{text}: {err}");
        }
    }

    #[test]
    fn only_restarts_and_gh_logs_follow_quit() {
        let ok = parse(
            r#"{"cmd": "quit"}
{"gh_log": {}, "expect_result": {"pointer": "", "len": 0}}
{"restart": {}}
{"cmd": "state"}
{"cmd": "quit"}
{"restart": {}}"#,
        );
        assert!(ok.is_ok(), "{:#}", ok.unwrap_err());
        for (text, line) in [
            ("{\"cmd\": \"quit\"}\n{\"cmd\": \"state\"}", 2),
            (
                "{\"cmd\": \"quit\"}\n{\"expect\": {\"pointer\": \"/a\", \"exists\": true}}",
                2,
            ),
            (
                "{\"cmd\": \"quit\"}\n{\"gh_log\": {}, \"expect\": {\"pointer\": \"/a\", \"exists\": true}}",
                2,
            ),
        ] {
            let err = format!("{:#}", parse(text).unwrap_err());
            assert!(
                err.contains(&format!("line {line}: after quit only a restart")),
                "{text}: {err}"
            );
        }
        let err = format!(
            "{:#}",
            parse(r#"{"cmd": "quit", "expect": {"pointer": "/a", "exists": true}}"#).unwrap_err()
        );
        assert!(err.contains("no state to expect"), "{err}");
    }

    #[test]
    fn runner_owned_env_is_rejected() {
        for name in [
            DATA_DIR_VAR,
            "dv_data_dir",
            GH_VAR,
            MOCK_GH_VAR,
            MOCK_GH_LOG_VAR,
            GIT_CEILING_VAR,
            "E2E_DATA_DIR",
        ] {
            let text = format!(
                "{{\"scenario\": {{\"env\": {{\"{name}\": \"x\"}}}}}}\n{{\"cmd\": \"state\"}}"
            );
            let err = format!("{:#}", parse(&text).unwrap_err());
            assert!(err.contains("the runner sets it"), "{name}: {err}");
        }
        let scenario =
            parse("{\"scenario\": {\"env\": {\"RUST_LOG\": \"debug\"}}}\n{\"cmd\": \"state\"}")
                .unwrap();
        assert_eq!(scenario.header.env["RUST_LOG"], "debug");
    }

    #[test]
    fn wildcards_match_file_names() {
        assert!(wildcard_match("*.jsonl", "smoke.jsonl"));
        assert!(wildcard_match("SMOKE.JSONL", "smoke.jsonl"));
        assert!(wildcard_match("shell_*.jsonl", "shell_layout.jsonl"));
        assert!(wildcard_match("s?oke.*", "smoke.jsonl"));
        assert!(!wildcard_match("*.jsonl", "smoke.json"));
        assert!(!wildcard_match("shell_*", "review.jsonl"));
        assert!(wildcard_match("*", ""));
    }

    #[test]
    fn discover_expands_directories_and_wildcards() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["b.jsonl", "a.jsonl", "notes.md"] {
            std::fs::write(dir.path().join(name), "{}").unwrap();
        }
        let names = |paths: Vec<PathBuf>| -> Vec<String> {
            paths
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(
            names(discover(&[dir.path().to_path_buf()]).unwrap()),
            ["a.jsonl", "b.jsonl"]
        );
        assert_eq!(
            names(discover(&[dir.path().join("b*")]).unwrap()),
            ["b.jsonl"]
        );
        assert!(discover(&[dir.path().join("z*")]).is_err());
        assert!(discover(&[dir.path().join("missing.jsonl")]).is_err());
    }
}
