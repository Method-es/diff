//! Markdown for comment, reply and summary bodies (DESIGN §4.1 thread card), rendered by
//! gpui-component's `TextView` over a sanitised source (PLAN §15).
//!
//! Every body is untrusted: an agent wrote it, or the reviewer pasted it from anywhere, and a
//! GitHub thread's body comes from any PR participant. `TextView` fetches the URL of an image
//! (`![](url)`, raw `<img>`), which would hand whoever wrote the body a tracking pixel (the
//! reviewer's address and the time they looked), and renders raw HTML. [`sanitize_untrusted`]
//! removes both before the source reaches it; [`view`] is the one way a view renders a body, so
//! nothing skips it. Links open through [`crate::system::open_url`], which opens web addresses
//! only and only records them under automation.

use gpui::{App, ElementId, SharedString};
use gpui_component::text::{TextView, TextViewStyle};

use crate::theme::ActiveTheme as _;

/// A Markdown body as a [`TextView`] keyed by `id`, which must be stable and unique per body
/// (a thread or reply id), so the view's parsed state follows the body across renders. The
/// current light/dark appearance is part of the key, so a mode change parses again and fenced
/// code takes the new palette.
pub fn view(id: impl Into<ElementId>, text: &str, cx: &App) -> TextView {
    let sanitized: SharedString = sanitize_untrusted(text).into();
    let keyed = SharedString::from(format!("{}/{}", id.into(), cx.dv_theme().key()));
    TextView::markdown(keyed, sanitized)
        .style(TextViewStyle::default().paragraph_gap(gpui::rems(0.5)))
        .on_link_click(|url, _event, _window, cx| crate::system::open_url(url, cx))
}

/// [`view`] whose links go to `on_link` instead of [`crate::system::open_url`]: the hover
/// card's, where a link to a file opens it in dv (PLAN §18.1). `on_link` must still send any
/// other link through `open_url`.
pub fn view_with_links(
    id: impl Into<ElementId>,
    text: &str,
    on_link: impl Fn(&str, &mut App) + Send + Sync + 'static,
    cx: &App,
) -> TextView {
    view(id, text, cx).on_link_click(move |url, _event, _window, cx| on_link(url, cx))
}

/// Neutralises the constructs that make `TextView` fetch anything or render markup from an
/// untrusted body, before the source reaches it:
///
/// - **Images** (`![alt](url)` and reference-style `![alt][ref]`) become an inert literal
///   `[image: alt]` (or `[image]`), so no URL survives to be fetched.
/// - **Raw HTML** (inline tags in any casing, and whole HTML blocks) is entity-escaped (`&` to
///   `&amp;`, `<` to `&lt;`) and shows as literal text.
///
/// The edits are made at the spans `markdown::to_mdast` reports, the parser `TextView` uses
/// itself, so code is left alone for free: text in fenced and indented blocks and inline code
/// spans lives under `Code`/`InlineCode` nodes, never `Image` or `Html` ones. Everything else
/// (links, emphasis, tables, lists) passes through. When the parse fails or a span doesn't map
/// onto the source, the whole body is escaped coarsely instead: uglier, still fetch-proof.
pub fn sanitize_untrusted(source: &str) -> String {
    let Ok(ast) = markdown::to_mdast(source, &markdown::ParseOptions::gfm()) else {
        return coarse_escape(source);
    };
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    if !collect_unsafe_spans(&ast, source, &mut edits) {
        return coarse_escape(source);
    }
    if edits.is_empty() {
        return source.to_string();
    }
    edits.sort_by_key(|&(start, _, _)| start);
    let mut out = String::with_capacity(source.len());
    let mut cursor = 0usize;
    for (start, end, replacement) in edits {
        // Spans never nest (a replaced node isn't descended into); skip an overlap rather than
        // emit text twice.
        if start < cursor {
            continue;
        }
        out.push_str(&source[cursor..start]);
        out.push_str(&replacement);
        cursor = end;
    }
    out.push_str(&source[cursor..]);
    out
}

/// The first line of a Markdown body as plain text, for one-line summaries (the comments list,
/// the sheet's comments, a collapsed card; DESIGN §4.1 "the first line of the comment"): the
/// first line with text of the first block that has any, its inline markup dropped (emphasis,
/// code spans and links read as their text; an image as `[image: alt]`), a fenced block by its
/// first line of code rather than its fence, a heading without its `#`s, a list item without
/// its marker. Trimmed; empty for a body with no text. A body that doesn't parse falls back to
/// its first non-empty line as written.
pub fn first_line_plain(source: &str) -> String {
    let parsed = markdown::to_mdast(source, &markdown::ParseOptions::gfm());
    match parsed {
        Ok(ast) => block_line(&ast).unwrap_or_default(),
        Err(_) => first_text_line(source).unwrap_or_default(),
    }
}

/// The first line with text of the first block under `node` that has one.
fn block_line(node: &markdown::mdast::Node) -> Option<String> {
    use markdown::mdast::Node;
    match node {
        Node::Code(code) => first_text_line(&code.value),
        Node::Math(math) => first_text_line(&math.value),
        Node::Html(html) => first_text_line(&html.value),
        Node::Paragraph(_) | Node::Heading(_) => {
            let mut text = String::new();
            inline_text(node, &mut text);
            first_text_line(&text)
        }
        Node::TableRow(row) => {
            let cells: Vec<String> = row
                .children
                .iter()
                .map(|cell| {
                    let mut text = String::new();
                    inline_text(cell, &mut text);
                    text.split_whitespace().collect::<Vec<_>>().join(" ")
                })
                .collect();
            first_text_line(&cells.join(" | "))
        }
        Node::ThematicBreak(_) | Node::Definition(_) => None,
        other => other.children()?.iter().find_map(block_line),
    }
}

/// Appends the text of the inline content under `node`, without its markup; a hard break is a
/// newline.
fn inline_text(node: &markdown::mdast::Node, out: &mut String) {
    use markdown::mdast::Node;
    match node {
        Node::Text(text) => out.push_str(&text.value),
        Node::InlineCode(code) => out.push_str(&code.value),
        Node::InlineMath(math) => out.push_str(&math.value),
        Node::Html(html) => out.push_str(&html.value),
        Node::Break(_) => out.push('\n'),
        Node::Image(image) => out.push_str(&image_placeholder(&image.alt)),
        Node::ImageReference(image) => out.push_str(&image_placeholder(&image.alt)),
        other => {
            for child in other.children().into_iter().flatten() {
                inline_text(child, out);
            }
        }
    }
}

/// The first line of `text` with anything on it, trimmed.
fn first_text_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Collects a `(start, end, replacement)` byte-span edit for every image and raw-HTML node
/// under `node`. `false` when a node's span can't be mapped onto `source` (no position, out of
/// range, off a char boundary), which makes the caller escape the whole body.
fn collect_unsafe_spans(
    node: &markdown::mdast::Node,
    source: &str,
    edits: &mut Vec<(usize, usize, String)>,
) -> bool {
    use markdown::mdast::Node;
    let span_of = |node: &Node| -> Option<(usize, usize)> {
        let position = node.position()?;
        let (start, end) = (position.start.offset, position.end.offset);
        source.get(start..end).map(|_| (start, end))
    };
    let mut edit = |node: &Node, replacement: &dyn Fn(&str) -> String| match span_of(node) {
        Some((start, end)) => {
            edits.push((start, end, replacement(&source[start..end])));
            true
        }
        None => false,
    };
    match node {
        Node::Image(image) => edit(node, &|_| image_placeholder(&image.alt)),
        Node::ImageReference(image) => edit(node, &|_| image_placeholder(&image.alt)),
        Node::Html(_) => edit(node, &coarse_escape),
        other => match other.children() {
            Some(children) => children
                .iter()
                .all(|child| collect_unsafe_spans(child, source, edits)),
            None => true,
        },
    }
}

/// The inert literal an image becomes: its alt text, stripped of every character that could
/// reopen Markdown or HTML syntax once spliced back into the source.
fn image_placeholder(alt: &str) -> String {
    let clean: String = alt
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | '(' | ')' | '<' | '>' | '`' | '!' | '&'))
        .collect();
    let clean = clean.trim();
    if clean.is_empty() {
        "[image]".to_string()
    } else {
        format!("[image: {clean}]")
    }
}

/// Escapes HTML (`&`, `<`) and breaks image syntax (`![` to `!\[`), so the result can only
/// render as literal text: for a raw-HTML node's span (an image inside an HTML block must not
/// come back to life once the tags are escaped) and for a whole body when span edits aren't
/// possible.
fn coarse_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace("![", "!\\[")
}

#[cfg(test)]
mod tests;
