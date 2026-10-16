//! Code intelligence's language servers (docs/PLAN.md §18.4): the LSP client, the language
//! specs and how each finds its server.
//!
//! - [`client`]: [`LspClient`], one server started through `Backend::spawn_stdio` where the
//!   repository lives (inside the distro for a WSL repository, PLAN §18.3), with hover, go to
//!   definition and find references in UTF-8 columns and worktree paths.
//! - [`spec`] and [`languages`]: [`LanguageServerSpec`], a server as data, and the registry of
//!   them ([`languages::ALL`]); [`languages::typescript`] is TypeScript 7's native `tsc`.
//! - [`resolve`]: the [`Resolver`] that finds a spec's server for a file, through the
//!   `DV_LSP_<ID>` override ([`settings::Override`]), the project's own copy (Node's resolution,
//!   [`resolve::node`]) and `settings.json`'s `lsp.<id>.command` ([`LspSettings`]).
//! - [`restarts`]: when a server that ended starts again (PLAN §18.5).
//! - [`framing`], [`paths`], [`text`]: the base protocol's framing, a worktree's paths and
//!   URIs, positions and their encodings.
//!
//! gpui: **never**; `script/check` fails when gpui appears in its dependency tree (PLAN §4).
//! The app's code intelligence (`dv_ui::intel`) drives it.

pub mod client;
pub mod framing;
pub mod languages;
pub mod paths;
pub mod resolve;
pub mod restarts;
pub mod settings;
pub mod spec;
pub mod text;

pub use client::{
    ClientEvent, EventSink, HoverInfo, HoverPart, LspClient, Pending, Progress, RequestError,
    ServerConfig, ServerStatus, StartError, SymbolLocation, Timeouts,
};
pub use paths::{UriTarget, WorkspacePaths};
pub use resolve::{CommandSource, Missing, Platform, Resolution, Resolver, ServerCommand};
pub use restarts::Restarts;
pub use settings::{LspSettings, Override};
pub use spec::LanguageServerSpec;
pub use text::{PositionEncoding, TextPosition, TextRange};

/// What `initialize` and `workspace/configuration` tell `spec`'s server rooted at `command`'s
/// root.
pub fn server_config(spec: &LanguageServerSpec, command: &ServerCommand) -> ServerConfig {
    ServerConfig {
        root: command.root.clone(),
        initialization_options: (spec.initialization_options)(),
        settings: (spec.settings)(),
    }
}
