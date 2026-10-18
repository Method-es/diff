//! Syntax highlighting for the diff stream (PLAN §6 step 4): tree-sitter, through
//! gpui-component's highlighter, over each side's full text, producing **token kinds** rather
//! than colours.
//!
//! A kind resolves to a colour only when a row paints ([`TokenKind::color`], from the theme's
//! [`SyntaxColors`]), so a theme or mode change repaints without highlighting again, and a
//! cached result never holds a stale colour. The highlighter's capture names (`keyword.return`,
//! `function.method`, `string.escape`, …) map to kinds through one table
//! ([`kind_for_capture`]); the highlighter reports styles, so the resolver hands it a style
//! that encodes the kind and the result is decoded back, losslessly.
//!
//! Highlighting runs off the UI thread, once per loaded file side ([`highlight_side`]), and is
//! capped: a side over [`MAX_HIGHLIGHT_BYTES`] isn't highlighted at all, and a line over
//! [`MAX_LINE_BYTES`] keeps no tokens (one minified line would otherwise carry thousands of
//! runs). The language comes from the path ([`language_for_path`]); a path dv has no grammar
//! for is plain text. Two of gpui-component's grammars are corrected first
//! ([`register_languages`]).

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Once;

use dv_core::diff::TextDiff;
use dv_core::review::{AnchorSide, side_lines};
use gpui::{HighlightStyle, Hsla};
use gpui_component::Rope;
use gpui_component::highlighter::{LanguageConfig, LanguageRegistry, SyntaxHighlighter};
use gpui_component::input::HighlightStyleResolver;

use crate::theme::SyntaxColors;

/// A side larger than this (its lines and their newlines, in bytes) isn't highlighted: the
/// same 2 MB as the diff pipeline's own cap (`dv_core::diff::MAX_FILE_BYTES`), which only
/// "Load anyway" passes.
pub const MAX_HIGHLIGHT_BYTES: usize = 2 * 1024 * 1024;

/// A line longer than this, in bytes, keeps no tokens and paints plain.
pub const MAX_LINE_BYTES: usize = 10_000;

/// What a piece of code is, as far as colouring goes (DESIGN §1.1, §1.2 syntax tokens).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Keyword,
    Function,
    String,
    Number,
    Type,
    Constant,
    Property,
    Punctuation,
    Comment,
    Plain,
}

impl TokenKind {
    /// Every kind, in the order [`TokenKind::index`] numbers them.
    pub const ALL: [TokenKind; 10] = [
        Self::Keyword,
        Self::Function,
        Self::String,
        Self::Number,
        Self::Type,
        Self::Constant,
        Self::Property,
        Self::Punctuation,
        Self::Comment,
        Self::Plain,
    ];

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|kind| *kind == self)
            .unwrap_or(Self::ALL.len() - 1)
    }

    /// `"keyword"`, `"function"`, …, as automation state and tests spell it.
    pub fn key(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Function => "function",
            Self::String => "string",
            Self::Number => "number",
            Self::Type => "type",
            Self::Constant => "constant",
            Self::Property => "property",
            Self::Punctuation => "punctuation",
            Self::Comment => "comment",
            Self::Plain => "plain",
        }
    }

    /// The colour this kind paints in, from the installed theme's syntax tokens.
    pub fn color(self, syntax: &SyntaxColors) -> Hsla {
        match self {
            Self::Keyword => syntax.keyword,
            Self::Function => syntax.function,
            Self::String => syntax.string,
            Self::Number => syntax.number,
            Self::Type => syntax.ty,
            Self::Constant => syntax.constant,
            Self::Property => syntax.property,
            Self::Punctuation => syntax.punctuation,
            Self::Comment => syntax.comment,
            Self::Plain => syntax.plain,
        }
    }
}

/// The kind a tree-sitter capture name stands for; `None` for a name that says nothing about
/// colour (a query's private `_name` captures, unknown names), which leaves the enclosing
/// capture's kind in place.
///
/// A capture that names plain code explicitly (`variable`, `variable.parameter`, `embedded`,
/// `module`) is [`TokenKind::Plain`], so a variable interpolated into a string paints plain,
/// not as the string around it. The table, by the name's first segment unless the full name is
/// listed:
///
/// | Kind | Names |
/// |---|---|
/// | Keyword | `keyword*`, `conditional`, `repeat`, `include`, `import`, `exception`, `storageclass`, `preproc`, `variable.builtin`, `variable.special` (`self`, `this`), `tag*` (markup tag names), `title` (Markdown headings) |
/// | Function | `function*`, `method*`, `link_text` |
/// | String | `string*`, `character*`, `escape`, `text.literal`, `text.code.span`, `text.uri`, `link_uri` |
/// | Number | `number*`, `float` |
/// | Type | `type*`, `constructor` |
/// | Constant | `constant*`, `boolean`, `label`, `string.special.symbol` |
/// | Property | `property*`, `field`, `attribute`, `tag.attribute`, `variable.member` |
/// | Punctuation | `punctuation*`, `operator`, `delimiter`, `tag.delimiter` |
/// | Comment | `comment*` |
/// | Plain | `variable*` (others), `parameter`, `embedded`, `module*`, `namespace`, `emphasis*`, `text*` (others), `none`, `spell`, `nospell` |
pub fn kind_for_capture(name: &str) -> Option<TokenKind> {
    use TokenKind::*;
    if name.is_empty() || name.starts_with('_') {
        return None;
    }
    let exact = match name {
        "boolean" | "label" | "string.special.symbol" => Some(Constant),
        "constructor" => Some(Type),
        "variable.builtin" | "variable.special" | "conditional" | "repeat" | "include"
        | "import" | "exception" | "storageclass" | "preproc" | "title" => Some(Keyword),
        "attribute" | "field" | "tag.attribute" | "variable.member" => Some(Property),
        "operator" | "delimiter" | "tag.delimiter" => Some(Punctuation),
        "escape" | "text.literal" | "text.code.span" | "text.uri" | "link_uri" => Some(String),
        "float" => Some(Number),
        "link_text" => Some(Function),
        _ => None,
    };
    if exact.is_some() {
        return exact;
    }
    let first = name.split('.').next().unwrap_or(name);
    match first {
        "keyword" | "tag" => Some(Keyword),
        "function" | "method" => Some(Function),
        "string" | "character" => Some(String),
        "number" => Some(Number),
        "type" => Some(Type),
        "constant" => Some(Constant),
        "property" => Some(Property),
        "punctuation" => Some(Punctuation),
        "comment" => Some(Comment),
        "variable" | "parameter" | "embedded" | "module" | "namespace" | "emphasis" | "text"
        | "none" | "spell" | "nospell" => Some(Plain),
        _ => None,
    }
}

/// The highlighter language for a repo path, from well-known file names and then the
/// extension (case-insensitive); `None` for a path dv has no grammar for, which paints plain.
/// Every name returned is one of the grammars the workspace enables for gpui-component.
pub fn language_for_path(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    let by_name = match lower.as_str() {
        "makefile" | "gnumakefile" => Some("make"),
        "cmakelists.txt" => Some("cmake"),
        ".bashrc" | ".bash_profile" | ".bash_aliases" | ".bash_logout" | ".profile" | ".zshrc"
        | ".zprofile" | ".zshenv" | "pkgbuild" => Some("bash"),
        "gemfile" | "rakefile" | "guardfile" | "podfile" | "vagrantfile" | "brewfile" => {
            Some("ruby")
        }
        "cargo.lock" | "pipfile" | "poetry.lock" | "uv.lock" => Some("toml"),
        ".babelrc" | ".eslintrc" | ".prettierrc" | "flake.lock" => Some("json"),
        _ => None,
    };
    if by_name.is_some() {
        return by_name;
    }
    let (_, ext) = lower.rsplit_once('.')?;
    Some(match ext {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "json" | "jsonc" | "json5" => "json",
        "py" | "pyi" | "pyw" => "python",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" | "ipp" | "inl" => "cpp",
        "cs" => "csharp",
        "css" | "scss" => "css",
        "html" | "htm" | "xhtml" => "html",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "lua" => "lua",
        "md" | "markdown" | "mdx" => "markdown",
        "php" | "phtml" => "php",
        "proto" => "proto",
        "rb" | "rake" | "gemspec" => "ruby",
        "scala" | "sc" => "scala",
        "svelte" => "svelte",
        "swift" => "swift",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "zig" => "zig",
        "sh" | "bash" | "zsh" => "bash",
        "ex" | "exs" => "elixir",
        "erb" => "erb",
        "ejs" => "ejs",
        "graphql" | "gql" => "graphql",
        "astro" => "astro",
        "diff" | "patch" => "diff",
        "cmake" => "cmake",
        "mk" | "mak" => "make",
        "sql" | "pgsql" => "sql",
        _ => return None,
    })
}

/// JSX's own captures, which neither of gpui-component's JavaScript and TypeScript queries has:
/// an element's name is a tag when it is lower case (`div`, an intrinsic element) and a type
/// when it is capitalised (`Card`, a component), an attribute's name is a property, and the
/// element's angle brackets are tag punctuation. The patterns go before the language's own, so
/// they win over its `(identifier) @variable` for the same node.
const JSX_HIGHLIGHTS: &str = r#"
(jsx_opening_element name: (identifier) @tag (#match? @tag "^[a-z]"))
(jsx_closing_element name: (identifier) @tag (#match? @tag "^[a-z]"))
(jsx_self_closing_element name: (identifier) @tag (#match? @tag "^[a-z]"))
(jsx_opening_element name: (identifier) @type (#match? @type "^[A-Z]"))
(jsx_closing_element name: (identifier) @type (#match? @type "^[A-Z]"))
(jsx_self_closing_element name: (identifier) @type (#match? @type "^[A-Z]"))
(jsx_attribute (property_identifier) @tag.attribute)
(jsx_opening_element ["<" ">"] @tag.delimiter)
(jsx_closing_element ["</" ">"] @tag.delimiter)
(jsx_self_closing_element ["<" "/>"] @tag.delimiter)
"#;

/// SQL's own captures ahead of tree-sitter-sequel's query, which is written for Neovim: its
/// number patterns use Lua's `%d` (a literal `d` to tree-sitter's regular expressions) and come
/// after its catch-all `(literal) @string`, so numbers would stay strings; and it lists `NULL`
/// among the built-in types. The patterns go first, so they win for the same node.
const SQL_HIGHLIGHTS: &str = r#"
(keyword_null) @constant.builtin
((literal) @number (#match? @number "^[-+]?[0-9]+$"))
((literal) @float (#match? @float "^[-+]?[0-9]*\.[0-9]*$"))
"#;

/// SQL's query as dv highlights it: [`SQL_HIGHLIGHTS`] first, and the query's lists of
/// qualifier and option keywords (`DEFAULT`, `UNSIGNED`, `AUTO_INCREMENT`, …), which it tags
/// `@type.qualifier` and `@attribute` and which would read as a type and a property, tagged as
/// keywords. Each of those tags names only that one list in the query.
fn sql_highlights(query: &str) -> String {
    let query = query
        .replace("] @type.qualifier", "] @keyword")
        .replace("] @attribute", "] @keyword");
    format!("{SQL_HIGHLIGHTS}\n{query}")
}

/// Corrects gpui-component's TSX, JavaScript and SQL grammars in its language registry, once
/// per process, before anything highlights with them (the diff stream, and comment Markdown's
/// fenced code). gpui-component pairs the TSX grammar with tree-sitter-typescript's bare query,
/// which only adds TypeScript's syntax to JavaScript's and so leaves most of a file plain: TSX
/// takes the query `.ts` files use instead. Both TSX and JavaScript (`.jsx` included) gain
/// [`JSX_HIGHLIGHTS`]; SQL takes [`sql_highlights`].
pub fn register_languages() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let registry = LanguageRegistry::singleton();
        let with_jsx = |highlights: &str| format!("{JSX_HIGHLIGHTS}\n{highlights}").into();
        if let (Some(typescript), Some(tsx)) =
            (registry.language("typescript"), registry.language("tsx"))
        {
            let highlights = with_jsx(&typescript.highlights);
            registry.register(
                "tsx",
                &LanguageConfig {
                    name: tsx.name,
                    language: tsx.language,
                    highlights,
                    ..typescript
                },
            );
        }
        if let Some(javascript) = registry.language("javascript") {
            let highlights = with_jsx(&javascript.highlights);
            registry.register(
                "javascript",
                &LanguageConfig {
                    highlights,
                    ..javascript
                },
            );
        }
        if let Some(sql) = registry.language("sql") {
            let highlights = sql_highlights(&sql.highlights).into();
            registry.register("sql", &LanguageConfig { highlights, ..sql });
        }
    });
}

/// One token on a line: a byte range of the line's text and its kind. Lines keep only their
/// non-plain tokens, sorted and non-overlapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub range: Range<u32>,
    pub kind: TokenKind,
}

/// One side's tokens, line by line: `line(n)` is line `n`'s (1-based).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SideHighlights {
    lines: Vec<Box<[Token]>>,
}

impl SideHighlights {
    /// Line `n`'s tokens (1-based); empty past the end and for a line that paints plain.
    pub fn line(&self, n: u32) -> &[Token] {
        (n as usize)
            .checked_sub(1)
            .and_then(|index| self.lines.get(index))
            .map(|tokens| &tokens[..])
            .unwrap_or_default()
    }

    /// How many lines the side has.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// Both sides of one file's text diff, highlighted. A side is `None` when it has no lines,
/// has no grammar or is over the cap.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileHighlights {
    pub old: Option<SideHighlights>,
    pub new: Option<SideHighlights>,
}

impl FileHighlights {
    /// The side `side`'s tokens.
    pub fn side(&self, side: AnchorSide) -> Option<&SideHighlights> {
        match side {
            AnchorSide::Old => self.old.as_ref(),
            AnchorSide::New => self.new.as_ref(),
        }
    }
}

/// Highlights both sides of `diff`, the old one as `old_path` (a renamed file's old name)
/// and the new one as `path`. Blocking: runs on a background thread.
pub fn highlight_diff(path: &str, old_path: Option<&str>, diff: &TextDiff) -> FileHighlights {
    let old = side_lines(diff, AnchorSide::Old);
    let new = side_lines(diff, AnchorSide::New);
    FileHighlights {
        old: highlight_side(old_path.unwrap_or(path), &old),
        new: highlight_side(path, &new),
    }
}

/// Highlights one side's full text, `lines[0]` being line 1 (without line endings), in the
/// language `path` names. `None` when there is nothing to do: no lines, no grammar for the
/// path, or more than [`MAX_HIGHLIGHT_BYTES`]. Blocking: runs on a background thread.
pub fn highlight_side<S: AsRef<str>>(path: &str, lines: &[S]) -> Option<SideHighlights> {
    let language = language_for_path(path)?;
    if lines.is_empty() {
        return None;
    }
    let len = lines
        .iter()
        .map(|line| line.as_ref().len() + 1)
        .sum::<usize>();
    if len > MAX_HIGHLIGHT_BYTES {
        return None;
    }
    let mut text = String::with_capacity(len);
    let mut starts = Vec::with_capacity(lines.len());
    for line in lines {
        starts.push(text.len());
        text.push_str(line.as_ref());
        text.push('\n');
    }
    register_languages();
    let mut highlighter = SyntaxHighlighter::new(language);
    highlighter.update(None, &Rope::from_str(&text), None);
    let styles = highlighter.styles(&(0..text.len()), &KindResolver);
    Some(bucket(&text, &starts, &styles))
}

/// Highlights lines one at a time, each on its own, as if it were the whole file, so nothing a
/// line opens (a block comment, a template literal) colours another: a references list's rows
/// (`crate::intel::references`), from many files. It keeps one highlighter per language, its
/// grammar's queries compiled once, and has it parse each line afresh (an edit replacing all
/// of the line before). Blocking: runs on a background thread.
#[derive(Default)]
pub struct LineHighlighter {
    highlighters: HashMap<&'static str, (SyntaxHighlighter, usize)>,
}

impl LineHighlighter {
    pub fn new() -> Self {
        Self::default()
    }

    /// `line`'s tokens, highlighted in the language `path` names; none without a grammar, or
    /// for a line over [`MAX_LINE_BYTES`].
    pub fn highlight(&mut self, path: &str, line: &str) -> Vec<Token> {
        let Some(language) = language_for_path(path) else {
            return Vec::new();
        };
        if line.len() > MAX_LINE_BYTES {
            return Vec::new();
        }
        register_languages();
        let (highlighter, before) = self
            .highlighters
            .entry(language)
            .or_insert_with(|| (SyntaxHighlighter::new(language), 0));
        // All of the line before replaced by this one: nothing of its parse is kept.
        let edit = tree_sitter::InputEdit {
            start_byte: 0,
            old_end_byte: *before,
            new_end_byte: line.len(),
            start_position: tree_sitter::Point::new(0, 0),
            old_end_position: tree_sitter::Point::new(0, *before),
            new_end_position: tree_sitter::Point::new(0, line.len()),
        };
        highlighter.update(Some(edit), &Rope::from_str(line), None);
        *before = line.len();
        let styles = highlighter.styles(&(0..line.len()), &KindResolver);
        let mut text = String::with_capacity(line.len() + 1);
        text.push_str(line);
        text.push('\n');
        bucket(&text, &[0], &styles).line(1).to_vec()
    }
}

/// Splits whole-text runs into line-relative tokens: plain runs and the newline between lines
/// are dropped, a run across lines is cut at each line's end, and lines over
/// [`MAX_LINE_BYTES`] keep nothing.
fn bucket(
    text: &str,
    starts: &[usize],
    styles: &[(Range<usize>, HighlightStyle)],
) -> SideHighlights {
    let mut lines: Vec<Vec<Token>> = vec![Vec::new(); starts.len()];
    for (range, style) in styles {
        let kind = decode(style);
        if kind == TokenKind::Plain {
            continue;
        }
        let mut pos = range.start;
        while pos < range.end {
            // The line holding `pos`: the last start at or before it.
            let index = starts.partition_point(|&start| start <= pos) - 1;
            let start = starts[index];
            // The line's text ends before its newline.
            let end = starts.get(index + 1).map_or(text.len(), |next| next - 1);
            let cut = range.end.min(end);
            if cut > pos && end - start <= MAX_LINE_BYTES {
                let tokens = &mut lines[index];
                let token = Token {
                    range: (pos - start) as u32..(cut - start) as u32,
                    kind,
                };
                match tokens.last_mut() {
                    // Adjacent runs of one kind are one token.
                    Some(last) if last.kind == kind && last.range.end == token.range.start => {
                        last.range.end = token.range.end
                    }
                    _ => tokens.push(token),
                }
            }
            pos = starts
                .get(index + 1)
                .copied()
                .unwrap_or(text.len())
                .max(pos + 1);
        }
    }
    SideHighlights {
        lines: lines.into_iter().map(Vec::into_boxed_slice).collect(),
    }
}

/// Hands the highlighter a style per capture that carries the capture's kind in its colour's
/// hue (the kind's index), which [`decode`] reads back. The highlighter only copies colours
/// from one capture to the run it covers, never blends them, so nothing is lost.
struct KindResolver;

impl HighlightStyleResolver for KindResolver {
    fn style(&self, name: &str) -> Option<HighlightStyle> {
        kind_for_capture(name).map(encode)
    }
}

fn encode(kind: TokenKind) -> HighlightStyle {
    HighlightStyle {
        color: Some(Hsla {
            h: kind.index() as f32,
            s: 0.,
            l: 0.,
            a: 1.,
        }),
        ..Default::default()
    }
}

fn decode(style: &HighlightStyle) -> TokenKind {
    style
        .color
        .and_then(|color| {
            let index = color.h as usize;
            (color.h == index as f32)
                .then(|| TokenKind::ALL.get(index).copied())
                .flatten()
        })
        .unwrap_or(TokenKind::Plain)
}

#[cfg(test)]
mod tests;
