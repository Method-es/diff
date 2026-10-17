//! `script/package`'s helper (docs/PLAN.md §13.2): the version of a local release. Never
//! shipped; `script/package` runs it as `cargo run -q -p dv_package -- version …`.
//!
//! **The version rule.** Velopack updates an installed copy to a release only when it is newer
//! (SemVer), and `vpk pack` refuses one that isn't newer than its feed's newest, so every local
//! release must be newer than the last:
//! - by default it is the workspace's version with a prerelease of the UTC time it was packaged,
//!   `<version>-local.<yyyymmddhhmmss>` (`0.1.0-local.20261018093012`): later packages are
//!   newer, and the workspace's own `0.1.0`, once released, is newer than all of them;
//! - `script/package --version <semver>` names it instead;
//! - either way it must be newer than every release already in the output feed
//!   (`releases.<channel>.json`), else packaging stops before it builds anything.

use std::path::Path;

use semver::Version;

/// The version of a local release (module docs): `explicit` when given, else `base` with the
/// prerelease `local.<stamp>`; refused unless newer than `newest`, the feed's newest release.
pub fn release_version(
    base: &str,
    explicit: Option<&str>,
    stamp: &str,
    newest: Option<&Version>,
) -> Result<Version, String> {
    let version = match explicit {
        Some(explicit) => Version::parse(explicit.trim())
            .map_err(|error| format!("--version {explicit:?} isn't a SemVer version: {error}"))?,
        None => {
            let base = Version::parse(base.trim())
                .map_err(|error| format!("the workspace's version {base:?}: {error}"))?;
            if !base.pre.is_empty() || !base.build.is_empty() {
                return Err(format!(
                    "the workspace's version {base} has a prerelease or build part already; \
                     pass --version"
                ));
            }
            if stamp.len() != 14 || !stamp.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(format!("the time {stamp:?} isn't yyyymmddhhmmss"));
            }
            Version::parse(&format!("{base}-local.{stamp}"))
                .map_err(|error| format!("{base}-local.{stamp}: {error}"))?
        }
    };
    if let Some(newest) = newest
        && version <= *newest
    {
        return Err(format!(
            "{version} isn't newer than {newest}, the newest release in the feed: Velopack \
             updates only to a newer version (pass a higher --version)"
        ));
    }
    Ok(version)
}

/// The newest release in the feed folder `dir`: the highest `Version` of every asset in its
/// `releases.*.json` files (none there, or no folder: `None`).
pub fn newest_in_feed(dir: &Path) -> Result<Option<Version>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("reading {}: {error}", dir.display())),
    };
    let mut newest: Option<Version> = None;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("reading {}: {error}", dir.display()))?
            .path();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !(name.starts_with("releases.") && name.ends_with(".json")) {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("reading {}: {error}", path.display()))?;
        let feed: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| format!("{} isn't JSON: {error}", path.display()))?;
        let assets = feed
            .get("Assets")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("{} has no \"Assets\"", path.display()))?;
        for asset in assets {
            let Some(version) = asset.get("Version").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let version = Version::parse(version)
                .map_err(|error| format!("{}: version {version:?}: {error}", path.display()))?;
            if newest.as_ref().is_none_or(|newest| version > *newest) {
                newest = Some(version);
            }
        }
    }
    Ok(newest)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn a_local_release_is_the_workspace_version_with_its_time() {
        assert_eq!(
            release_version("0.1.0", None, "20261018093012", None),
            Ok(v("0.1.0-local.20261018093012"))
        );
        // A later package is newer, and the workspace's version itself newer than both.
        let first = release_version("0.1.0", None, "20261018093012", None).unwrap();
        let second = release_version("0.1.0", None, "20261018101500", Some(&first)).unwrap();
        assert!(second > first);
        assert!(v("0.1.0") > second);
        assert!(v("0.1.1-local.20250101000000") > v("0.1.0"));
    }

    #[test]
    fn a_release_must_be_newer_than_the_feeds_newest() {
        let newest = v("0.1.0-local.20261018101500");
        let refused = release_version("0.1.0", None, "20261018101500", Some(&newest));
        assert!(refused.unwrap_err().contains("isn't newer than"));
        assert!(release_version("0.1.0", None, "20261018090000", Some(&newest)).is_err());
        assert!(release_version("0.1.0", Some("0.1.0-local.1"), "", Some(&newest)).is_err());
        assert_eq!(
            release_version("0.1.0", Some(" 0.2.0 "), "", Some(&newest)),
            Ok(v("0.2.0"))
        );
    }

    #[test]
    fn what_isnt_a_version_or_a_time_is_refused() {
        assert!(release_version("0.1.0", Some("two"), "", None).is_err());
        assert!(release_version("0.1", None, "20261018093012", None).is_err());
        assert!(release_version("0.1.0-beta", None, "20261018093012", None).is_err());
        for stamp in ["", "2026101809301", "2026-10-18T09", "202610180930120"] {
            assert!(
                release_version("0.1.0", None, stamp, None).is_err(),
                "{stamp}"
            );
        }
    }

    #[test]
    fn the_feeds_newest_is_read_from_its_releases_files() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(newest_in_feed(&dir.path().join("none")), Ok(None));
        assert_eq!(newest_in_feed(dir.path()), Ok(None));
        std::fs::write(
            dir.path().join("releases.win.json"),
            r#"{"Assets": [
                {"PackageId": "dv", "Version": "0.1.0-local.20261018093012", "Type": "Full"},
                {"PackageId": "dv", "Version": "0.1.0-local.20261018101500", "Type": "Delta"},
                {"PackageId": "dv", "Version": "0.1.0-local.20261018100000", "Type": "Full"}
            ]}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("RELEASES"), "not JSON, Squirrel's old feed").unwrap();
        assert_eq!(
            newest_in_feed(dir.path()),
            Ok(Some(v("0.1.0-local.20261018101500")))
        );
        std::fs::write(dir.path().join("releases.beta.json"), "{}").unwrap();
        assert!(newest_in_feed(dir.path()).is_err());
    }
}
