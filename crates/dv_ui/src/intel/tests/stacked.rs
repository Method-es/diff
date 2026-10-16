//! Several servers on one file (PLAN §18.4): a `.tsx` in a Tailwind project gets TypeScript and
//! Tailwind CSS; hover asks both and stacks their answers in the registry's order as each lands,
//! definition takes the first server that answers with anything, and a Tailwind project
//! without its server shows Tailwind CSS as missing.

use std::collections::HashMap;

use pretty_assertions::assert_eq;

use super::*;

/// A Tailwind v4 project: `src/App.tsx` changes (its line 2 renders a `p-4` div calling
/// `helper`), `src/app.css` imports Tailwind.
fn tailwind_project() -> FixtureRepo {
    let repo = FixtureRepo::new();
    repo.write("package.json", "{\"name\": \"web\"}\n");
    repo.write("src/app.css", "@import \"tailwindcss\";\n");
    repo.write(
        "src/App.tsx",
        "import { helper } from './b';\nexport const App = () => <div />;\n",
    );
    repo.write(
        "src/b.ts",
        "export function helper(x: number): number {\n  return x + 1;\n}\n",
    );
    repo.commit("base");
    repo.write(
        "src/App.tsx",
        "import { helper } from './b';\nexport const App = () => <div className=\"p-4\">{helper(1)}</div>;\n",
    );
    repo
}

/// Fake servers by spec id, each answering through its own `Answer`: `DV_LSP_<ID>` names each
/// one, and the starter starts the one its command's spec asks for.
fn use_fakes(
    answers: Vec<(&'static str, Answer)>,
    cx: &mut VisualTestContext,
) -> HashMap<&'static str, Fakes> {
    let fakes: HashMap<&'static str, Fakes> = answers
        .iter()
        .map(|(id, _)| (*id, Fakes::default()))
        .collect();
    let answers: HashMap<&'static str, Answer> = answers.into_iter().collect();
    let servers = cx.update(|_, cx| cx.services().language_servers.clone());
    let named: Vec<String> = answers
        .keys()
        .map(|id| format!("DV_LSP_{}", id.to_ascii_uppercase()))
        .collect();
    let made = fakes.clone();
    servers.update(cx, |servers, _| {
        servers.set_env(Arc::new(move |name| {
            named
                .iter()
                .any(|var| var == name)
                .then(|| format!("/fake/{name}"))
        }));
        servers.set_starter(Arc::new(move |backend, command, config, events| {
            let paths = WorkspacePaths::new(&backend.repo().toplevel);
            let answer = answers[command.spec_id].clone();
            let fakes = made[command.spec_id].clone();
            LspClient::start_with(
                move |sink| Ok(spawn_fake(sink, answer, &fakes)),
                paths,
                Some(backend),
                config,
                events,
                Timeouts::default(),
            )
        }));
    });
    fakes
}

fn hover_answer(text: &'static str, delay: Duration) -> Answer {
    Arc::new(move |message| {
        (message["method"] == "textDocument/hover").then(|| {
            std::thread::sleep(delay);
            json!({"contents": {"kind": "markdown", "value": text}})
        })
    })
}

fn sections(intel: &Entity<CodeIntel>, cx: &mut VisualTestContext) -> Vec<(&'static str, String)> {
    intel.read_with(cx, |intel, _| {
        intel
            .hover_card()
            .map(|card| {
                card.sections
                    .iter()
                    .map(|section| (section.spec_id, section.text()))
                    .collect()
            })
            .unwrap_or_default()
    })
}

#[gpui::test]
fn a_tsx_hover_stacks_typescript_then_tailwind_as_each_answers(cx: &mut TestAppContext) {
    let repo = tailwind_project();
    // TypeScript answers late, Tailwind at once: the card opens with Tailwind's and takes
    // TypeScript's above it.
    let (_dir, shell, cx, fakes) = open_with(&repo, Target::Working, cx, |cx| {
        use_fakes(
            vec![
                (
                    "typescript",
                    hover_answer(
                        "```typescript\nconst App: () => JSX.Element\n```",
                        Duration::from_millis(600),
                    ),
                ),
                (
                    "tailwindcss",
                    hover_answer(
                        "```css\n.p-4 {\n  padding: calc(var(--spacing) * 4);\n}\n```",
                        Duration::ZERO,
                    ),
                ),
            ],
            cx,
        )
    });
    wait(cx, &shell, "both servers", |_| true);
    let started: Vec<(String, String)> = servers(&shell, cx);
    assert_eq!(started.len(), 2, "{started:?}");
    assert!(
        started.iter().all(|(_, state)| state == "ready"),
        "{started:?}"
    );
    let intel = intel(&shell, cx);
    let (at, diff) = symbol(&shell, "src/App.tsx", 2, "App", cx);
    intel.update(cx, |intel, cx| {
        intel.hover(at.clone(), Pane::Stream, Default::default(), &diff, cx)
    });
    let card = intel.clone();
    wait(cx, &shell, "the first answer", move |cx| {
        card.read(cx).hover_card().is_some()
    });
    let first = sections(&intel, cx);
    let id = intel.read_with(cx, |intel, _| intel.hover_card().unwrap().id);
    if first.len() == 1 {
        assert_eq!(first[0].0, "tailwindcss", "the quicker answer shows first");
        assert!(
            !intel.read_with(cx, |intel, cx| intel.is_settled(cx)),
            "TypeScript's is still coming"
        );
    }
    let card = intel.clone();
    wait(cx, &shell, "the second answer", move |cx| {
        card.read(cx)
            .hover_card()
            .is_some_and(|card| card.sections.len() == 2)
    });
    let both = sections(&intel, cx);
    assert_eq!(
        both.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        ["typescript", "tailwindcss"],
        "the registry's order, whatever order they landed in"
    );
    assert!(both[0].1.contains("const App"));
    assert!(both[1].1.contains("padding: calc(var(--spacing) * 4)"));
    let card = intel.read_with(cx, |intel, _| intel.hover_card().cloned().unwrap());
    assert_eq!(
        card.id, id,
        "the card stays the same card as answers join it"
    );
    assert!(card.text().contains("const App") && card.text().contains(".p-4"));
    // Each server read the stream's text.
    for id in ["typescript", "tailwindcss"] {
        assert_eq!(fakes[id].with_method("textDocument/hover").len(), 1, "{id}");
        let opened = fakes[id].with_method("textDocument/didOpen");
        assert_eq!(
            opened[0]["params"]["textDocument"]["languageId"],
            "typescriptreact"
        );
    }
    // Closing takes the card and anything still coming.
    intel.update(cx, |intel, cx| intel.close_hover(cx));
    assert!(intel.read_with(cx, |intel, cx| intel.hover_card().is_none()
        && intel.is_settled(cx)));
}

#[gpui::test]
fn a_server_that_says_nothing_leaves_the_card_to_the_others(cx: &mut TestAppContext) {
    let repo = tailwind_project();
    let silent: Answer = Arc::new(|_| None);
    let (_dir, shell, cx, _fakes) = open_with(&repo, Target::Working, cx, |cx| {
        use_fakes(
            vec![
                ("typescript", silent),
                ("tailwindcss", hover_answer("`.p-4`", Duration::ZERO)),
            ],
            cx,
        )
    });
    wait(cx, &shell, "both servers", |_| true);
    let intel = intel(&shell, cx);
    let (at, diff) = symbol(&shell, "src/App.tsx", 2, "className", cx);
    intel.update(cx, |intel, cx| {
        intel.hover(at, Pane::Stream, Default::default(), &diff, cx)
    });
    let card = intel.clone();
    wait(cx, &shell, "the hover", move |cx| {
        card.read(cx).is_settled(cx)
    });
    assert_eq!(
        sections(&intel, cx),
        [("tailwindcss", "`.p-4`".to_string())]
    );
}

#[gpui::test]
fn definition_takes_the_first_server_that_answers_with_anything(cx: &mut TestAppContext) {
    let repo = tailwind_project();
    let root = WorkspacePaths::new(repo.path().to_str().unwrap()).uri_text("");
    let typescript: Answer =
        Arc::new(|message| (message["method"] == "textDocument/definition").then(|| json!([])));
    let tailwind_root = root.clone();
    let tailwind: Answer = Arc::new(move |message| {
        (message["method"] == "textDocument/definition")
            .then(|| json!([at(&tailwind_root, location("src/app.css", 0, 0))]))
    });
    let (_dir, shell, cx, fakes) = open_with(&repo, Target::Working, cx, |cx| {
        use_fakes(
            vec![("typescript", typescript), ("tailwindcss", tailwind)],
            cx,
        )
    });
    wait(cx, &shell, "both servers", |_| true);
    let intel = intel(&shell, cx);
    let (at, diff) = symbol(&shell, "src/App.tsx", 2, "helper", cx);
    intel.update(cx, |intel, cx| {
        intel.definition(at, Pane::Stream, &diff, cx)
    });
    let asked = intel.clone();
    wait(cx, &shell, "the definition", move |cx| {
        asked.read(cx).last_definitions().is_some()
    });
    // TypeScript found nothing, so Tailwind CSS was asked, and its one place counts.
    assert_eq!(
        intel.read_with(cx, |intel, _| intel.last_definitions()),
        Some(1)
    );
    assert_eq!(
        fakes["typescript"]
            .with_method("textDocument/definition")
            .len(),
        1
    );
    assert_eq!(
        fakes["tailwindcss"]
            .with_method("textDocument/definition")
            .len(),
        1
    );
}

#[gpui::test]
fn an_earlier_servers_answer_wins_however_late(cx: &mut TestAppContext) {
    let repo = tailwind_project();
    let root = WorkspacePaths::new(repo.path().to_str().unwrap()).uri_text("");
    let ts_root = root.clone();
    let typescript: Answer = Arc::new(move |message| {
        (message["method"] == "textDocument/definition").then(|| {
            std::thread::sleep(Duration::from_millis(400));
            json!([at(&ts_root, location("src/b.ts", 0, 16))])
        })
    });
    let tailwind: Answer = Arc::new(move |message| {
        (message["method"] == "textDocument/definition").then(|| {
            json!([
                at(&root, location("src/app.css", 0, 0)),
                at(&root, location("src/app.css", 0, 9))
            ])
        })
    });
    let (_dir, shell, cx, fakes) = open_with(&repo, Target::Working, cx, |cx| {
        use_fakes(
            vec![("typescript", typescript), ("tailwindcss", tailwind)],
            cx,
        )
    });
    wait(cx, &shell, "both servers", |_| true);
    let intel = intel(&shell, cx);
    let (at, diff) = symbol(&shell, "src/App.tsx", 2, "helper", cx);
    intel.update(cx, |intel, cx| {
        intel.definition(at, Pane::Stream, &diff, cx)
    });
    let asked = intel.clone();
    wait(cx, &shell, "the definition", move |cx| {
        asked.read(cx).last_definitions().is_some()
    });
    // Both were asked at once; Tailwind CSS answered first with two places, but TypeScript
    // comes first in the registry and its one place is the answer.
    assert_eq!(
        intel.read_with(cx, |intel, _| intel.last_definitions()),
        Some(1)
    );
    for id in ["typescript", "tailwindcss"] {
        assert_eq!(
            fakes[id].with_method("textDocument/definition").len(),
            1,
            "{id}"
        );
    }
}

#[gpui::test]
fn a_server_without_the_capability_isnt_asked(cx: &mut TestAppContext) {
    let repo = tailwind_project();
    let root = WorkspacePaths::new(repo.path().to_str().unwrap()).uri_text("");
    let typescript: Answer = Arc::new(move |message| {
        (message["method"] == "textDocument/definition")
            .then(|| json!([at(&root, location("src/b.ts", 0, 16))]))
    });
    let tailwind: Answer = Arc::new(|_| None);
    let (_dir, shell, cx, fakes) = open_with(&repo, Target::Working, cx, |cx| {
        let fakes = use_fakes(
            vec![("typescript", typescript), ("tailwindcss", tailwind)],
            cx,
        );
        // Tailwind CSS's server does hover only.
        *fakes["tailwindcss"].capabilities.lock() =
            Some(json!({"positionEncoding": "utf-8", "hoverProvider": true}));
        fakes
    });
    wait(cx, &shell, "both servers", |_| true);
    let intel = intel(&shell, cx);
    let (at, diff) = symbol(&shell, "src/App.tsx", 2, "helper", cx);
    intel.update(cx, |intel, cx| {
        intel.definition(at, Pane::Stream, &diff, cx)
    });
    let asked = intel.clone();
    wait(cx, &shell, "the definition", move |cx| {
        asked.read(cx).last_definitions().is_some()
    });
    assert_eq!(
        intel.read_with(cx, |intel, _| intel.last_definitions()),
        Some(1)
    );
    assert!(
        fakes["tailwindcss"]
            .with_method("textDocument/definition")
            .is_empty()
    );
}

/// One request sent to two servers (a stack's hovers, a definition asked of both): the one
/// that answered is left alone, the one still answering is cancelled when the UI lets go.
#[test]
fn an_abandoned_request_cancels_the_server_still_answering() {
    let start = |answer: Answer, fakes: &Fakes| {
        let made = fakes.clone();
        let client = LspClient::start_with(
            move |sink| Ok(spawn_fake(sink, answer, &made)),
            WorkspacePaths::new("/w/repo"),
            None,
            dv_lsp::ServerConfig::default(),
            Arc::new(|_| {}),
            Timeouts::default(),
        )
        .expect("the fake starts");
        client
            .open_document("a.tsx", "typescriptreact", "App")
            .unwrap();
        client
    };
    let (slow, quick) = (Fakes::default(), Fakes::default());
    let typescript = start(hover_answer("const App", Duration::from_secs(3)), &slow);
    let tailwind = start(hover_answer("`.p-4`", Duration::ZERO), &quick);
    let (in_flight, arming) = InFlight::detached();
    let late = typescript.hover("a.tsx", dv_lsp::TextPosition::new(0, 0));
    let answered = tailwind.hover("a.tsx", dv_lsp::TextPosition::new(0, 0));
    assert!(arming.arm(&late) && arming.arm(&answered));
    assert!(answered.wait().unwrap().is_some());
    let waiting = std::thread::spawn(move || late.wait());
    drop(in_flight);
    assert_eq!(waiting.join().unwrap().unwrap_err(), RequestError::Exited);
    let deadline = Instant::now() + Duration::from_secs(10);
    while slow.with_method("$/cancelRequest").is_empty() {
        assert!(
            Instant::now() < deadline,
            "TypeScript's hover was never cancelled"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        slow.with_method("$/cancelRequest")[0]["params"]["id"],
        slow.with_method("textDocument/hover")[0]["id"]
    );
    assert!(quick.with_method("$/cancelRequest").is_empty());
}

#[gpui::test]
fn a_tailwind_project_without_its_server_shows_it_missing(cx: &mut TestAppContext) {
    let repo = tailwind_project();
    let typescript: Answer = Arc::new(|_| None);
    let (_dir, shell, cx, _fakes) = open_with(&repo, Target::Working, cx, |cx| {
        use_fakes(vec![("typescript", typescript)], cx)
    });
    wait(cx, &shell, "the servers", |_| true);
    let intel = intel(&shell, cx);
    let missing = intel.read_with(cx, |intel, cx| intel.missing(cx));
    let tailwind = missing
        .iter()
        .find(|missing| missing.spec_id == "tailwindcss")
        .unwrap_or_else(|| panic!("Tailwind CSS isn't missing: {missing:?}"));
    assert_eq!(
        tailwind.reason,
        "DV_LSP_TAILWINDCSS isn't set, and automated runs find language servers only through it"
    );
    assert!(
        tailwind
            .hint
            .contains("bun add -g @tailwindcss/language-server")
    );
    assert!(
        tailwind
            .hint
            .contains("npm install -g @tailwindcss/language-server")
    );
    // The stylesheet isn't one of the review's files, so the CSS server isn't asked for;
    // TypeScript runs.
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert_eq!(servers(&shell, cx), [("typescript".into(), "ready".into())]);
}

#[gpui::test]
fn a_project_without_tailwind_never_asks_for_it(cx: &mut TestAppContext) {
    let repo = project();
    let typescript: Answer = Arc::new(|_| None);
    let (_dir, shell, cx, _fakes) = open_with(&repo, Target::Working, cx, |cx| {
        use_fakes(vec![("typescript", typescript)], cx)
    });
    wait(cx, &shell, "the servers", |_| true);
    let intel = intel(&shell, cx);
    assert_eq!(intel.read_with(cx, |intel, cx| intel.missing(cx)), []);
    assert_eq!(servers(&shell, cx), [("typescript".into(), "ready".into())]);
}

/// A hover is about the pointer's own character (PLAN §18.1 "Hover"): any piece of a class
/// name, a number in it, its punctuation; never whitespace or a quote.
#[test]
fn a_hover_asks_at_the_pointers_character() {
    let line = r#"  <div className="bg-red-500 md:hover:text-sm [--x:1px]">"#;
    let token = |piece: &str, nth: usize| {
        let at = line.find(piece).unwrap() + nth;
        hover_token_at(line, at).map(|range| &line[range])
    };
    assert_eq!(token("bg-red", 0), Some("bg"));
    assert_eq!(token("500", 0), Some("500"), "a number asks too");
    assert_eq!(token("-red", 0), Some("bg-red-500"), "punctuation: the run");
    assert_eq!(token(":hover", 0), Some("md:hover:text-sm"));
    assert_eq!(token("[--x", 0), Some("[--x:1px]"));
    assert_eq!(token("\"bg", 0), None, "a quote");
    assert_eq!(token(" md", 0), None, "whitespace");
    assert_eq!(hover_token_at(line, line.len()), None);
    // F12's identifier keeps its number rule.
    assert_eq!(identifier_at(line, line.find("500").unwrap()), None);
}

/// The card is about the range the servers answered for: Tailwind's whole class, so the
/// pointer anywhere in `bg-red-500` keeps it.
#[test]
fn the_card_spans_the_servers_range() {
    let line = r#"  <div className="bg-red-500">"#;
    let start = line.find("bg-red-500").unwrap();
    let at = SymbolAt {
        path: "a.tsx".into(),
        side: AnchorSide::New,
        line: 3,
        range: start + 7..start + 10,
        name: "500".into(),
        offset: start + 7,
    };
    let range = |line: u32, from: usize, to: usize| dv_lsp::TextRange {
        start: dv_lsp::TextPosition::new(line, from as u32),
        end: dv_lsp::TextPosition::new(line, to as u32),
    };
    let section = |order, range| HoverSection {
        order,
        spec_id: "tailwindcss",
        server: "Tailwind CSS",
        info: dv_lsp::HoverInfo {
            parts: vec![dv_lsp::HoverPart::Plain("x".into())],
            range,
        },
    };
    let mut card = HoverCard {
        at: at.clone(),
        pane: Pane::Stream,
        token: Default::default(),
        sections: Vec::new(),
        span: at.range.clone(),
        against_worktree: false,
        id: 1,
    };
    card.add(section(1, Some(range(2, start, start + 10))));
    assert_eq!(card.span, start..start + 10);
    for (offset, covered) in [
        (start, true),
        (start + 3, true),
        (start + 9, true),
        (start + 10, false),
    ] {
        assert_eq!(
            card.covers(&SymbolAt {
                offset,
                ..at.clone()
            }),
            covered,
            "{offset}"
        );
    }
    assert!(!card.covers(&SymbolAt {
        line: 4,
        ..at.clone()
    }));
    // A range on another line, or one that doesn't hold the asked byte, is ignored.
    card.add(section(0, Some(range(5, 0, 3))));
    card.add(section(2, Some(range(2, 0, 2))));
    assert_eq!(card.span, start..start + 10);
}

/// A server installed while the review is open (nothing in the worktree changes): **Check
/// again** finds it; the window coming back looks again only while one is missing, and at most
/// every `RECHECK_EVERY`.
#[gpui::test]
fn check_again_finds_a_server_installed_since(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let repo = tailwind_project();
    let installed = Arc::new(AtomicBool::new(false));
    let flag = installed.clone();
    let (_dir, shell, cx, _fakes) = open_with(&repo, Target::Working, cx, move |cx| {
        let fakes = use_fakes(
            vec![
                ("typescript", Arc::new(|_: &Value| None) as Answer),
                ("tailwindcss", hover_answer("`.p-4`", Duration::ZERO)),
            ],
            cx,
        );
        let servers = cx.update(|_, cx| cx.services().language_servers.clone());
        servers.update(cx, |servers, _| {
            servers.set_env(Arc::new(move |name| match name {
                "DV_LSP_TYPESCRIPT" => Some("/fake/typescript".to_string()),
                "DV_LSP_TAILWINDCSS" if flag.load(Ordering::SeqCst) => {
                    Some("/fake/tailwindcss".to_string())
                }
                _ => None,
            }));
        });
        fakes
    });
    wait(cx, &shell, "the servers", |_| true);
    let intel = intel(&shell, cx);
    let missing = |cx: &mut VisualTestContext| {
        intel.read_with(cx, |intel, cx| {
            intel
                .missing(cx)
                .into_iter()
                .map(|missing| missing.spec_id)
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(missing(cx), ["tailwindcss"]);
    // Coming back to the window looks again (one is missing)...
    intel.update(cx, |intel, cx| intel.window_activated(cx));
    wait(cx, &shell, "the recheck", |_| true);
    assert_eq!(intel.read_with(cx, |intel, cx| intel.rechecks(cx)), 1);
    // ...but not again so soon.
    intel.update(cx, |intel, cx| intel.window_activated(cx));
    wait(cx, &shell, "nothing", |_| true);
    assert_eq!(intel.read_with(cx, |intel, cx| intel.rechecks(cx)), 1);
    // Installed now: Check again finds it, whenever it is asked.
    installed.store(true, Ordering::SeqCst);
    intel.update(cx, |intel, cx| intel.check_again(cx));
    wait(cx, &shell, "Tailwind CSS", |_| true);
    assert_eq!(intel.read_with(cx, |intel, cx| intel.rechecks(cx)), 2);
    assert_eq!(missing(cx), [] as [&str; 0]);
    let mut started = servers(&shell, cx);
    started.sort();
    assert_eq!(
        started,
        [
            ("tailwindcss".to_string(), "ready".to_string()),
            ("typescript".into(), "ready".into())
        ]
    );
}
