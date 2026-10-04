//! Where dv keeps its files (PLAN §13.3, §14.1): `--data-dir` / `DV_DATA_DIR` when given; under
//! automation without one, a fresh temporary folder (removed again when dv exits), so a scripted
//! run never reaches the real one; otherwise the platform's config dir plus `dv` (`%APPDATA%\dv` on Windows, which also
//! holds the first implementation's files: `dv_ui::services::settings` reads it leniently).

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// How the data dir was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// `--data-dir` or `DV_DATA_DIR`.
    Explicit,
    /// A temporary folder for this automation run.
    Automation,
    /// The platform's config dir.
    Platform,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "--data-dir / DV_DATA_DIR",
            Self::Automation => "automation without --data-dir: a fresh temporary folder",
            Self::Platform => "the platform's config folder",
        }
    }
}

/// The data dir and how it was chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub dir: PathBuf,
    pub source: Source,
}

/// Where the data dir is for this process (see the module docs). Nothing is created here.
pub fn resolve(explicit: Option<PathBuf>, automation: bool) -> Result<Place> {
    choose(
        explicit,
        automation,
        std::process::id(),
        &std::env::temp_dir(),
        dirs::config_dir(),
    )
}

fn choose(
    explicit: Option<PathBuf>,
    automation: bool,
    pid: u32,
    temp: &Path,
    config: Option<PathBuf>,
) -> Result<Place> {
    if let Some(dir) = explicit.filter(|dir| !dir.as_os_str().is_empty()) {
        return Ok(Place {
            dir,
            source: Source::Explicit,
        });
    }
    if automation {
        return Ok(Place {
            dir: temp.join(format!("dv-automation-{pid}")),
            source: Source::Automation,
        });
    }
    let config = config.context(
        "this system has no config folder for dv's settings; pass --data-dir or set DV_DATA_DIR",
    )?;
    Ok(Place {
        dir: config.join("dv"),
        source: Source::Platform,
    })
}

/// Creates the data dir (and its parents) and returns its absolute form. An automation run's
/// temporary folder starts empty: one left by an earlier process with the same id is cleared.
pub fn prepare(place: &Place) -> Result<PathBuf> {
    let dir = &place.dir;
    if place.source == Source::Automation && dir.exists() {
        std::fs::remove_dir_all(dir)
            .with_context(|| format!("clearing the stale data directory {}", dir.display()))?;
    }
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating the data directory {}", dir.display()))?;
    std::path::absolute(dir).with_context(|| format!("resolving {}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_dir_wins() {
        let place = choose(
            Some(PathBuf::from("D:/data")),
            true,
            7,
            Path::new("C:/Temp"),
            Some(PathBuf::from("C:/Users/me/AppData/Roaming")),
        )
        .unwrap();
        assert_eq!(place.dir, PathBuf::from("D:/data"));
        assert_eq!(place.source, Source::Explicit);
    }

    #[test]
    fn automation_never_uses_the_real_dir() {
        for explicit in [None, Some(PathBuf::new())] {
            let place = choose(
                explicit,
                true,
                42,
                Path::new("C:/Temp"),
                Some(PathBuf::from("C:/Users/me/AppData/Roaming")),
            )
            .unwrap();
            assert_eq!(place.dir, Path::new("C:/Temp").join("dv-automation-42"));
            assert_eq!(place.source, Source::Automation);
        }
    }

    #[test]
    fn otherwise_the_config_dir() {
        let place = choose(
            None,
            false,
            1,
            Path::new("C:/Temp"),
            Some(PathBuf::from("C:/Users/me/AppData/Roaming")),
        )
        .unwrap();
        assert_eq!(
            place.dir,
            Path::new("C:/Users/me/AppData/Roaming").join("dv")
        );
        assert_eq!(place.source, Source::Platform);
        assert!(choose(None, false, 1, Path::new("C:/Temp"), None).is_err());
    }

    #[test]
    fn prepare_creates_the_dir_and_clears_a_stale_automation_one() {
        let root = tempfile::tempdir().unwrap();
        let explicit = Place {
            dir: root.path().join("a").join("b"),
            source: Source::Explicit,
        };
        let dir = prepare(&explicit).unwrap();
        assert!(dir.is_absolute() && dir.is_dir());
        std::fs::write(dir.join("settings.json"), "{}").unwrap();
        prepare(&explicit).unwrap();
        assert!(
            dir.join("settings.json").exists(),
            "an explicit dir is kept"
        );

        let automation = Place {
            dir: root.path().join("dv-automation-1"),
            source: Source::Automation,
        };
        let dir = prepare(&automation).unwrap();
        std::fs::write(dir.join("settings.json"), "{}").unwrap();
        let dir = prepare(&automation).unwrap();
        assert!(
            !dir.join("settings.json").exists(),
            "a fresh automation dir"
        );
    }
}
