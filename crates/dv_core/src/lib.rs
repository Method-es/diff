//! dv's headless domain logic: repo locations, review targets, the git runner, the diff pipeline,
//! the review event store, anchoring and relocation, and GitHub through `gh`
//! (docs/PLAN.md §2, §6–§9).
//!
//! gpui: **never**. This crate runs in the CLI, inside WSL distros as part of the backend, and in
//! fast headless tests; `script/check` fails when gpui appears in its dependency tree
//! (PLAN §4, §13.2).
