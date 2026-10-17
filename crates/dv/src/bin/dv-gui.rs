//! `dv-gui.exe`: the windows-subsystem binary (docs/PLAN.md §13.1) that runs the app in its own
//! process, with no console to flash: the Start menu's shortcut, Velopack's main executable,
//! and what `dv.exe` starts for `dv` and `dv open` beside it. Everything is in the crate's
//! library; elsewhere than Windows the attribute does nothing and `dv` alone is shipped.

#![windows_subsystem = "windows"]

fn main() -> std::process::ExitCode {
    dv::main(dv::Entry::Gui)
}
