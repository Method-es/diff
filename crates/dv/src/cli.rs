//! dv's one command tree (PLAN §10.1): the GUI's options and `dv open` here, the headless
//! commands and the global flags from `dv_cli`, composed so `dv --help` lists every command.

use std::{fmt, path::PathBuf, str::FromStr};

use clap::{ArgGroup, Args, Parser, Subcommand};

/// Review diffs and GitHub PRs locally. `dv` alone opens the app on the current repository;
/// every command but `open` runs without a window.
// A repeated option takes its last value: the e2e runner passes the scenario's `--window-size`
// before the scenario's own `app_args`, which may give another (tests/e2e/README.md, `restart`).
#[derive(Parser, Debug, Clone, PartialEq)]
#[command(name = "dv", bin_name = "dv", version, args_override_self = true)]
#[command(group(ArgGroup::new("automation_mode").args(["automation", "automation_pipe"]).multiple(true)))]
pub struct Cli {
    /// Where dv keeps its settings [default: %APPDATA%\dv on Windows, the config dir elsewhere;
    /// under automation a fresh temporary folder].
    #[arg(long, env = "DV_DATA_DIR", value_name = "PATH")]
    pub data_dir: Option<PathBuf>,

    /// Accept automation commands as JSON lines on stdin (PLAN §14, tests/e2e/README.md).
    #[arg(long)]
    pub automation: bool,

    /// Serve automation on the Windows named pipe `\\.\pipe\<NAME>` instead of stdio, so a
    /// driver can attach to a running app. Implies `--automation`.
    #[arg(long, value_name = "NAME")]
    pub automation_pipe: Option<String>,

    /// Under automation, open every window unfocused, behind all other windows and out of the
    /// taskbar, so scripted runs never disturb the person using the machine. The e2e runner
    /// passes it unless given `--foreground`. Needs `--automation` or `--automation-pipe`.
    #[arg(long, requires = "automation_mode")]
    pub automation_background: bool,

    /// Window size in logical pixels, e.g. 1440x900 (minimum 1100x700) [default: 1440x900].
    #[arg(long, value_name = "WxH")]
    pub window_size: Option<WindowSize>,

    /// Run the app in this process rather than starting it detached: what a detaching `dv`
    /// passes the copy it starts (PLAN §13.1).
    #[arg(long, hide = true)]
    pub no_detach: bool,

    #[command(flatten)]
    pub global: dv_cli::GlobalArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// Whether the automation driver runs, over stdio or a named pipe.
    pub fn automation_enabled(&self) -> bool {
        self.automation || self.automation_pipe.is_some()
    }
}

/// The subcommands: `open`, the GUI's, and the headless ones (`dv_cli`).
#[derive(Subcommand, Debug, Clone, PartialEq)]
pub enum Command {
    /// Open the app on a repository (the switcher on its changes) or, with --review, on a
    /// review
    Open(OpenArgs),

    #[command(flatten)]
    Headless(dv_cli::Command),
}

/// `dv open [<path> | <PR-url> | #<n>] [--review <id>]`.
#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct OpenArgs {
    /// A path inside a repository's working tree, or \\wsl.localhost\<distro>\<path>
    /// [default: the current directory]; a PR (URL or #<n>) in a later version
    #[arg(value_name = "PATH")]
    pub target: Option<String>,
}

/// A window size in logical pixels, written `WIDTHxHEIGHT`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
}

impl WindowSize {
    /// DESIGN §2's reference size.
    pub const DEFAULT: WindowSize = WindowSize {
        width: 1440,
        height: 900,
    };
    /// Below 1100 wide the panel would have to collapse (DESIGN §2).
    pub const MIN: WindowSize = WindowSize {
        width: 1100,
        height: 700,
    };

    /// This size, grown to at least [`Self::MIN`] in each dimension.
    pub fn at_least_min(self) -> Self {
        Self {
            width: self.width.max(Self::MIN.width),
            height: self.height.max(Self::MIN.height),
        }
    }
}

impl fmt::Display for WindowSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

impl FromStr for WindowSize {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (width, height) = value
            .trim()
            .split_once(['x', 'X', '×'])
            .ok_or_else(|| format!("expected WIDTHxHEIGHT, got {value:?}"))?;
        let parse = |part: &str, name: &str| {
            part.trim()
                .parse::<u32>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("{name} must be a positive whole number, got {part:?}"))
        };
        Ok(Self {
            width: parse(width, "width")?,
            height: parse(height, "height")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("dv").chain(args.iter().copied()))
    }

    #[test]
    fn defaults() {
        // `data_dir` is left out: clap reads DV_DATA_DIR, which an e2e run sets.
        let cli = Cli::try_parse_from(["dv"]).unwrap();
        assert!(!cli.automation);
        assert!(!cli.automation_enabled());
        assert_eq!(cli.automation_pipe, None);
        assert!(!cli.automation_background);
        assert_eq!(cli.window_size, None);
    }

    #[test]
    fn automation_pipe_implies_automation() {
        let cli = parse(&["--automation-pipe", "dv-e2e"]).unwrap();
        assert!(!cli.automation);
        assert!(cli.automation_enabled());
        assert_eq!(cli.automation_pipe.as_deref(), Some("dv-e2e"));
        assert!(parse(&["--automation-pipe"]).is_err(), "needs a name");
        assert!(parse(&["--automation"]).unwrap().automation_enabled());
    }

    #[test]
    fn all_flags() {
        let cli = parse(&[
            "--data-dir",
            "target/tmp/data",
            "--automation",
            "--window-size",
            "1100x700",
            "--automation-background",
        ])
        .unwrap();
        assert_eq!(cli.data_dir, Some(PathBuf::from("target/tmp/data")));
        assert!(cli.automation);
        assert!(cli.automation_background);
        assert_eq!(
            cli.window_size,
            Some(WindowSize {
                width: 1100,
                height: 700
            })
        );
    }

    #[test]
    fn window_size_syntax() {
        let size = |s: &str| s.parse::<WindowSize>();
        assert_eq!(
            size("2560x1440"),
            Ok(WindowSize {
                width: 2560,
                height: 1440
            })
        );
        assert_eq!(
            size(" 800 X 600 ").map(|s| s.to_string()),
            Ok("800x600".into())
        );
        assert_eq!(size("1440×900").map(|s| s.width), Ok(1440));
        assert!(size("1440").is_err());
        assert!(size("0x900").is_err());
        assert!(size("-5x900").is_err());
        assert!(size("wide x tall").is_err());
        assert!(parse(&["--window-size", "big"]).is_err());
    }

    #[test]
    fn window_size_respects_the_minimum() {
        let small = WindowSize {
            width: 800,
            height: 1000,
        };
        assert_eq!(
            small.at_least_min(),
            WindowSize {
                width: 1100,
                height: 1000
            }
        );
        assert_eq!(WindowSize::DEFAULT.at_least_min(), WindowSize::DEFAULT);
    }

    #[test]
    fn a_repeated_option_takes_its_last_value() {
        let cli = parse(&["--window-size", "1440x900", "--window-size", "900x600"]).unwrap();
        assert_eq!(
            cli.window_size,
            Some(WindowSize {
                width: 900,
                height: 600
            })
        );
    }

    #[test]
    fn background_needs_automation() {
        assert!(parse(&["--automation-background"]).is_err());
        assert!(parse(&["--automation", "--automation-background"]).is_ok());
        assert!(parse(&["--automation-pipe", "p", "--automation-background"]).is_ok());
    }

    #[test]
    fn rejects_unknown_flags() {
        assert!(parse(&["--frobnicate"]).is_err());
    }

    #[test]
    fn one_tree_holds_the_app_and_the_headless_commands() {
        let cli = parse(&[]).unwrap();
        assert_eq!(cli.command, None);
        assert!(!cli.no_detach);

        let cli = parse(&["status", "--json"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Command::Headless(dv_cli::Command::Status))
        );
        assert!(cli.global.json);
        assert!(matches!(
            parse(&["review", "list", "--all"]).unwrap().command,
            Some(Command::Headless(dv_cli::Command::Review(
                dv_cli::args::ReviewCommand::List(_)
            )))
        ));

        // The e2e runner's line: the app's flags first, then the scenario's `open`.
        let cli = parse(&[
            "--automation",
            "--automation-background",
            "--data-dir",
            "D:/data",
            "--window-size",
            "1440x900",
            "open",
            "repos/api-server",
            "--review",
            "r-01k6zzzzzzzzzzzzzzzzzzzzzz",
        ])
        .unwrap();
        assert!(cli.automation_enabled());
        assert_eq!(
            cli.command,
            Some(Command::Open(OpenArgs {
                target: Some("repos/api-server".into())
            }))
        );
        assert_eq!(
            cli.global.review.as_deref(),
            Some("r-01k6zzzzzzzzzzzzzzzzzzzzzz")
        );
        assert_eq!(
            parse(&["--no-detach", "open"]).unwrap().command,
            Some(Command::Open(OpenArgs { target: None }))
        );

        // The app's options belong to the app, not to the headless commands.
        assert!(parse(&["status", "--automation"]).is_err());
        assert!(parse(&["review", "list", "--window-size", "1440x900"]).is_err());
    }

    #[test]
    fn the_tree_is_consistent() {
        use clap::CommandFactory as _;
        Cli::command().debug_assert();
    }
}
