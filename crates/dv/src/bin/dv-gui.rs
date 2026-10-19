//! `dv-gui`: the binary that runs the app in its own process (docs/PLAN.md §13.1). On Windows
//! the windows-subsystem `dv-gui.exe`, with no console to flash: the Start menu's shortcut,
//! Velopack's main executable, and what `dv.exe` starts for `dv` and `dv open` beside it. On
//! macOS the app bundle's executable (`dv.app/Contents/MacOS/dv-gui`): what the Dock, Finder
//! and Velopack start, and what the `dv` beside it starts. Linux ships `dv` alone. Everything
//! is in the crate's library; off Windows the attribute does nothing.

#![windows_subsystem = "windows"]

fn main() -> std::process::ExitCode {
    dv::main(dv::Entry::Gui)
}
