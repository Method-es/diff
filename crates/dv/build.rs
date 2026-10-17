//! On Windows, gives dv.exe and dv-gui.exe their icon and version information (docs/PLAN.md
//! §13.1, DESIGN §1.7).
//!
//! The icon (`resources/windows/dv.ico`, made from `dv.svg`; `tests/app_icon.rs` keeps the two
//! in step) is resource 1, the one gpui loads for its windows, so the title bar, the taskbar
//! and Alt Tab show it as Explorer does. The version resource carries Cargo's version as
//! numbers and the release's own (`DV_VERSION`, which `script/package` sets) as text, which
//! Task Manager and the file's properties show. Elsewhere this does nothing: the headless
//! Linux build and macOS have no resources.

use std::path::PathBuf;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=resources/windows/dv.ico");
    println!("cargo::rerun-if-env-changed=DV_VERSION");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .expect("cargo sets CARGO_MANIFEST_DIR for build scripts"),
    );
    let out_dir =
        PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR for build scripts"));
    let icon = manifest_dir.join("resources/windows/dv.ico");
    let rc = out_dir.join("dv.rc");
    std::fs::write(&rc, resource_script(&icon.display().to_string()))
        .expect("writing dv.rc into OUT_DIR");
    // The binaries only: tests keep gpui's default icon. dv-gui exists with the app alone.
    let mut bins = vec!["dv"];
    if std::env::var_os("CARGO_FEATURE_GUI").is_some() {
        bins.push("dv-gui");
    }
    embed_resource::compile_for(&rc, bins, embed_resource::NONE)
        .manifest_optional()
        .expect("compiling dv.rc (rc.exe comes with the Windows SDK, as the MSVC linker does)");
}

/// The resource script: the icon at `icon`, and the version.
fn resource_script(icon: &str) -> String {
    let part = |name: &str| {
        std::env::var(name)
            .ok()
            .and_then(|part| part.parse::<u16>().ok())
            .expect("cargo sets the package's version parts, each a number")
    };
    let (major, minor, patch) = (
        part("CARGO_PKG_VERSION_MAJOR"),
        part("CARGO_PKG_VERSION_MINOR"),
        part("CARGO_PKG_VERSION_PATCH"),
    );
    let version = std::env::var("DV_VERSION")
        .ok()
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| {
            std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION")
        });
    // Strings in a resource script take C escapes.
    let icon = icon.replace('\\', "\\\\");
    let version = version.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3F
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "FileDescription", "dv"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "dv"
            VALUE "ProductName", "dv"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    )
}
