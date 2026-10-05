//! The sanitiser's guarantee (no construct that makes `TextView` fetch a URL or render markup
//! survives; code stays verbatim) and the parser's tolerance of hostile input, headless: the
//! same `markdown::to_mdast` call `TextView` makes before rendering anything.

use markdown::mdast::Node;
use markdown::{ParseOptions, to_mdast};

use super::{coarse_escape, sanitize_untrusted};

fn parse(source: &str) -> Result<Node, markdown::message::Message> {
    to_mdast(source, &ParseOptions::gfm())
}

/// After sanitising, a parse of the result holds no `Image`, `ImageReference` or `Html` node
/// anywhere: exactly the nodes `TextView` turns into fetches or markup.
fn assert_inert(sanitized: &str) {
    fn walk(node: &Node) {
        match node {
            Node::Image(_) | Node::ImageReference(_) | Node::Html(_) => {
                panic!("sanitised output still has a fetchable node: {node:?}")
            }
            other => {
                if let Some(children) = other.children() {
                    children.iter().for_each(walk);
                }
            }
        }
    }
    walk(&parse(sanitized).expect("sanitised output parses"));
}

#[test]
fn plain_markdown_parses_into_its_nodes() {
    let Node::Root(root) = parse("## Section\n\nsee [dv](https://example.com/dv)").unwrap() else {
        panic!("a root node");
    };
    let Node::Heading(heading) = &root.children[0] else {
        panic!("a heading, got {:?}", root.children[0]);
    };
    assert_eq!(heading.depth, 2);
    let Node::Paragraph(paragraph) = &root.children[1] else {
        panic!("a paragraph");
    };
    assert!(
        paragraph
            .children
            .iter()
            .any(|child| matches!(child, Node::Link(_)))
    );
    let Node::Root(root) = parse("```rust\nfn main() {}\n```").unwrap() else {
        panic!("a root node");
    };
    assert!(matches!(root.children[0], Node::Code(_)));
    parse("> quoted\n\n- one\n- two").expect("blockquote and list parse");
}

#[test]
fn hostile_input_never_panics() {
    for source in [
        "```rust\nfn main() {\n// never closed".to_string(),
        "*".repeat(500) + "text" + &"*".repeat(500),
        "a".repeat(200_000),
        "]()[ [[[]]] ()()( ![]( [text](".to_string(),
        "hello \0 world \u{7} \u{1b}[31m fake ansi".to_string(),
        "héllo 🎉 \u{200b} zero-width \u{feff} bom-in-middle".to_string(),
        String::new(),
        "   \n\n\t  ".to_string(),
    ] {
        let _ = parse(&source);
        assert_inert(&sanitize_untrusted(&source));
    }
}

#[test]
fn plain_markdown_passes_through_unchanged() {
    let source = "## Title\n\nsome *emphasis*, a [link](https://example.com), and\n\n- a list\n";
    assert_eq!(sanitize_untrusted(source), source);
}

#[test]
fn an_inline_image_becomes_an_inert_placeholder() {
    let out = sanitize_untrusted("before ![tracking pixel](https://evil.example/p.png) after");
    assert!(!out.contains("evil.example"), "the URL is gone: {out}");
    assert!(out.contains("[image: tracking pixel]"), "{out}");
    assert!(
        out.starts_with("before ") && out.ends_with(" after"),
        "{out}"
    );
    assert_inert(&out);

    let out = sanitize_untrusted("x ![](https://evil.example/p.png) y");
    assert!(!out.contains("evil.example"));
    assert!(out.contains("[image]"), "{out}");
    assert_inert(&out);
}

#[test]
fn reference_images_of_every_form_are_neutralised() {
    let out = sanitize_untrusted("see ![alt text][pix]\n\n[pix]: https://evil.example/p.png\n");
    assert!(out.contains("[image: alt text]"), "{out}");
    assert_inert(&out);
    assert_inert(&sanitize_untrusted(
        "![pix][]\n\n[pix]: https://evil.example/a.png\n",
    ));
    assert_inert(&sanitize_untrusted(
        "![pix]\n\n[pix]: https://evil.example/a.png\n",
    ));
}

#[test]
fn inline_html_is_escaped_in_any_casing() {
    let out = sanitize_untrusted("a <img src=\"https://evil.example/p.png\"> b");
    assert!(out.contains("&lt;img"), "{out}");
    assert_inert(&out);
    let upper = sanitize_untrusted("a <IMG SRC=\"https://evil.example/p.png\"> b");
    assert!(upper.contains("&lt;IMG"), "{upper}");
    assert_inert(&upper);
}

#[test]
fn an_html_block_is_escaped_with_the_image_inside_it() {
    let source = "<div>\n<img src=\"https://evil.example/p.png\">\n![x](https://evil.example/q.png)\n</div>\n";
    assert_inert(&sanitize_untrusted(source));
}

#[test]
fn images_nested_in_lists_and_links_are_neutralised() {
    let out =
        sanitize_untrusted("- first\n- has ![pix](https://evil.example/p.png) inline\n- last");
    assert!(!out.contains("evil.example"));
    assert!(out.contains("- has [image: pix] inline"), "{out}");
    assert_inert(&out);

    let out = sanitize_untrusted("[![badge](https://evil.example/b.svg)](https://ok.example)");
    assert!(!out.contains("evil.example"));
    assert_inert(&out);

    let out = sanitize_untrusted(
        "![a](https://e.example/1.png)\n\ntext\n\n![b](https://e.example/2.png)",
    );
    assert!(!out.contains("e.example"));
    assert_inert(&out);
}

#[test]
fn code_stays_verbatim() {
    for source in [
        "intro\n\n```html\n![alt](https://example.com/x.png)\n<img src=\"y.png\">\n```\n\noutro",
        "use `![alt](url)` and `<img src=x>` literally",
        "para\n\n    ![alt](https://example.com/x.png)\n",
    ] {
        assert_eq!(sanitize_untrusted(source), source);
    }
}

#[test]
fn multibyte_text_before_an_image_keeps_the_edits_aligned() {
    let out = sanitize_untrusted("héllo 🎉 ![pïx](https://evil.example/p.png) done");
    assert!(!out.contains("evil.example"), "{out}");
    assert!(out.contains("héllo 🎉 "), "{out}");
    assert!(out.ends_with(" done"), "{out}");
    assert_inert(&out);
}

#[test]
fn alt_text_cannot_reopen_syntax_through_the_placeholder() {
    let out = sanitize_untrusted("![a<b>`c`[d]](https://evil.example/p.png)");
    assert!(!out.contains("evil.example"));
    assert_inert(&out);
}

#[test]
fn the_coarse_fallback_is_fetch_proof_too() {
    assert_inert(&coarse_escape(
        "<img src=\"https://e.example/x\"> ![a](https://e.example/y)",
    ));
}

#[test]
fn a_one_line_summary_reads_as_the_rendered_text() {
    for (body, line) in [
        (
            "add a sweep or **cap** the size.\nmore",
            "add a sweep or cap the size.",
        ),
        (
            "use `limit` _here_ ~~not there~~",
            "use limit here not there",
        ),
        (
            "see [the docs](https://example.com/d) first",
            "see the docs first",
        ),
        ("```ts\nconst x = 1;\n```\nafter", "const x = 1;"),
        ("## Heading *one*\n\nbody", "Heading one"),
        ("- [ ] first item\n- second", "first item"),
        ("> quoted **bit**", "quoted bit"),
        ("---\n\nafter the rule", "after the rule"),
        (
            "![shot](https://evil.example/p.png) look",
            "[image: shot] look",
        ),
        ("line one  \nline two", "line one"),
        ("| a | b |\n|---|---|\n| 1 | 2 |", "a | b"),
        ("  \n\n", ""),
        ("plain words", "plain words"),
    ] {
        assert_eq!(super::first_line_plain(body), line, "{body:?}");
    }
}
