//! The remote side of dv's `Backend` interface: the JSON-lines stdio protocol, the server loop
//! behind `dv backend --stdio`, the `RemoteBackend` client and the install of the Linux `dv` into
//! WSL distros (docs/PLAN.md D6, §5). The trait itself and the in-process `LocalBackend` live in
//! `dv_core::backend`, which the diff pipeline and the review store are written against
//! (PLAN §5.1).
//!
//! gpui: **never**. The Linux build of this crate is the WSL backend, a static musl binary
//! without a window system; `script/check` fails when gpui appears in its dependency tree
//! (PLAN §4, §13.2).
