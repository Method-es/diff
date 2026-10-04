//! Bundled assets: icons through gpui's [`AssetSource`] ([`Assets`]), fonts through
//! [`load_fonts`].
//!
//! Asset paths are relative to the repo's `assets/` directory (`icons/folder.svg`). Everything is
//! embedded in the binary, so an installed dv needs no files beside it.

use std::borrow::Cow;

use anyhow::{Result, bail};
use gpui::{App, AssetSource, SharedString};

/// The app's [`AssetSource`]: the icons in [`crate::icons`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }
        match crate::icons::FILES.iter().find(|(name, _)| *name == path) {
            Some((_, bytes)) => Ok(Some(Cow::Borrowed(*bytes))),
            None => bail!("no bundled asset at {path:?}"),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths: Vec<SharedString> = crate::icons::FILES
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::new_static(name))
            .collect();
        paths.sort();
        Ok(paths)
    }
}

/// The bundled font families and their files (DESIGN §1.4).
pub mod fonts {
    /// UI text: Geist 400/500/600 (OFL, `assets/fonts/Geist/OFL.txt`).
    pub const GEIST: &str = "Geist";
    /// Code, keycaps and counts: JetBrains Mono 400 (OFL,
    /// `assets/fonts/jetbrains-mono/OFL.txt`).
    pub const JETBRAINS_MONO: &str = "JetBrains Mono";

    macro_rules! font {
        ($path:literal) => {
            (
                $path,
                include_bytes!(concat!("../../../assets/fonts/", $path)).as_slice(),
            )
        };
    }

    /// Every bundled font file as `(path under assets/fonts, bytes)`.
    pub const FILES: &[(&str, &[u8])] = &[
        font!("Geist/Geist-Regular.ttf"),
        font!("Geist/Geist-Medium.ttf"),
        font!("Geist/Geist-SemiBold.ttf"),
        font!("jetbrains-mono/JetBrainsMono-Regular.ttf"),
    ];
}

/// Registers the bundled fonts with gpui's text system. Call once at startup, before any window
/// renders text in them.
pub fn load_fonts(cx: &App) -> Result<()> {
    cx.text_system().add_fonts(
        fonts::FILES
            .iter()
            .map(|(_, bytes)| Cow::Borrowed(*bytes))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the (typographic, else legacy) family name from a TrueType `name` table.
    fn family_name(font: &[u8]) -> Option<String> {
        let be16 = |at: usize| u16::from_be_bytes([font[at], font[at + 1]]) as usize;
        let be32 = |at: usize| {
            u32::from_be_bytes([font[at], font[at + 1], font[at + 2], font[at + 3]]) as usize
        };
        let table = (0..be16(4))
            .map(|i| 12 + 16 * i)
            .find(|&record| &font[record..record + 4] == b"name")
            .map(|record| be32(record + 8))?;
        let strings = table + be16(table + 4);
        let mut legacy = None;
        for i in 0..be16(table + 2) {
            let record = table + 6 + 12 * i;
            let (platform, language, name_id) = (be16(record), be16(record + 4), be16(record + 6));
            if platform != 3 || language != 0x409 {
                continue;
            }
            let (len, offset) = (be16(record + 8), be16(record + 10));
            let utf16: Vec<u16> = font[strings + offset..strings + offset + len]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_be_bytes(*pair))
                .collect();
            let name = String::from_utf16(&utf16).ok()?;
            match name_id {
                16 => return Some(name),
                1 => legacy = Some(name),
                _ => {}
            }
        }
        legacy
    }

    #[test]
    fn font_files_declare_the_family_names_we_use() {
        for (path, bytes) in fonts::FILES {
            assert_eq!(&bytes[..4], &[0, 1, 0, 0], "{path} is not a TrueType font");
            let family = family_name(bytes).unwrap_or_else(|| panic!("{path} has no family"));
            let want = if path.starts_with("Geist/") {
                fonts::GEIST
            } else {
                fonts::JETBRAINS_MONO
            };
            assert_eq!(family, want, "{path}");
        }
    }

    #[test]
    fn icons_load_and_list() {
        let folder = Assets.load("icons/folder.svg").unwrap().unwrap();
        assert!(folder.starts_with(b"<svg"));
        assert!(Assets.load("icons/definitely-missing.svg").is_err());
        assert!(Assets.load("").unwrap().is_none());
        let icons = Assets.list("icons/").unwrap();
        assert_eq!(icons.len(), crate::DvIcon::ALL.len());
        assert!(icons.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(Assets.list("images/").unwrap().is_empty());
    }
}
