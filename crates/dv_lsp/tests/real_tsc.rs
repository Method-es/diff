//! The real TypeScript check (docs/PLAN.md §18.7), opt-in: with `DV_TEST_TSC` naming a
//! TypeScript 7 native `tsc`, a fixture project gets hover, definition and references from the
//! real server, through `LocalBackend::spawn_stdio` as the app runs it. It reads only the
//! fixture (a temp dir) and installs nothing; without the variable it passes at once.
//!
//! ```text
//! DV_TEST_TSC=/path/to/node_modules/@typescript/typescript-linux-x64/lib/tsc \
//!     cargo test -p dv_lsp --test real_tsc
//! ```

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use dv_core::backend::{Backend, LocalBackend, LocalBackendOptions};
use dv_lsp::{
    CommandSource, HoverPart, LspClient, PositionEncoding, ServerCommand, ServerStatus,
    TextPosition, Timeouts, UriTarget, languages,
};
use dv_test_support::FixtureRepo;

const GREET: &str = "/** Says hello to someone. */
export function greet(name: string): string {
  return `Hello, ${name}!`;
}
";

const MAIN: &str = "import { greet } from \"./greet\";

const message = greet(\"dv\");
console.log(message, greet(\"again\"));
";

#[test]
fn hover_definition_and_references_from_tsc() {
    let Some(tsc) = std::env::var("DV_TEST_TSC").ok().filter(|v| !v.is_empty()) else {
        eprintln!("skipping: DV_TEST_TSC doesn't name a TypeScript 7 tsc");
        return;
    };
    let repo = FixtureRepo::new();
    repo.write(
        "tsconfig.json",
        r#"{"compilerOptions": {"strict": true, "target": "es2022", "module": "esnext", "moduleResolution": "bundler"}, "include": ["src"]}"#,
    );
    repo.write("package.json", r#"{"name": "fixture", "type": "module"}"#);
    repo.write("src/greet.ts", GREET);
    repo.write("src/main.ts", MAIN);
    repo.commit("fixture");
    let backend: Arc<dyn Backend> = Arc::new(
        LocalBackend::open_with(
            repo.path(),
            LocalBackendOptions {
                env: repo.env_overlay(),
            },
        )
        .unwrap(),
    );
    let spec = &languages::typescript::SPEC;
    let command = ServerCommand {
        spec_id: spec.id,
        program: tsc,
        args: spec.args.iter().map(|arg| arg.to_string()).collect(),
        root: String::new(),
        source: CommandSource::Override("DV_TEST_TSC".into()),
        env: Vec::new(),
    };
    let generous = Duration::from_secs(60);
    let client = LspClient::start(
        backend,
        &command,
        dv_lsp::server_config(spec, &command),
        Arc::new(|_| {}),
        Timeouts {
            initialize: generous,
            hover: generous,
            definition: generous,
            references: generous,
            ..Timeouts::default()
        },
    )
    .unwrap_or_else(|err| panic!("starting {}: {err}", command.command_line()));
    assert_eq!(client.encoding(), PositionEncoding::Utf8);
    client
        .open_document("src/main.ts", "typescript", MAIN)
        .unwrap();

    // `greet` in `const message = greet("dv");`.
    let at = TextPosition::new(2, 18);
    let hover = client
        .hover("src/main.ts", at)
        .wait()
        .unwrap()
        .expect("tsc answers a hover on greet");
    let text: String = hover
        .parts
        .iter()
        .map(|part| match part {
            HoverPart::Markdown(text) | HoverPart::Plain(text) => text.clone(),
            HoverPart::Code { text, .. } => text.clone(),
        })
        .collect();
    assert!(
        text.contains("greet(name: string): string"),
        "the signature: {text}"
    );
    assert!(text.contains("Says hello to someone."), "the docs: {text}");

    let definitions = client.definition("src/main.ts", at).wait().unwrap();
    let places: Vec<(UriTarget, u32, u32)> = definitions
        .iter()
        .map(|place| {
            (
                place.target.clone(),
                place.range.start.line,
                place.range.start.column,
            )
        })
        .collect();
    assert!(
        places.contains(&(UriTarget::Worktree("src/greet.ts".into()), 1, 16)),
        "the definition: {definitions:?}"
    );

    let references = client.references("src/main.ts", at, true).wait().unwrap();
    let lines: BTreeSet<(String, u32)> = references
        .iter()
        .filter_map(|place| match &place.target {
            UriTarget::Worktree(rel) => Some((rel.clone(), place.range.start.line)),
            UriTarget::Outside { .. } => None,
        })
        .collect();
    let expected: BTreeSet<(String, u32)> = [
        ("src/greet.ts".to_string(), 1),
        ("src/main.ts".into(), 0),
        ("src/main.ts".into(), 2),
        ("src/main.ts".into(), 3),
    ]
    .into_iter()
    .collect();
    assert_eq!(lines, expected, "the references: {references:?}");
    // A clean stop: tsc answers shutdown and ends on exit well within the grace period,
    // never killed (it exits 1 even so).
    let started = std::time::Instant::now();
    let status = client.stop();
    assert!(
        matches!(status, ServerStatus::Stopped(Some(_))),
        "the stop: {status:?}"
    );
    assert!(
        started.elapsed() < Timeouts::default().stop_grace,
        "the stop took {:?}",
        started.elapsed()
    );
}
