//! The real JavaScript servers (docs/PLAN.md §18.7), opt-in: with `DV_TEST_JS_RUNTIME` naming
//! bun or node and `DV_TEST_TAILWIND` naming `@tailwindcss/language-server`'s entry script
//! (`bin/tailwindcss-language-server`), or `DV_TEST_CSS` naming
//! `vscode-langservers-extracted`'s `bin/vscode-css-language-server`, a fixture gets a hover
//! from the real server, run as the app runs it (`<runtime> <script> --stdio`) through
//! `LocalBackend::spawn_stdio`. It reads only the fixture (a temp dir) and installs nothing;
//! without the variables each test passes at once.
//!
//! ```text
//! DV_TEST_JS_RUNTIME=$HOME/.bun/bin/bun \
//! DV_TEST_TAILWIND=$HOME/.bun/install/global/node_modules/@tailwindcss/language-server/bin/tailwindcss-language-server \
//!     cargo test -p dv_lsp --test real_js
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use dv_core::backend::{Backend, LocalBackend, LocalBackendOptions};
use dv_lsp::{
    CommandSource, HoverPart, LanguageServerSpec, LspClient, ServerCommand, TextPosition, Timeouts,
    languages,
};
use dv_test_support::FixtureRepo;

/// The runtime and the entry script `var` names, when both are set.
fn configured(var: &str) -> Option<(String, String)> {
    let get = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    match (get("DV_TEST_JS_RUNTIME"), get(var)) {
        (Some(runtime), Some(script)) => Some((runtime, script)),
        _ => {
            eprintln!("skipping: DV_TEST_JS_RUNTIME and {var} aren't both set");
            None
        }
    }
}

/// `spec`'s server, its script run by `runtime`, over `repo`.
fn start(
    repo: &FixtureRepo,
    spec: &LanguageServerSpec,
    runtime: String,
    script: String,
) -> LspClient {
    let backend: Arc<dyn Backend> = Arc::new(
        LocalBackend::open_with(
            repo.path(),
            LocalBackendOptions {
                env: repo.env_overlay(),
            },
        )
        .unwrap(),
    );
    let command = ServerCommand {
        spec_id: spec.id,
        program: runtime,
        args: std::iter::once(script)
            .chain(spec.args.iter().map(|arg| arg.to_string()))
            .collect(),
        root: String::new(),
        source: CommandSource::Settings,
        env: Vec::new(),
    };
    let generous = Duration::from_secs(60);
    LspClient::start(
        backend,
        &command,
        dv_lsp::server_config(spec, &command),
        Arc::new(|_| {}),
        Timeouts {
            initialize: generous,
            hover: generous,
            ..Timeouts::default()
        },
    )
    .unwrap_or_else(|err| panic!("starting {}: {err}", command.command_line()))
}

/// The hover's text at `at` in `path`, asked again while the server is still loading the
/// project (Tailwind's first answer comes after it scanned it), up to a minute.
fn hover_text(client: &LspClient, path: &str, at: TextPosition) -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let answer = client.hover(path, at).wait().unwrap();
        if let Some(info) = answer {
            return info
                .parts
                .iter()
                .map(|part| match part {
                    HoverPart::Markdown(text) | HoverPart::Plain(text) => text.clone(),
                    HoverPart::Code { text, .. } => text.clone(),
                })
                .collect::<Vec<_>>()
                .join("\n");
        }
        assert!(
            Instant::now() < deadline,
            "no hover at {at:?} in {path} within a minute"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
fn tailwinds_hover_shows_a_classs_css() {
    let Some((runtime, script)) = configured("DV_TEST_TAILWIND") else {
        return;
    };
    let repo = FixtureRepo::new();
    repo.write("package.json", r#"{"name": "fixture"}"#);
    repo.write("src/app.css", "@import \"tailwindcss\";\n");
    let app = "export const App = () => <div className=\"p-4\">x</div>;\n";
    repo.write("src/App.tsx", app);
    repo.commit("fixture");
    let client = start(&repo, &languages::tailwindcss::SPEC, runtime, script);
    client
        .open_document("src/App.tsx", "typescriptreact", app)
        .unwrap();
    let column = app.find("p-4").unwrap() as u32 + 1;
    let text = hover_text(&client, "src/App.tsx", TextPosition::new(0, column));
    assert!(text.contains(".p-4") && text.contains("padding"), "{text}");
    let _ = client.stop();
}

#[test]
fn the_css_servers_hover_documents_a_property() {
    let Some((runtime, script)) = configured("DV_TEST_CSS") else {
        return;
    };
    let repo = FixtureRepo::new();
    let css = "body {\n  color: red;\n}\n";
    repo.write("a.css", css);
    repo.commit("fixture");
    let client = start(&repo, &languages::css::SPEC, runtime, script);
    client.open_document("a.css", "css", css).unwrap();
    let text = hover_text(&client, "a.css", TextPosition::new(1, 3));
    assert!(text.to_lowercase().contains("color"), "{text}");
    let _ = client.stop();
}
