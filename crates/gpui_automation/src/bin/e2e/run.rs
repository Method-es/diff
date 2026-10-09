//! Running one scenario: spawn the app with a hermetic environment, stream
//! the steps, check the assertions, and always clean up.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_automation::matcher::{Expectation, compact};
use serde_json::{Value, json};

use crate::foreground::{self, Mark, Watch, WindowInfo};
use crate::process::{self, ChildProcess, ReadError};
use crate::scenario::{
    Action, DATA_DIR_VAR, GH_VAR, GIT_CEILING_VAR, MOCK_GH_LOG_VAR, MOCK_GH_VAR, Restart, Scenario,
    Step, WSL_HOME_VAR, substitute,
};

/// The file in a scenario's output dir that `DV_MOCK_GH_LOG` names.
pub const GH_LOG_FILE: &str = "gh_calls.jsonl";

/// How long the app may take to answer `quit`, and then to exit.
const QUIT_REPLY_TIMEOUT: Duration = Duration::from_secs(10);
const EXIT_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Config {
    pub app: PathBuf,
    /// The fake `gh` every launch's `DV_GH` names (docs/PLAN.md §14.2).
    pub mock_gh: PathBuf,
    pub out_root: PathBuf,
    pub workspace: PathBuf,
    /// Base allowance for each reply, on top of the command's own waits.
    pub step_timeout: Duration,
    pub ready_timeout: Duration,
    pub keep_data: bool,
    /// Start the app with `--automation-background` and fail a scenario whose app takes the
    /// foreground.
    pub background: bool,
    pub verbose: bool,
}

pub enum Outcome {
    Passed {
        steps: usize,
        screenshots: usize,
    },
    Failed {
        line: Option<usize>,
        message: String,
    },
    Interrupted,
}

/// Why a scenario stopped early.
enum Stop {
    Failed {
        line: Option<usize>,
        message: String,
    },
    Interrupted,
}

impl From<anyhow::Error> for Stop {
    fn from(err: anyhow::Error) -> Self {
        Stop::Failed {
            line: None,
            message: format!("{err:#}"),
        }
    }
}

/// Everything needed to (re)start the app the same way.
struct Launcher<'a> {
    config: &'a Config,
    scenario: &'a Scenario,
    out_dir: PathBuf,
    data_dir: PathBuf,
    /// The header's `gh` file, for `DV_MOCK_GH`.
    gh_scenario: Option<PathBuf>,
    /// `DV_MOCK_GH_LOG`: `<out_dir>/gh_calls.jsonl`.
    gh_log: PathBuf,
    vars: Vec<(&'static str, String)>,
    /// The header's `app_args` until a restart replaces them.
    app_args: Vec<String>,
    /// Pass `--data-dir` (until a restart says otherwise).
    data_dir_arg: bool,
    launches: usize,
    /// Every app process this scenario started, for the foreground check.
    pids: Vec<u32>,
    /// Start the app with `--automation-background`: a background run, unless the scenario's
    /// header asks for the foreground.
    background: bool,
}

impl Launcher<'_> {
    /// The app's command line and environment: hermetic whatever the
    /// runner's own environment holds (PLAN §14.1).
    fn command(&self) -> Command {
        let mut command = Command::new(&self.config.app);
        command
            // The scenario's own data dir, not the repo root: relative paths resolve there, and
            // a git repository lookup from it stops at the output dir (`GIT_CEILING_DIRECTORIES`
            // below) instead of finding this checkout.
            .current_dir(&self.data_dir)
            .arg("--automation");
        if self.background {
            command.arg("--automation-background");
        }
        if self.data_dir_arg {
            command.arg("--data-dir").arg(&self.data_dir);
        }
        command
            .arg("--window-size")
            .arg(self.scenario.window_size())
            .args(substituted_args(&self.app_args, &self.vars));
        command
            // Never one inherited from the runner's environment: only a header names a distro
            // home dv may install into.
            .env_remove(WSL_HOME_VAR)
            .envs(&self.scenario.header.env)
            .env("E2E_OUT_DIR", &self.out_dir)
            .env("E2E_DATA_DIR", &self.data_dir)
            .env(DATA_DIR_VAR, &self.data_dir)
            .env(GH_VAR, &self.config.mock_gh)
            .env(MOCK_GH_LOG_VAR, &self.gh_log)
            .env(GIT_CEILING_VAR, &self.out_dir);
        match &self.gh_scenario {
            Some(path) => command.env(MOCK_GH_VAR, path),
            // Never one inherited from the runner's environment: without a scenario file
            // mock_gh answers nothing.
            None => command.env_remove(MOCK_GH_VAR),
        };
        command
    }

    fn spawn(&mut self) -> Result<ChildProcess> {
        if !self.config.app.is_file() {
            bail!(
                "app {} not found; build it (`cargo build -p dv`) or pass --app",
                self.config.app.display()
            );
        }
        self.launches += 1;
        let log = self.out_dir.join(stderr_log_name(self.launches));
        let app = ChildProcess::spawn("app", self.command(), &log)?;
        self.pids.push(app.pid());
        Ok(app)
    }
}

/// `app.stderr.log` for the first launch, `app.<n>.stderr.log` after restarts.
fn stderr_log_name(launch: usize) -> String {
    match launch {
        0 | 1 => "app.stderr.log".to_string(),
        n => format!("app.{n}.stderr.log"),
    }
}

struct Session<'a> {
    config: &'a Config,
    scenario: &'a Scenario,
    launcher: Launcher<'a>,
    app: ChildProcess,
    /// The app answered `quit` and exited; only a restart brings it back.
    exited: bool,
    transcript: Option<File>,
    started: Instant,
    screenshots: usize,
    foreground: ForegroundCheck<'a>,
    /// Variables set by `save`, in the order they were first set.
    saved: Vec<(String, String)>,
}

/// Background runs: the foreground window before the app started, and the watch that reports
/// every foreground change since.
struct ForegroundCheck<'a> {
    watch: Option<&'a Watch>,
    before: WindowInfo,
    mark: Mark,
}

pub fn run(scenario: &Scenario, config: &Config, watch: Option<&Watch>) -> Outcome {
    let out_dir = config.out_root.join(&scenario.name);
    let data_dir = data_dir(&out_dir);
    if let Err(err) = prepare_dirs(&out_dir, &data_dir) {
        return Outcome::Failed {
            line: None,
            message: format!("{err:#}"),
        };
    }

    let outcome = run_in(scenario, config, watch, &out_dir, &data_dir);
    // Kept on failure for post-mortems; an interrupted run is simply discarded.
    let failed = matches!(outcome, Outcome::Failed { .. });
    if failed || config.keep_data {
        eprintln!("e2e: kept data dir {}", data_dir.display());
    } else if let Err(err) = std::fs::remove_dir_all(&data_dir) {
        eprintln!(
            "e2e: could not remove the data dir {}: {err}",
            data_dir.display()
        );
    }
    outcome
}

/// The scenario's data dir: inside its output dir, so test data stays on the repo's drive
/// (never the system temp dir) and the next run of the scenario wipes it.
fn data_dir(out_dir: &Path) -> PathBuf {
    out_dir.join("data")
}

/// The file a runner holds locked in its output root for the whole run.
pub const LOCK_FILE: &str = ".e2e.lock";

/// Creates `out_root` and takes an exclusive OS lock on `<out_root>/.e2e.lock`, held until the
/// returned file is dropped. Two runners sharing an output root would wipe each other's
/// scenario dirs ([`prepare_dirs`]) and fail on the data dir the other's app runs in, so the
/// second one fails fast instead. The OS releases the lock when the process ends, however it
/// ends, so a killed run never leaves a stale lock behind.
pub fn lock_out_root(out_root: &Path) -> Result<File> {
    std::fs::create_dir_all(out_root)
        .with_context(|| format!("creating {}", out_root.display()))?;
    let path = out_root.join(LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => bail!(
            "another e2e run is using {} (it holds {LOCK_FILE}); wait for it to finish or pass \
             a different --out",
            out_root.display()
        ),
        Err(TryLockError::Error(err)) => {
            Err(err).with_context(|| format!("locking {}", path.display()))
        }
    }
}

/// Clears the scenario's output dir left by the previous run, data dir and `mock_gh` log
/// included, and creates both dirs afresh.
fn prepare_dirs(out_dir: &Path, data_dir: &Path) -> Result<()> {
    if out_dir.exists() {
        std::fs::remove_dir_all(out_dir)
            .with_context(|| format!("clearing {}", out_dir.display()))?;
    }
    std::fs::create_dir_all(data_dir).with_context(|| format!("creating {}", data_dir.display()))
}

/// The `${...}` variables for app args, commands and expectations: the
/// directories plus what the app was pointed at.
fn vars(
    config: &Config,
    out_dir: &Path,
    data_dir: &Path,
    gh_scenario: Option<&Path>,
    gh_log: &Path,
) -> Vec<(&'static str, String)> {
    let mut vars = vec![
        ("data_dir", data_dir.display().to_string()),
        ("out_dir", out_dir.display().to_string()),
        ("workspace", config.workspace.display().to_string()),
        ("dv_gh", config.mock_gh.display().to_string()),
        ("dv_mock_gh_log", gh_log.display().to_string()),
    ];
    if let Some(path) = gh_scenario {
        vars.push(("dv_mock_gh", path.display().to_string()));
    }
    vars
}

/// The header's `gh` file as an absolute path, after checking that it and
/// `mock_gh` exist: a scenario that scripts `gh` answers is meaningless
/// without either.
fn resolve_gh_scenario(scenario: &Scenario, config: &Config) -> Result<Option<PathBuf>> {
    let Some(path) = scenario.gh_scenario() else {
        return Ok(None);
    };
    let path = std::path::absolute(&path).unwrap_or(path);
    if !path.is_file() {
        bail!(
            "the scenario's gh file {} does not exist (header \"gh\" is relative to the scenario \
             file)",
            path.display()
        );
    }
    if !config.mock_gh.is_file() {
        bail!(
            "mock_gh {} not found; build it (`cargo build -p mock_gh`) or pass --mock-gh",
            config.mock_gh.display()
        );
    }
    Ok(Some(path))
}

fn run_in(
    scenario: &Scenario,
    config: &Config,
    watch: Option<&Watch>,
    out_dir: &Path,
    data_dir: &Path,
) -> Outcome {
    let failed = |err: anyhow::Error| Outcome::Failed {
        line: None,
        message: format!("{err:#}"),
    };
    let background = config.background && !scenario.header.foreground;
    // Recorded before anything starts, so a failure can say what the app took the foreground
    // from.
    let foreground = ForegroundCheck {
        watch: watch.filter(|_| background),
        before: foreground::current(),
        mark: watch.map(Watch::mark).unwrap_or_default(),
    };

    let gh_scenario = match resolve_gh_scenario(scenario, config) {
        Ok(path) => path,
        Err(err) => return failed(err),
    };
    let gh_log = out_dir.join(GH_LOG_FILE);
    let mut launcher = Launcher {
        config,
        scenario,
        out_dir: out_dir.to_path_buf(),
        data_dir: data_dir.to_path_buf(),
        vars: vars(config, out_dir, data_dir, gh_scenario.as_deref(), &gh_log),
        gh_scenario,
        gh_log,
        app_args: scenario.header.app_args.clone(),
        data_dir_arg: true,
        launches: 0,
        pids: Vec::new(),
        background,
    };
    let app = match launcher.spawn() {
        Ok(app) => app,
        Err(err) => return failed(err),
    };
    let transcript = File::create(out_dir.join("transcript.log")).ok();
    let mut session = Session {
        config,
        scenario,
        launcher,
        app,
        exited: false,
        transcript,
        started: Instant::now(),
        screenshots: 0,
        foreground,
        saved: Vec::new(),
    };
    if background {
        let before = session.foreground.before.to_string();
        session.log(&format!(
            "# background run; foreground window before: {before}"
        ));
    }

    let outcome = match session.drive() {
        Ok(steps) => Outcome::Passed {
            steps,
            screenshots: session.screenshots,
        },
        Err(Stop::Interrupted) => Outcome::Interrupted,
        Err(Stop::Failed { line, message }) => {
            session.capture_failure();
            let mut message = message;
            let tail = session.app.stderr_tail(25);
            if !tail.trim().is_empty() {
                message.push_str(&format!(
                    "\n  app stderr ({}):\n    {}",
                    session.app.stderr_log.display(),
                    tail.replace('\n', "\n    ")
                ));
            }
            Outcome::Failed { line, message }
        }
    };
    session.app.shutdown(Duration::from_secs(5));
    outcome
}

fn substituted_args(args: &[String], vars: &[(&str, String)]) -> Vec<String> {
    args.iter()
        .map(|arg| {
            let mut value = Value::String(arg.clone());
            substitute(&mut value, vars);
            value.as_str().map(str::to_string).unwrap_or_default()
        })
        .collect()
}

/// The calls in a `mock_gh` log: one JSON object per line (PLAN §14.2), as
/// an array in the order they were made. A log that doesn't exist yet means
/// no call has been made.
fn read_gh_log(path: &Path) -> Result<Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(json!([])),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    let mut calls = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let call: Value = serde_json::from_str(line).with_context(|| {
            format!(
                "{}:{}: not a JSON call record: {line}",
                path.display(),
                index + 1
            )
        })?;
        calls.push(call);
    }
    Ok(Value::Array(calls))
}

impl Session<'_> {
    /// The launcher's variables plus those set by `save`.
    fn vars(&self) -> Vec<(&str, String)> {
        self.launcher
            .vars
            .iter()
            .map(|(name, value)| (*name, value.clone()))
            .chain(
                self.saved
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.clone())),
            )
            .collect()
    }

    /// Sets the step's `save` variables from its action's result.
    fn save(&mut self, step: &Step, result: &Value) -> Result<(), Stop> {
        for (name, pointer) in &step.save {
            let value = result.pointer(pointer).ok_or_else(|| {
                anyhow!(
                    "save {name}: {pointer:?} is not in the result {}",
                    compact(result)
                )
            })?;
            let text = match value {
                Value::String(text) => text.clone(),
                other => compact(other),
            };
            self.log(&format!("# save {name} = {text}"));
            match self.saved.iter_mut().find(|(saved, _)| saved == name) {
                Some((_, slot)) => *slot = text,
                None => self.saved.push((name.clone(), text)),
            }
        }
        Ok(())
    }

    fn drive(&mut self) -> Result<usize, Stop> {
        self.wait_ready()?;
        self.check_foreground()?;
        let scenario = self.scenario;
        for step in &scenario.steps {
            if process::interrupted() {
                return Err(Stop::Interrupted);
            }
            self.run_step(step)
                .and_then(|()| self.check_foreground())
                .map_err(|stop| match stop {
                    Stop::Failed {
                        line: None,
                        message,
                    } => Stop::Failed {
                        line: Some(step.line),
                        message,
                    },
                    other => other,
                })?;
        }
        if !self.exited {
            self.quit_app(json!("final-quit"))
                .map_err(|stop| match stop {
                    Stop::Failed { line, message } => Stop::Failed {
                        line,
                        message: format!("after the last step: {message}"),
                    },
                    other => other,
                })?;
        }
        self.check_foreground()?;
        Ok(scenario.steps.len())
    }

    /// Background runs: fail if one of this scenario's app processes owned the foreground
    /// window since the last check. Foreground changes between other processes are the person
    /// at the machine switching windows; they are only noted in the transcript.
    fn check_foreground(&mut self) -> Result<(), Stop> {
        let Some(watch) = self.foreground.watch else {
            return Ok(());
        };
        let (ours, others) = watch.since(self.foreground.mark, &self.launcher.pids);
        self.foreground.mark = watch.mark();
        for window in others {
            self.log(&format!("# foreground changed to {window} (not the app)"));
        }
        match ours {
            None => Ok(()),
            Some(window) => {
                self.log(&format!("# the app took the foreground: {window}"));
                let before = &self.foreground.before;
                Err(anyhow!(
                    "the app took the foreground during a background run: {window} became the \
                     foreground window (before the run it was {before}); background windows must \
                     never activate (tests/e2e/README.md \"Background runs\")"
                )
                .into())
            }
        }
    }

    /// Wait for the app's `ready` event and return it.
    fn wait_ready(&mut self) -> Result<Value, Stop> {
        let deadline = Instant::now() + self.config.ready_timeout;
        loop {
            let line = self.read_line(deadline, "the ready event")?;
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && value.get("event").and_then(Value::as_str) == Some("ready")
            {
                if self.config.verbose {
                    eprintln!("e2e: app ready (pid {})", self.app.pid());
                }
                return Ok(value);
            }
        }
    }

    fn run_step(&mut self, step: &Step) -> Result<(), Stop> {
        if self.exited && !matches!(step.action, Action::Restart(_) | Action::GhLog) {
            // Scenario::parse rejects this; kept so a bad file can't hang here.
            return Err(anyhow!("the app has quit; only a restart may follow").into());
        }
        let result = match &step.action {
            Action::Command(request) => self.run_command(step, request)?,
            Action::GhLog => {
                let calls = self.gh_log()?;
                self.check_outcome(step, Ok(calls), "gh_log")?
            }
            Action::Restart(restart) => {
                let ready = self.restart(step, restart)?;
                let expect = Step::expectations(&step.expect_result, &self.vars())?;
                check_all(&expect, &ready, "ready event")?;
                Some(ready)
            }
            Action::Check => None,
        };
        if let Some(result) = &result {
            self.save(step, result)?;
        }
        let expect = Step::expectations(&step.expect, &self.vars())?;
        if !expect.is_empty() {
            let mut request = json!({"cmd": "state"});
            if let Some(window) = step.window() {
                request["window"] = window.clone();
            }
            let id = json!(format!("{}/state", step.line));
            let reply = self.request(id, request, self.config.step_timeout)?;
            let state = reply_result(&reply).map_err(|err| anyhow!("fetching state: {err}"))?;
            check_all(&expect, &state, "state")?;
        }
        Ok(())
    }

    fn run_command(
        &mut self,
        step: &Step,
        request: &serde_json::Map<String, Value>,
    ) -> Result<Option<Value>, Stop> {
        let mut request = Value::Object(request.clone());
        substitute(&mut request, &self.vars());
        if let Some(name) = &step.screenshot {
            self.screenshots += 1;
            let path = self
                .launcher
                .out_dir
                .join(format!("{:02}-{name}.png", self.screenshots));
            request["path"] = Value::String(path.display().to_string());
        } else if let Some(path) = request.get("path").and_then(Value::as_str)
            && step.cmd() == Some("screenshot")
        {
            // Relative to the runner's directory, not the app's (the data dir).
            let absolute = std::path::absolute(path).map_err(anyhow::Error::from)?;
            request["path"] = Value::String(absolute.display().to_string());
        }
        let timeout = step.reply_timeout(self.config.step_timeout);
        let reply = self.request(json!(step.line), request, timeout)?;
        let quitting = step.cmd() == Some("quit") && reply_result(&reply).is_ok();
        let result = self.check_outcome(step, reply_result(&reply), describe(step))?;
        if quitting {
            self.await_exit()?;
        }
        Ok(result)
    }

    /// Check a command's reply (or the `gh_log` calls) against the step's
    /// `expect_error` / `expect_result`. Returns the result when there is one
    /// (not when an expected error arrived).
    fn check_outcome(
        &self,
        step: &Step,
        outcome: Result<Value, String>,
        what: &str,
    ) -> Result<Option<Value>, Stop> {
        match (&step.expect_error, outcome) {
            (Some(expected), Err(error)) => {
                if !error.contains(expected.as_str()) {
                    return Err(anyhow!(
                        "expected an error containing {expected:?}, got {error:?}"
                    )
                    .into());
                }
            }
            (Some(expected), Ok(result)) => {
                return Err(anyhow!(
                    "expected an error containing {expected:?}, but it succeeded with {}",
                    compact(&result)
                )
                .into());
            }
            (None, Err(error)) => return Err(anyhow!("{what} failed: {error}").into()),
            (None, Ok(result)) => {
                let expect = Step::expectations(&step.expect_result, &self.vars())?;
                check_all(&expect, &result, "result")?;
                return Ok(Some(result));
            }
        }
        Ok(None)
    }

    /// The calls `mock_gh` has logged so far in this scenario (every launch shares the log).
    fn gh_log(&mut self) -> Result<Value, Stop> {
        let path = self.launcher.gh_log.clone();
        self.log(&format!("> gh_log {}", path.display()));
        let calls = read_gh_log(&path)?;
        let shown = compact(&calls);
        self.log(&format!("< gh_log {shown}"));
        if self.config.verbose {
            eprintln!("  < gh_log {shown}");
        }
        Ok(calls)
    }

    /// Send `quit` and require a clean exit.
    fn quit_app(&mut self, id: Value) -> Result<(), Stop> {
        let reply = self.request(id, json!({"cmd": "quit"}), QUIT_REPLY_TIMEOUT)?;
        reply_result(&reply).map_err(|err| anyhow!("quit failed: {err}"))?;
        self.await_exit()
    }

    /// After a successful `quit` reply: the app must exit with success soon.
    fn await_exit(&mut self) -> Result<(), Stop> {
        let status = self.app.wait_exit(EXIT_TIMEOUT);
        self.log(&format!(
            "# app exit: {}",
            status.map_or_else(|| "still running".to_string(), |s| s.to_string())
        ));
        check_exit(status)?;
        self.exited = true;
        Ok(())
    }

    /// Quit (unless a `quit` step already did) and launch the app again on
    /// the same data dir, `mock_gh` scenario and log, and environment.
    /// Returns the new `ready` event.
    fn restart(&mut self, step: &Step, restart: &Restart) -> Result<Value, Stop> {
        if !self.exited {
            self.quit_app(json!(format!("{}/quit", step.line)))?;
        }
        if let Some(app_args) = &restart.app_args {
            // Variables `save` set earlier are known now and stay what they were; the
            // launcher's own are the same for every launch.
            let app_args = substituted_args(app_args, &self.vars());
            self.launcher.app_args = app_args;
        }
        if let Some(data_dir) = restart.data_dir {
            self.launcher.data_dir_arg = data_dir;
        }
        let launch = self.launcher.launches + 1;
        self.log(&format!("# restart: launch {launch}"));
        if self.config.verbose {
            eprintln!("e2e: restarting the app (launch {launch})");
        }
        // The old process has exited, so dropping it only reaps it.
        self.app = self.launcher.spawn()?;
        self.exited = false;
        self.wait_ready()
    }

    /// Send one request and wait for the reply carrying its id.
    fn request(&mut self, id: Value, mut request: Value, timeout: Duration) -> Result<Value, Stop> {
        request["id"] = id.clone();
        let text = request.to_string();
        self.log(&format!("> {text}"));
        if self.config.verbose {
            eprintln!("  > {text}");
        }
        self.app.send_line(&text)?;
        let deadline = Instant::now() + timeout;
        let what = format!("the reply to {}", compact(&request));
        loop {
            let line = self.read_line(deadline, &what)?;
            let Ok(reply) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if reply.get("id") == Some(&id) {
                if self.config.verbose {
                    eprintln!("  < {}", compact(&reply));
                }
                return Ok(reply);
            }
        }
    }

    /// Next stdout line from the app. Non-JSON lines are recorded in the
    /// transcript but otherwise ignored: only replies matter.
    fn read_line(&mut self, deadline: Instant, what: &str) -> Result<String, Stop> {
        match self.app.read_line(deadline) {
            Ok(line) => {
                self.log(&format!("< {line}"));
                Ok(line)
            }
            Err(ReadError::Interrupted) => Err(Stop::Interrupted),
            Err(ReadError::Timeout) => Err(anyhow!("timed out waiting for {what}").into()),
            Err(ReadError::Closed) => {
                let status = self
                    .app
                    .wait_exit(Duration::from_secs(2))
                    .map_or_else(|| "still running".to_string(), |s| s.to_string());
                Err(anyhow!("the app closed stdout while waiting for {what} ({status})").into())
            }
        }
    }

    fn log(&mut self, line: &str) {
        if let Some(transcript) = &mut self.transcript {
            let elapsed = self.started.elapsed().as_secs_f64();
            let _ = writeln!(transcript, "[{elapsed:8.3}s] {line}");
        }
    }

    /// Best effort: a screenshot of the failing state helps more than any
    /// message.
    fn capture_failure(&mut self) {
        if self.exited || self.app.try_status().is_some() {
            return;
        }
        let path = self.launcher.out_dir.join("failure.png");
        let request = json!({"cmd": "screenshot", "path": path.display().to_string()});
        match self.request(
            json!("failure-screenshot"),
            request,
            Duration::from_secs(10),
        ) {
            Ok(reply) if reply_result(&reply).is_ok() => {
                eprintln!("e2e: failure screenshot {}", path.display());
            }
            _ => {}
        }
    }
}

fn describe(step: &Step) -> &str {
    step.cmd().unwrap_or("step")
}

/// How the app ended after answering `quit`.
fn check_exit(status: Option<std::process::ExitStatus>) -> Result<()> {
    match status {
        Some(status) if status.success() => Ok(()),
        Some(status) => bail!("the app exited with {status} after quit"),
        None => bail!(
            "the app did not exit within {}s of quit",
            EXIT_TIMEOUT.as_secs()
        ),
    }
}

/// `Ok(result)` or `Err(error text)` from a reply line.
fn reply_result(reply: &Value) -> Result<Value, String> {
    match reply.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(reply.get("result").cloned().unwrap_or(Value::Null)),
        Some(false) => Err(reply
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string()),
        None => Err(format!("malformed reply {}", compact(reply))),
    }
}

fn check_all(expectations: &[Expectation], value: &Value, what: &str) -> Result<()> {
    let failures: Vec<String> = expectations
        .iter()
        .filter_map(|expectation| expectation.evaluate(value).err())
        .map(|mismatch| format!("{what} {mismatch}"))
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        bail!("{}", failures.join("\n  "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::Map;
    use std::collections::BTreeMap;
    use std::ffi::OsStr;

    fn reply(ok: bool, payload: Value) -> Value {
        let mut map = Map::new();
        map.insert("id".into(), json!(1));
        map.insert("ok".into(), json!(ok));
        map.insert(if ok { "result" } else { "error" }.into(), payload);
        Value::Object(map)
    }

    #[test]
    fn replies_split_into_result_or_error() {
        assert_eq!(
            reply_result(&reply(true, json!({"a": 1}))),
            Ok(json!({"a": 1}))
        );
        assert_eq!(
            reply_result(&reply(false, json!("boom"))),
            Err("boom".into())
        );
        assert!(reply_result(&json!({"id": 1})).is_err());
    }

    #[test]
    fn check_all_reports_every_failure() {
        let expectations = Expectation::parse_list(&json!([
            {"pointer": "/a", "equals": 1},
            {"pointer": "/b", "equals": 2},
            {"pointer": "/c", "exists": false},
        ]))
        .unwrap();
        let err = check_all(&expectations, &json!({"a": 1, "b": 3, "c": 0}), "state")
            .unwrap_err()
            .to_string();
        assert!(err.contains("state /b: expected equals 2, got 3"), "{err}");
        assert!(err.contains("state /c: expected not to exist"), "{err}");
        assert!(!err.contains("/a"), "{err}");
    }

    #[test]
    fn app_args_are_substituted() {
        let vars = [("data_dir", "D:/tmp/x".to_string())];
        assert_eq!(
            substituted_args(&["--repo".into(), "${data_dir}/repo".into()], &vars),
            ["--repo", "D:/tmp/x/repo"]
        );
        // A restart's arguments see `save`d variables too.
        let vars = [
            ("data_dir", "D:/tmp/x".to_string()),
            ("review", "r-1".to_string()),
        ];
        assert_eq!(
            substituted_args(&["--review".into(), "${review}".into()], &vars),
            ["--review", "r-1"]
        );
    }

    fn config() -> Config {
        Config {
            app: PathBuf::from("app.exe"),
            mock_gh: PathBuf::from("mock_gh.exe"),
            out_root: PathBuf::from("out"),
            workspace: PathBuf::from("ws"),
            step_timeout: Duration::from_secs(30),
            ready_timeout: Duration::from_secs(60),
            keep_data: false,
            background: true,
            verbose: false,
        }
    }

    /// A launcher as `run_in` builds one, with the header's `gh` file resolved but not checked.
    fn launcher<'a>(config: &'a Config, scenario: &'a Scenario) -> Launcher<'a> {
        let (out_dir, data_dir) = (PathBuf::from("out/demo"), PathBuf::from("data"));
        let gh_scenario = scenario.gh_scenario();
        let gh_log = out_dir.join(GH_LOG_FILE);
        Launcher {
            config,
            scenario,
            vars: vars(config, &out_dir, &data_dir, gh_scenario.as_deref(), &gh_log),
            out_dir,
            data_dir,
            gh_scenario,
            gh_log,
            app_args: scenario.header.app_args.clone(),
            data_dir_arg: true,
            launches: 0,
            pids: Vec::new(),
            background: config.background && !scenario.header.foreground,
        }
    }

    /// The command's environment changes: `Some(value)` set, `None` removed.
    fn envs(command: &Command) -> BTreeMap<String, Option<String>> {
        command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().to_ascii_uppercase(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    fn scenario(text: &str) -> Scenario {
        Scenario::parse("demo", Path::new("tests/demo.jsonl"), text).unwrap()
    }

    #[test]
    fn runs_are_hermetic() {
        let config = config();
        let scenario = scenario(r#"{"cmd": "state"}"#);
        let command = launcher(&config, &scenario).command();
        let envs = envs(&command);
        let path = |value: &Option<String>| value.as_deref().map(PathBuf::from);
        assert_eq!(
            envs[DATA_DIR_VAR].as_deref(),
            Some("data"),
            "never the owner's data dir"
        );
        assert_eq!(
            envs[GH_VAR].as_deref(),
            Some("mock_gh.exe"),
            "never the real gh"
        );
        assert_eq!(
            envs[MOCK_GH_VAR], None,
            "removed, so mock_gh answers nothing and no inherited scenario applies"
        );
        assert_eq!(
            envs[WSL_HOME_VAR], None,
            "removed, so dv reaches WSL through its stand-in"
        );
        assert_eq!(
            path(&envs[MOCK_GH_LOG_VAR]),
            Some(Path::new("out/demo").join(GH_LOG_FILE))
        );
        assert_eq!(
            path(&envs[GIT_CEILING_VAR]),
            Some(PathBuf::from("out/demo")),
            "a repository lookup from the data dir never finds this checkout"
        );
        assert_eq!(envs["E2E_DATA_DIR"].as_deref(), Some("data"));
        assert_eq!(path(&envs["E2E_OUT_DIR"]), Some(PathBuf::from("out/demo")));
        assert_eq!(command.get_current_dir(), Some(Path::new("data")));
        let args: Vec<&OsStr> = command.get_args().collect();
        assert_eq!(
            args,
            [
                "--automation",
                "--automation-background",
                "--data-dir",
                "data",
                "--window-size",
                "1440x900"
            ]
        );
    }

    #[test]
    fn only_a_header_names_a_wsl_home() {
        let config = config();
        // An exploratory run against a real distro names its scratch home in the header.
        let wsl = scenario(
            "{\"scenario\": {\"env\": {\"DV_WSL_HOME\": \"/tmp/dv-explore\"}}}\n{\"cmd\": \"state\"}",
        );
        let envs = envs(&launcher(&config, &wsl).command());
        assert_eq!(envs[WSL_HOME_VAR].as_deref(), Some("/tmp/dv-explore"));
    }

    #[test]
    fn gh_headers_point_mock_gh_at_their_file() {
        let config = config();
        let scenario = scenario(
            "{\"scenario\": {\"gh\": \"fixtures/gh/pr.json\", \"app_args\": [\"--log\", \"${dv_mock_gh_log}\"], \"env\": {\"RUST_LOG\": \"info\"}}}\n{\"cmd\": \"state\"}",
        );
        let launcher = launcher(&config, &scenario);
        let command = launcher.command();
        let envs = envs(&command);
        assert_eq!(
            envs[MOCK_GH_VAR].as_deref().map(PathBuf::from),
            Some(Path::new("tests").join("fixtures/gh/pr.json"))
        );
        assert_eq!(envs[GH_VAR].as_deref(), Some("mock_gh.exe"));
        assert_eq!(envs["RUST_LOG"].as_deref(), Some("info"));
        let args: Vec<&OsStr> = command.get_args().collect();
        let log = Path::new("out/demo").join(GH_LOG_FILE);
        assert_eq!(&args[6..], [OsStr::new("--log"), log.as_os_str()]);
        let names: Vec<&str> = launcher.vars.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            [
                "data_dir",
                "out_dir",
                "workspace",
                "dv_gh",
                "dv_mock_gh_log",
                "dv_mock_gh"
            ]
        );
    }

    #[test]
    fn gh_scenarios_need_their_file_and_mock_gh() {
        let dir = tempfile::tempdir().unwrap();
        let scenario_path = dir.path().join("demo.jsonl");
        let header = "{\"scenario\": {\"gh\": \"gh.json\"}}\n{\"cmd\": \"state\"}";
        let scenario = Scenario::parse("demo", &scenario_path, header).unwrap();
        let mock_gh = dir.path().join("mock_gh.exe");
        let with_mock_gh = Config {
            mock_gh: mock_gh.clone(),
            ..config()
        };

        let err = resolve_gh_scenario(&scenario, &with_mock_gh)
            .unwrap_err()
            .to_string();
        assert!(err.contains("gh.json does not exist"), "{err}");
        std::fs::write(dir.path().join("gh.json"), "{\"calls\": []}").unwrap();
        let err = resolve_gh_scenario(&scenario, &with_mock_gh)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("not found") && err.contains("--mock-gh"),
            "{err}"
        );
        std::fs::write(&mock_gh, b"").unwrap();
        assert_eq!(
            resolve_gh_scenario(&scenario, &with_mock_gh).unwrap(),
            Some(dir.path().join("gh.json"))
        );

        let plain = Scenario::parse("plain", &scenario_path, r#"{"cmd": "state"}"#).unwrap();
        let missing = Config {
            mock_gh: dir.path().join("absent.exe"),
            ..config()
        };
        assert_eq!(
            resolve_gh_scenario(&plain, &missing).unwrap(),
            None,
            "a scenario without gh runs without a built mock_gh"
        );
    }

    #[test]
    fn foreground_runs_leave_the_background_flag_out() {
        let foreground = Config {
            background: false,
            ..config()
        };
        let plain = scenario(r#"{"cmd": "state"}"#);
        let command = launcher(&foreground, &plain).command();
        let args: Vec<&OsStr> = command.get_args().collect();
        assert_eq!(args[..2], ["--automation", "--data-dir"]);

        // A scenario can ask for the foreground in a background run.
        let background = config();
        let asks = scenario("{\"scenario\": {\"foreground\": true}}\n{\"cmd\": \"state\"}");
        let command = launcher(&background, &asks).command();
        let args: Vec<&OsStr> = command.get_args().collect();
        assert_eq!(args[..2], ["--automation", "--data-dir"]);
    }

    #[test]
    fn data_dirs_live_in_the_output_dir_and_start_empty() {
        let root = tempfile::tempdir().unwrap();
        let out_dir = root.path().join("demo");
        let data_dir = data_dir(&out_dir);
        assert_eq!(data_dir, out_dir.join("data"));

        prepare_dirs(&out_dir, &data_dir).unwrap();
        std::fs::write(out_dir.join("01-old.png"), b"x").unwrap();
        std::fs::write(out_dir.join(GH_LOG_FILE), b"{}\n").unwrap();
        std::fs::write(data_dir.join("settings.json"), b"x").unwrap();
        prepare_dirs(&out_dir, &data_dir).unwrap();
        assert!(data_dir.is_dir());
        assert_eq!(
            std::fs::read_dir(&out_dir).unwrap().count(),
            1,
            "only data/"
        );
        assert_eq!(std::fs::read_dir(&data_dir).unwrap().count(), 0);
    }

    #[test]
    fn one_runner_at_a_time_per_output_root() {
        let root = tempfile::tempdir().unwrap();
        let out_root = root.path().join("e2e");
        let first = lock_out_root(&out_root).unwrap();
        assert!(out_root.join(LOCK_FILE).is_file());

        let err = format!("{:#}", lock_out_root(&out_root).unwrap_err());
        assert!(err.contains("another e2e run is using"), "{err}");
        assert!(
            err.ends_with("wait for it to finish or pass a different --out"),
            "{err}"
        );
        let _elsewhere = lock_out_root(&root.path().join("other"))
            .expect("a different output root is independent");

        drop(first);
        lock_out_root(&out_root).expect("free again once the first run ends");
    }

    #[test]
    fn restarted_launches_log_separately() {
        assert_eq!(stderr_log_name(1), "app.stderr.log");
        assert_eq!(stderr_log_name(2), "app.2.stderr.log");
        assert_eq!(stderr_log_name(3), "app.3.stderr.log");
    }

    #[test]
    fn restarts_keep_the_environment_and_can_swap_app_args() {
        let config = config();
        let scenario = scenario(
            "{\"scenario\": {\"gh\": \"gh.json\", \"app_args\": [\"--a\"], \"env\": {\"X\": \"1\"}}}
{\"cmd\": \"state\"}",
        );
        let mut launcher = launcher(&config, &scenario);
        let before = envs(&launcher.command());
        launcher.app_args = vec!["--b".into(), "${data_dir}".into()];
        let command = launcher.command();
        assert_eq!(envs(&command), before);
        let args: Vec<&OsStr> = command.get_args().collect();
        assert_eq!(&args[6..], ["--b", "data"]);
        // Without --data-dir: the app finds the same data dir through DV_DATA_DIR.
        launcher.data_dir_arg = false;
        let command = launcher.command();
        let args: Vec<&OsStr> = command.get_args().collect();
        assert!(!args.contains(&OsStr::new("--data-dir")), "{args:?}");
        assert_eq!(envs(&command), before, "still hermetic");
        assert_eq!(envs(&command)[DATA_DIR_VAR].as_deref(), Some("data"));
    }

    #[test]
    fn missing_apps_are_reported() {
        let config = Config {
            app: PathBuf::from("definitely/not/here.exe"),
            ..config()
        };
        let scenario = scenario(r#"{"cmd": "state"}"#);
        let err = launcher(&config, &scenario)
            .spawn()
            .err()
            .expect("a missing app must fail");
        assert!(err.to_string().contains("not found"), "{err}");
        assert!(err.to_string().contains("cargo build -p dv"), "{err}");
    }

    #[test]
    fn gh_logs_read_as_an_array_of_calls() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(GH_LOG_FILE);
        assert_eq!(
            read_gh_log(&path).unwrap(),
            json!([]),
            "no log yet: no calls"
        );

        std::fs::write(
            &path,
            "{\"argv\": [\"pr\", \"view\", \"1\"], \"stdin\": \"\", \"matched\": 0}\n\n\
             {\"argv\": [\"auth\", \"status\"], \"stdin\": \"\", \"matched\": null}\n",
        )
        .unwrap();
        assert_eq!(
            read_gh_log(&path).unwrap(),
            json!([
                {"argv": ["pr", "view", "1"], "stdin": "", "matched": 0},
                {"argv": ["auth", "status"], "stdin": "", "matched": null}
            ])
        );

        std::fs::write(&path, "{\"argv\": []}\r\nnot json\r\n").unwrap();
        let err = format!("{:#}", read_gh_log(&path).unwrap_err());
        assert!(
            err.contains(&format!("{}:2: not a JSON call record", path.display())),
            "{err}"
        );
    }

    #[cfg(windows)]
    fn exit_status(code: u32) -> std::process::ExitStatus {
        std::os::windows::process::ExitStatusExt::from_raw(code)
    }

    #[cfg(unix)]
    fn exit_status(code: u32) -> std::process::ExitStatus {
        std::os::unix::process::ExitStatusExt::from_raw((code as i32) << 8)
    }

    #[test]
    fn exit_checks_cover_failure_and_hangs() {
        assert!(check_exit(Some(exit_status(0))).is_ok());
        let err = check_exit(Some(exit_status(101))).unwrap_err().to_string();
        assert!(
            err.contains("the app exited with")
                && err.contains("101")
                && err.contains("after quit"),
            "{err}"
        );
        let err = check_exit(None).unwrap_err().to_string();
        assert!(err.contains("did not exit within 15s of quit"), "{err}");
    }
}
