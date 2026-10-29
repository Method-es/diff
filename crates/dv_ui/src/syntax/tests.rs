use dv_core::diff::{DiffLine, LineKind, TextDiff};
use pretty_assertions::assert_eq;

use super::*;
use crate::theme::{Appearance, DvColors, Material, Palette};

#[test]
fn capture_names_map_to_kinds() {
    use TokenKind::*;
    let table = [
        ("keyword", Some(Keyword)),
        ("keyword.return", Some(Keyword)),
        ("keyword.function", Some(Keyword)),
        ("conditional", Some(Keyword)),
        ("repeat", Some(Keyword)),
        ("include", Some(Keyword)),
        ("storageclass", Some(Keyword)),
        ("preproc", Some(Keyword)),
        ("variable.builtin", Some(Keyword)),
        ("variable.special", Some(Keyword)),
        ("tag", Some(Keyword)),
        ("tag.doctype", Some(Keyword)),
        ("title", Some(Keyword)),
        ("function", Some(Function)),
        ("function.method", Some(Function)),
        ("function.builtin", Some(Function)),
        ("function.macro", Some(Function)),
        ("method", Some(Function)),
        ("method.call", Some(Function)),
        ("link_text", Some(Function)),
        ("string", Some(String)),
        ("string.escape", Some(String)),
        ("string.regex", Some(String)),
        ("string.special", Some(String)),
        ("character", Some(String)),
        ("character.special", Some(String)),
        ("escape", Some(String)),
        ("text.literal", Some(String)),
        ("text.code.span", Some(String)),
        ("link_uri", Some(String)),
        ("number", Some(Number)),
        ("number.float", Some(Number)),
        ("float", Some(Number)),
        ("type", Some(Type)),
        ("type.builtin", Some(Type)),
        ("type.qualifier", Some(Type)),
        ("constructor", Some(Type)),
        ("constant", Some(Constant)),
        ("constant.builtin", Some(Constant)),
        ("constant.macro", Some(Constant)),
        ("boolean", Some(Constant)),
        ("label", Some(Constant)),
        ("string.special.symbol", Some(Constant)),
        ("property", Some(Property)),
        ("property.definition", Some(Property)),
        ("field", Some(Property)),
        ("attribute", Some(Property)),
        ("tag.attribute", Some(Property)),
        ("variable.member", Some(Property)),
        ("punctuation", Some(Punctuation)),
        ("punctuation.bracket", Some(Punctuation)),
        ("punctuation.delimiter", Some(Punctuation)),
        ("punctuation.special", Some(Punctuation)),
        ("operator", Some(Punctuation)),
        ("delimiter", Some(Punctuation)),
        ("tag.delimiter", Some(Punctuation)),
        ("comment", Some(Comment)),
        ("comment.doc", Some(Comment)),
        ("comment.documentation", Some(Comment)),
        ("variable", Some(Plain)),
        ("variable.parameter", Some(Plain)),
        ("parameter", Some(Plain)),
        ("embedded", Some(Plain)),
        ("module", Some(Plain)),
        ("namespace", Some(Plain)),
        ("emphasis.strong", Some(Plain)),
        ("text.reference", Some(Plain)),
        ("none", Some(Plain)),
        ("spell", Some(Plain)),
        // Private captures and names that say nothing about colour.
        ("_function", None),
        ("__name__", None),
        ("clean", None),
        ("error", None),
        ("", None),
    ];
    for (name, kind) in table {
        assert_eq!(kind_for_capture(name), kind, "{name:?}");
    }
}

#[test]
fn kinds_resolve_to_the_themes_syntax_colours() {
    let colors = DvColors::new(Palette::Aura, Appearance::Dark, Material::Opaque);
    let syntax = &colors.syntax;
    assert_eq!(TokenKind::Keyword.color(syntax), syntax.keyword);
    assert_eq!(TokenKind::Comment.color(syntax), colors.fg3);
    assert_eq!(TokenKind::Plain.color(syntax), syntax.plain);
    let light = DvColors::new(Palette::Aura, Appearance::Light, Material::Mica);
    assert_eq!(TokenKind::Comment.color(&light.syntax), light.fg3);
}

#[test]
fn every_kind_survives_the_style_round_trip() {
    for kind in TokenKind::ALL {
        assert_eq!(decode(&encode(kind)), kind, "{kind:?}");
    }
    assert_eq!(decode(&HighlightStyle::default()), TokenKind::Plain);
    // A colour that isn't an encoded kind (another resolver's) is plain.
    let foreign = HighlightStyle {
        color: Some(Hsla {
            h: 0.3,
            s: 0.5,
            l: 0.5,
            a: 1.,
        }),
        ..Default::default()
    };
    assert_eq!(decode(&foreign), TokenKind::Plain);
}

#[test]
fn languages_come_from_names_and_extensions() {
    let table = [
        ("src/main.rs", Some("rust")),
        ("SRC/MAIN.RS", Some("rust")),
        ("src/routes/users.ts", Some("typescript")),
        ("web/App.tsx", Some("tsx")),
        ("web/index.jsx", Some("javascript")),
        ("lib/x.mjs", Some("javascript")),
        ("package.json", Some("json")),
        ("tool.py", Some("python")),
        ("cmd/main.go", Some("go")),
        ("include/x.h", Some("c")),
        ("src/x.hpp", Some("cpp")),
        ("App.cs", Some("csharp")),
        ("style.scss", Some("css")),
        ("index.html", Some("html")),
        ("README.md", Some("markdown")),
        ("Cargo.toml", Some("toml")),
        ("Cargo.lock", Some("toml")),
        (".github/ci.yml", Some("yaml")),
        ("script/check.sh", Some("bash")),
        ("home/.bashrc", Some("bash")),
        ("Makefile", Some("make")),
        ("sub/GNUmakefile", Some("make")),
        ("CMakeLists.txt", Some("cmake")),
        ("Gemfile", Some("ruby")),
        ("lib/x.rb", Some("ruby")),
        ("fix.patch", Some("diff")),
        ("notes.txt", None),
        ("LICENSE", None),
        ("migrations/0042_add_email.sql", Some("sql")),
        ("db/schema.PGSQL", Some("sql")),
        ("archive.tar.gz", None),
        (".gitignore", None),
    ];
    for (path, language) in table {
        assert_eq!(language_for_path(path), language, "{path}");
    }
}

fn kinds(side: &SideHighlights, line: u32, text: &str) -> Vec<(String, TokenKind)> {
    side.line(line)
        .iter()
        .map(|token| {
            let range = token.range.start as usize..token.range.end as usize;
            (text[range].to_string(), token.kind)
        })
        .collect()
}

#[test]
fn rust_highlights_into_line_relative_kinds() {
    let lines = [
        "/// Says hello.",
        "fn main() {",
        "    let greeting = \"hi\";",
        "    let n = 42;",
        "}",
    ];
    let side = highlight_side("src/main.rs", &lines).expect("rust has a grammar");
    assert_eq!(side.len(), 5);
    assert!(
        kinds(&side, 1, lines[0])
            .iter()
            .all(|(_, kind)| *kind == TokenKind::Comment),
        "{:?}",
        kinds(&side, 1, lines[0])
    );
    let main = kinds(&side, 2, lines[1]);
    assert!(
        main.contains(&("fn".into(), TokenKind::Keyword)),
        "{main:?}"
    );
    assert!(
        main.contains(&("main".into(), TokenKind::Function)),
        "{main:?}"
    );
    let greeting = kinds(&side, 3, lines[2]);
    assert!(
        greeting.contains(&("let".into(), TokenKind::Keyword)),
        "{greeting:?}"
    );
    assert!(
        greeting
            .iter()
            .any(|(text, kind)| text.contains("hi") && *kind == TokenKind::String),
        "{greeting:?}"
    );
    // Rust's query captures literals as `constant.builtin`.
    let n = kinds(&side, 4, lines[3]);
    assert!(n.contains(&("42".into(), TokenKind::Constant)), "{n:?}");
    // Past the end and line 0 are empty, never a panic.
    assert!(side.line(0).is_empty());
    assert!(side.line(99).is_empty());
}

#[test]
fn sql_highlights_a_migration() {
    let lines = [
        "-- Adds the email column.",
        "ALTER TABLE users ADD COLUMN email varchar(255) NOT NULL DEFAULT '';",
        "select count(*) from users where id = 42;",
    ];
    let side = highlight_side("migrations/0042_add_email.sql", &lines).expect("sql has a grammar");
    assert!(
        kinds(&side, 1, lines[0])
            .iter()
            .all(|(_, kind)| *kind == TokenKind::Comment),
        "{:?}",
        kinds(&side, 1, lines[0])
    );
    let alter = kinds(&side, 2, lines[1]);
    for keyword in ["ALTER", "TABLE", "ADD", "COLUMN", "NOT", "DEFAULT"] {
        assert!(
            alter.contains(&(keyword.into(), TokenKind::Keyword)),
            "{keyword}: {alter:?}"
        );
    }
    assert!(
        alter.contains(&("varchar".into(), TokenKind::Type)),
        "{alter:?}"
    );
    assert!(
        alter.contains(&("255".into(), TokenKind::Number)),
        "{alter:?}"
    );
    assert!(
        alter.contains(&("NULL".into(), TokenKind::Constant)),
        "{alter:?}"
    );
    assert!(
        alter.contains(&("''".into(), TokenKind::String)),
        "{alter:?}"
    );
    // Lower-case keywords read the same, and a call is a function.
    let select = kinds(&side, 3, lines[2]);
    for keyword in ["select", "from", "where"] {
        assert!(
            select.contains(&(keyword.into(), TokenKind::Keyword)),
            "{keyword}: {select:?}"
        );
    }
    assert!(
        select.contains(&("count".into(), TokenKind::Function)),
        "{select:?}"
    );
    assert!(
        select.contains(&("42".into(), TokenKind::Number)),
        "{select:?}"
    );
    let decimal = ["select 1.5;"];
    let side = highlight_side("q.sql", &decimal).expect("sql has a grammar");
    let decimal = kinds(&side, 1, decimal[0]);
    assert!(
        decimal.contains(&("1.5".into(), TokenKind::Number)),
        "{decimal:?}"
    );
}

#[test]
fn tokens_are_sorted_disjoint_and_inside_their_line() {
    let lines = [
        "/* a comment",
        "   spanning lines */ const x: number = 1;",
        "export function f(a: string) { return `t ${a}`; }",
    ];
    let side = highlight_side("x.ts", &lines).expect("typescript has a grammar");
    for (index, line) in lines.iter().enumerate() {
        let mut end = 0;
        for token in side.line(index as u32 + 1) {
            assert!(token.range.start >= end, "line {index}: {token:?}");
            assert!(token.range.end as usize <= line.len(), "line {index}");
            assert!(token.range.start < token.range.end);
            assert_ne!(token.kind, TokenKind::Plain);
            end = token.range.end;
        }
    }
    assert!(
        kinds(&side, 2, lines[1]).contains(&("1".to_string(), TokenKind::Number)),
        "{:?}",
        kinds(&side, 2, lines[1])
    );
    // The block comment's second line starts with its tail.
    let second = kinds(&side, 2, lines[1]);
    assert_eq!(
        second.first(),
        Some(&("   spanning lines */".to_string(), TokenKind::Comment)),
        "{second:?}"
    );
}

#[test]
fn caps_and_unknown_languages_give_nothing() {
    assert_eq!(highlight_side("notes.txt", &["fn main() {}"]), None);
    assert_eq!(highlight_side::<&str>("src/main.rs", &[]), None);
    let big = "x".repeat(1024);
    let many: Vec<&str> = std::iter::repeat_n(big.as_str(), 2049).collect();
    assert_eq!(highlight_side("src/main.rs", &many), None);

    let long = format!("fn f() {{ let s = \"{}\"; }}", "y".repeat(MAX_LINE_BYTES));
    let lines = ["fn a() {}", long.as_str(), "fn b() {}"];
    let side = highlight_side("src/main.rs", &lines).expect("under the side cap");
    assert!(
        side.line(2).is_empty(),
        "a line over the cap keeps no tokens"
    );
    assert!(!side.line(1).is_empty());
    assert!(!side.line(3).is_empty());
}

fn line(kind: LineKind, old_no: Option<u32>, new_no: Option<u32>, text: &str) -> DiffLine {
    DiffLine {
        kind,
        old_no,
        new_no,
        text: text.to_string(),
        words: Vec::new(),
    }
}

#[test]
fn a_diff_highlights_each_side_under_its_own_name() {
    let diff = TextDiff {
        lines: vec![
            line(LineKind::Context, Some(1), Some(1), "fn keep() {}"),
            line(LineKind::Removed, Some(2), None, "fn gone() {}"),
            line(LineKind::Added, None, Some(2), "fn added() {}"),
        ],
        ..TextDiff::empty()
    };
    // A rename from a name dv has no grammar for: only the new side highlights.
    let highlights = highlight_diff("src/lib.rs", Some("src/lib.txt"), &diff);
    assert_eq!(highlights.old, None);
    let new = highlights.side(AnchorSide::New).expect("the new side");
    assert_eq!(new.len(), 2);
    assert!(kinds(new, 2, "fn added() {}").contains(&("added".into(), TokenKind::Function)));

    let both = highlight_diff("src/lib.rs", None, &diff);
    let old = both.side(AnchorSide::Old).expect("the old side");
    assert!(kinds(old, 2, "fn gone() {}").contains(&("gone".into(), TokenKind::Function)));
}

const TSX: [&str; 6] = [
    "import { useState } from \"react\";",
    "export function Card({ title }: { title: string }) {",
    "  const [n, setN] = useState<number>(0);",
    "  return <div className=\"card\" onClick={() => setN(n + 1)}>",
    "    <Title text={title} />",
    "  </div>;",
];

#[test]
fn tsx_highlights_its_typescript_and_its_jsx() {
    use TokenKind::*;
    let side = highlight_side("web/Card.tsx", &TSX).expect("tsx has a grammar");
    let has = |line: u32, text: &str, kind: TokenKind| {
        let tokens = kinds(&side, line, TSX[line as usize - 1]);
        assert!(
            tokens.contains(&(text.to_string(), kind)),
            "line {line}: {text:?} as {kind:?} in {tokens:?}"
        );
    };
    has(1, "import", Keyword);
    has(1, "\"react\"", String);
    has(2, "function", Keyword);
    has(2, "Card", Function);
    has(2, "string", Type);
    has(3, "const", Keyword);
    has(3, "0", Number);
    has(4, "return", Keyword);
    has(4, "div", Keyword);
    has(4, "className", Property);
    has(4, "\"card\"", String);
    has(5, "Title", Type);
    has(5, "text", Property);
    has(6, "div", Keyword);
}

#[test]
fn typescript_reads_the_same_in_a_tsx_file() {
    let lines = [
        "/** Doubles. */",
        "export async function twice(n: number): Promise<number> {",
        "  const out = { value: n * 2, label: `x${n}` } satisfies Record<string, unknown>;",
        "  return out.value as number;",
        "}",
    ];
    let ts = highlight_side("a.ts", &lines).expect("typescript has a grammar");
    let tsx = highlight_side("a.tsx", &lines).expect("tsx has a grammar");
    assert_eq!(tsx, ts);
}

#[test]
fn jsx_in_a_javascript_file_has_its_tags() {
    use TokenKind::*;
    let lines = ["const el = <Card title=\"a\"><span>x</span></Card>;"];
    let side = highlight_side("web/el.jsx", &lines).expect("javascript has a grammar");
    let tokens = kinds(&side, 1, lines[0]);
    for (text, kind) in [
        ("const", Keyword),
        ("Card", Type),
        ("title", Property),
        ("\"a\"", String),
        ("span", Keyword),
    ] {
        assert!(
            tokens.contains(&(text.to_string(), kind)),
            "{text:?} as {kind:?} in {tokens:?}"
        );
    }
}

#[test]
fn lines_highlighted_on_their_own_keep_to_themselves() {
    let mut highlighter = LineHighlighter::new();
    // A line opening a block comment or a template literal colours nothing after it.
    let whole = highlighter.highlight("a.ts", "/* a comment */");
    assert_eq!(
        whole.first().map(|token| token.kind),
        Some(TokenKind::Comment)
    );
    highlighter.highlight("a.ts", "/* opens a comment");
    let next = highlighter.highlight("a.ts", "const y = `x");
    assert_eq!(
        next.first().map(|token| (token.range.clone(), token.kind)),
        Some((0..5, TokenKind::Keyword))
    );
    let after = highlighter.highlight("b.ts", "export const z = 1;");
    assert!(
        after.iter().any(|token| token.kind == TokenKind::Keyword),
        "{after:?}"
    );
    assert!(
        after.iter().all(|token| token.kind != TokenKind::String),
        "{after:?}"
    );
    // The same line twice, and no grammar.
    assert_eq!(highlighter.highlight("b.ts", "export const z = 1;"), after);
    assert!(highlighter.highlight("notes.unknown", "const x").is_empty());
}
