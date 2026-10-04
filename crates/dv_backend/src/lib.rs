//! The `Backend` interface between dv and a repo's location: the in-process `LocalBackend`, the
//! JSON-lines stdio protocol, the server loop behind `dv backend --stdio`, the `RemoteBackend`
//! client and the install of the Linux `dv` into WSL distros (docs/PLAN.md D6, §5).
//!
//! gpui: **never**. The Linux build of this crate is the WSL backend, a static musl binary
//! without a window system; `script/check` fails when gpui appears in its dependency tree
//! (PLAN §4, §13.2).
