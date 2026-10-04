//! `e2e`: run automation scenarios against a freshly started app
//! (docs/PLAN.md §14; format in tests/e2e/README.md).
//!
//! ```text
//! cargo run -p gpui_automation --bin e2e -- [options] <scenario.jsonl | dir | glob>...
//! ```

mod foreground;
mod process;
mod run;
mod scenario;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

use crate::run::{Config, Outcome};
use crate::scenario::Scenario;

const USAGE: &str = "\
usage: e2e [options] <scenario.jsonl | directory | glob>...

Runs each scenario against a fresh app process (--automation
--automation-background --data-dir <out>/<scenario>/data --window-size WxH,
with DV_DATA_DIR set to the same data dir and DV_GH to mock_gh) and exits
non-zero if any fails.

options:
  --app <exe>          app under test (default: <target>/debug/dv, .exe on
                       Windows)
  --mock-gh <exe>      the fake gh every run's DV_GH points at; scenarios
                       with a \"gh\" header need it built (default:
                       <target>/debug/mock_gh, .exe on Windows)
  --filter <text>      only run scenarios whose name contains <text>
  --out <dir>          screenshots and logs (default: <target>/e2e); one run
                       at a time per directory
  --timeout-ms <n>     base reply timeout per command (default 30000)
  --ready-timeout-ms <n>  how long the app may take to start (default 60000)
  --keep-data          keep each scenario's data dir after it passes
  --foreground         open the app's windows normally (focused, in front)
                       instead of in the background; see tests/e2e/README.md
  -v, --verbose        print every request and reply
  -h, --help           this help
";

struct Cli {
    app: Option<PathBuf>,
    mock_gh: Option<PathBuf>,
    filter: Option<String>,
    out: Option<PathBuf>,
    step_timeout: Duration,
    ready_timeout: Duration,
    keep_data: bool,
    foreground: bool,
    verbose: bool,
    scenarios: Vec<PathBuf>,
}

fn parse_cli(args: impl IntoIterator<Item = String>) -> Result<Option<Cli>> {
    let mut cli = Cli {
        app: None,
        mock_gh: None,
        filter: None,
        out: None,
        step_timeout: Duration::from_secs(30),
        ready_timeout: Duration::from_secs(60),
        keep_data: false,
        foreground: false,
        verbose: false,
        scenarios: Vec::new(),
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().with_context(|| format!("{name} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--app" => cli.app = Some(value("--app")?.into()),
            "--mock-gh" => cli.mock_gh = Some(value("--mock-gh")?.into()),
            "--filter" => cli.filter = Some(value("--filter")?),
            "--out" => cli.out = Some(value("--out")?.into()),
            "--timeout-ms" => cli.step_timeout = millis("--timeout-ms", &value("--timeout-ms")?)?,
            "--ready-timeout-ms" => {
                cli.ready_timeout = millis("--ready-timeout-ms", &value("--ready-timeout-ms")?)?
            }
            "--keep-data" => cli.keep_data = true,
            "--foreground" => cli.foreground = true,
            "-v" | "--verbose" => cli.verbose = true,
            flag if flag.starts_with('-') => bail!("unknown option {flag}\n\n{USAGE}"),
            path => cli.scenarios.push(path.into()),
        }
    }
    if cli.scenarios.is_empty() {
        bail!("no scenarios given\n\n{USAGE}");
    }
    Ok(Some(cli))
}

fn millis(flag: &str, value: &str) -> Result<Duration> {
    let ms: u64 = value
        .parse()
        .with_context(|| format!("{flag} expects milliseconds, got {value:?}"))?;
    Ok(Duration::from_millis(ms))
}

/// The workspace this binary was built from, for default paths that work
/// whatever the current directory is.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .filter(|root| root.join("Cargo.toml").is_file())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn target_dir(workspace: &Path) -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => workspace.join("target"),
    }
}

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

fn main() -> ExitCode {
    match real_main() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("e2e: {err:#}");
            ExitCode::from(2)
        }
    }
}

fn real_main() -> Result<ExitCode> {
    let Some(cli) = parse_cli(std::env::args().skip(1))? else {
        print!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    };
    process::install_interrupt_handler()?;

    let workspace = workspace_root();
    let target = target_dir(&workspace);
    let absolute = |path: PathBuf| std::path::absolute(&path).unwrap_or(path);
    let config = Config {
        app: absolute(
            cli.app
                .unwrap_or_else(|| target.join("debug").join(exe("dv"))),
        ),
        mock_gh: absolute(
            cli.mock_gh
                .unwrap_or_else(|| target.join("debug").join(exe("mock_gh"))),
        ),
        out_root: absolute(cli.out.unwrap_or_else(|| target.join("e2e"))),
        workspace,
        step_timeout: cli.step_timeout,
        ready_timeout: cli.ready_timeout,
        keep_data: cli.keep_data,
        background: !cli.foreground,
        verbose: cli.verbose,
    };
    // Held until the run ends: a second runner on the same output root would wipe this one's
    // scenario dirs.
    let _out_lock = run::lock_out_root(&config.out_root)?;
    // A background app must never take the foreground: watch for it across every scenario.
    let watch = config.background.then(foreground::Watch::start);

    let paths = scenario::discover(&cli.scenarios)?;
    let mut scenarios = Vec::new();
    for path in paths {
        let scenario = Scenario::load(&path)?;
        if cli
            .filter
            .as_deref()
            .is_none_or(|filter| scenario.name.contains(filter))
        {
            scenarios.push(scenario);
        }
    }
    if scenarios.is_empty() {
        bail!("no scenario matches the filter");
    }

    println!(
        "e2e: {} scenario(s) against {} in the {}",
        scenarios.len(),
        config.app.display(),
        if config.background {
            "background"
        } else {
            "foreground"
        }
    );
    let mut passed = 0;
    let mut skipped = 0;
    let mut failed = Vec::new();
    let mut interrupted = false;
    for scenario in &scenarios {
        if !scenario.runs_on(std::env::consts::OS) {
            skipped += 1;
            println!(
                "skip {}: runs on {} only",
                scenario.name,
                scenario.header.platforms.join(", ")
            );
            continue;
        }
        println!("---- {} ({})", scenario.name, scenario.path.display());
        if config.verbose
            && let Some(description) = &scenario.header.description
        {
            println!("     {description}");
        }
        let started = Instant::now();
        let outcome = run::run(scenario, &config, watch.as_ref());
        let seconds = started.elapsed().as_secs_f32();
        let out_dir = config.out_root.join(&scenario.name);
        match outcome {
            Outcome::Passed { steps, screenshots } => {
                passed += 1;
                println!(
                    "ok   {}: {steps} steps, {screenshots} screenshots, {seconds:.1}s -> {}",
                    scenario.name,
                    out_dir.display()
                );
            }
            Outcome::Failed { line, message } => {
                let location = match line {
                    Some(line) => format!("{}:{line}", scenario.path.display()),
                    None => scenario.path.display().to_string(),
                };
                println!(
                    "FAIL {} ({seconds:.1}s)\n  at {location}\n  {message}",
                    scenario.name
                );
                println!("  logs and screenshots: {}", out_dir.display());
                failed.push(scenario.name.clone());
            }
            Outcome::Interrupted => {
                println!("INTERRUPTED {}", scenario.name);
                interrupted = true;
                break;
            }
        }
    }

    println!(
        "summary: {passed} passed, {} failed{}{}",
        failed.len(),
        if skipped > 0 {
            format!(", {skipped} skipped on {}", std::env::consts::OS)
        } else {
            String::new()
        },
        if interrupted { ", interrupted" } else { "" }
    );
    if !failed.is_empty() {
        println!("failed: {}", failed.join(", "));
    }
    Ok(if interrupted {
        ExitCode::from(130)
    } else if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn cli_parses_options_and_scenarios() {
        let cli = parse_cli(args(&[
            "--app",
            "h.exe",
            "--mock-gh",
            "gh.exe",
            "--filter",
            "smoke",
            "--timeout-ms",
            "500",
            "-v",
            "a.jsonl",
            "tests/e2e",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(cli.app, Some(PathBuf::from("h.exe")));
        assert_eq!(cli.mock_gh, Some(PathBuf::from("gh.exe")));
        assert_eq!(cli.filter.as_deref(), Some("smoke"));
        assert_eq!(cli.step_timeout, Duration::from_millis(500));
        assert!(cli.verbose);
        assert!(!cli.foreground, "background by default");
        assert!(
            parse_cli(args(&["--foreground", "a.jsonl"]))
                .unwrap()
                .unwrap()
                .foreground
        );
        assert_eq!(
            cli.scenarios,
            [PathBuf::from("a.jsonl"), PathBuf::from("tests/e2e")]
        );
    }

    #[test]
    fn cli_rejects_bad_input() {
        assert!(parse_cli(args(&[])).is_err());
        assert!(parse_cli(args(&["--bogus", "a.jsonl"])).is_err());
        assert!(parse_cli(args(&["--app"])).is_err());
        assert!(parse_cli(args(&["--mock-gh"])).is_err());
        assert!(parse_cli(args(&["--timeout-ms", "soon", "a.jsonl"])).is_err());
        assert!(parse_cli(args(&["--help"])).unwrap().is_none());
    }

    #[test]
    fn workspace_root_is_this_repo() {
        assert!(
            workspace_root()
                .join("crates")
                .join("gpui_automation")
                .is_dir()
        );
    }
}
