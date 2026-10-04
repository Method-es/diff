//! The `dv` command tree (clap), the versioned JSON output DTOs agents parse, and the setup/doctor
//! checks shared by the CLI and the onboarding screen (docs/PLAN.md §10, §12).
//!
//! gpui: **never**. The CLI runs headless, on Windows and inside WSL distros; `script/check` fails
//! when gpui appears in its dependency tree (PLAN §4, §13.2).
