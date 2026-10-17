//! `dv_package version --base <version> --feed <dir> --stamp <yyyymmddhhmmss> [--version <v>]`:
//! prints the version of the next local release, or why there is none, exit 2
//! (`dv_package::release_version`, docs/PLAN.md §13.2). Run by `script/package`.

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: dv_package version --base <version> --feed <dir> \
                     --stamp <yyyymmddhhmmss> [--version <v>]";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(version) => {
            println!("{version}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("dv_package: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(args: Vec<String>) -> Result<String, String> {
    let mut args = args.into_iter();
    if args.next().as_deref() != Some("version") {
        return Err(USAGE.to_string());
    }
    let (mut base, mut feed, mut stamp, mut explicit) = (None, None, None, None);
    while let Some(arg) = args.next() {
        let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--base" => base = Some(value),
            "--feed" => feed = Some(PathBuf::from(value)),
            "--stamp" => stamp = Some(value),
            "--version" => explicit = Some(value).filter(|value| !value.is_empty()),
            other => return Err(format!("unknown argument {other}; {USAGE}")),
        }
    }
    let base = base.ok_or("--base is missing")?;
    let feed = feed.ok_or("--feed is missing")?;
    let stamp = stamp.unwrap_or_default();
    let newest = dv_package::newest_in_feed(&feed)?;
    dv_package::release_version(&base, explicit.as_deref(), &stamp, newest.as_ref())
        .map(|version| version.to_string())
}
