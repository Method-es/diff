//! A code line as it paints (DESIGN §4.1): tabs as four spaces, its syntax kinds and intraline
//! words merged into one list of styled runs, and only the columns in view, so a long line (a
//! minified file's) costs no more to draw than a short one.
//!
//! Which columns are in view counts characters, a tab taking [`TAB_WIDTH`], so the sideways
//! scroll moves by the code font's advance. Where a character is drawn is the shaped line's
//! business, not this module's: a wide character (CJK, an emoji) takes two cells, and a combining
//! mark none. [`Visible::painted_of`] maps a byte of the source line to a byte of the painted
//! slice, and the pointer and the painted marks go through the slice's shaped line with it
//! (`select`, `render::code`): [`caret_at`] and [`unit_at`] take any monotonic `x_of(byte)`.
//! Every range here is a byte range on character boundaries.

use std::ops::Range;

use crate::syntax::{Token, TokenKind};

/// Columns a tab takes (DESIGN §4.1: tabs render as four spaces).
pub const TAB_WIDTH: usize = 4;

/// How a run of a painted line looks: its syntax kind, and whether it is an intraline word
/// (`addWord` / `delWord` behind it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style {
    pub kind: TokenKind,
    pub word: bool,
}

/// The columns of a line in view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Visible {
    pub text: String,
    /// Styled runs of `text`, sorted and apart; text outside them is plain.
    pub runs: Vec<(Range<usize>, Style)>,
    /// The line goes on past the last column in view.
    pub clipped: bool,
    /// Each source character shown, in order: where it starts in `text` and in the source line
    /// (a tab's painted spaces start at its one source byte).
    pub map: Vec<(usize, usize)>,
}

impl Visible {
    /// The byte of the painted slice where source byte `source` paints: a byte before the slice
    /// at its start, one after it at its end.
    pub fn painted_of(&self, source: usize) -> usize {
        match self.map.iter().find(|&&(_, from)| from >= source) {
            Some(&(painted, _)) => painted,
            None => self.text.len(),
        }
    }
}

/// The columns `text` takes.
pub fn columns(text: &str) -> usize {
    text.chars()
        .map(|ch| if ch == '\t' { TAB_WIDTH } else { 1 })
        .sum()
}

/// The painted column where byte `offset` of `text` starts (an offset past the end counts as the
/// end), counting a tab as [`TAB_WIDTH`] and anything else as one: the fallback geometry for a
/// line not drawn this frame, and the tests' monospaced model.
pub fn column_at(text: &str, offset: usize) -> usize {
    let mut column = 0;
    for (at, ch) in text.char_indices() {
        if at >= offset {
            break;
        }
        column += if ch == '\t' { TAB_WIDTH } else { 1 };
    }
    column
}

/// Whether `ch` belongs with the character before it rather than standing alone: a combining
/// mark, a zero-width joiner, a variation selector (an emoji's) or a skin-tone modifier.
pub fn is_extending(ch: char) -> bool {
    matches!(ch,
        '\u{0300}'..='\u{036F}'
        | '\u{1AB0}'..='\u{1AFF}'
        | '\u{1DC0}'..='\u{1DFF}'
        | '\u{20D0}'..='\u{20FF}'
        | '\u{FE20}'..='\u{FE2F}'
        | '\u{200D}'
        | '\u{FE00}'..='\u{FE0F}'
        | '\u{1F3FB}'..='\u{1F3FF}'
        | '\u{E0100}'..='\u{E01EF}')
}

/// The characters of `text` as the pointer takes them: each with the extending characters
/// after it ([`is_extending`]), so a caret never falls between a letter and its accent.
pub fn units(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut chars = text.char_indices().peekable();
    std::iter::from_fn(move || {
        let (start, ch) = chars.next()?;
        let mut end = start + ch.len_utf8();
        while let Some(&(at, next)) = chars.peek() {
            if !is_extending(next) {
                break;
            }
            end = at + next.len_utf8();
            chars.next();
        }
        Some(start..end)
    })
}

/// How far into a character the pointer goes before the caret goes after it: past its middle, so
/// a press on a character's centre puts the caret before it.
pub const CARET_BIAS: f32 = 0.6;

/// The unit ([`units`]) drawn at `x`, `x_of(byte)` being where each byte of `text` is drawn
/// (increasing; a unit not drawn spans nothing): the one under `x`, the first drawn one before
/// them all, the last past them; `None` for an empty line.
pub fn unit_at(text: &str, x: f32, x_of: impl Fn(usize) -> f32) -> Option<Range<usize>> {
    let mut last = None;
    for unit in units(text) {
        let (start, end) = (x_of(unit.start), x_of(unit.end));
        if end > start && x < end {
            return Some(unit);
        }
        if end > start {
            last = Some(unit);
        }
    }
    last.or_else(|| units(text).last())
}

/// Where a pointer at `x` puts the caret in `text` (`x_of` as for [`unit_at`]): before the unit
/// under it, or after it once the pointer is past [`CARET_BIAS`] of its width; before the first
/// unit drawn when the pointer is before it, at the end of the last past it.
pub fn caret_at(text: &str, x: f32, x_of: impl Fn(usize) -> f32) -> usize {
    let Some(unit) = unit_at(text, x, &x_of) else {
        return 0;
    };
    let (start, end) = (x_of(unit.start), x_of(unit.end));
    if x < start || (x < end && x - start < (end - start) * CARET_BIAS) {
        unit.start
    } else {
        unit.end
    }
}

/// What a double-click selects around the character at byte range `at`: the run of characters
/// of its class around it — identifier characters (alphanumerics and `_`), whitespace, or any
/// other character — each with the extending characters after it.
pub fn word_at(text: &str, at: Range<usize>) -> Range<usize> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |ch: char| {
        if ch.is_alphanumeric() || ch == '_' {
            Class::Word
        } else if ch.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let Some(ch) = text.get(at.clone()).and_then(|s| s.chars().next()) else {
        return at;
    };
    let kind = class(ch);
    let same = |ch: char| is_extending(ch) || class(ch) == kind;
    let mut start = at.start;
    for (i, ch) in text[..at.start].char_indices().rev() {
        if !same(ch) {
            break;
        }
        start = i;
    }
    // A run can't start on an extending character: it goes with the one before it.
    while let Some(ch) = text[start..].chars().next().filter(|&ch| is_extending(ch)) {
        start += ch.len_utf8();
    }
    let end = text[at.end..]
        .char_indices()
        .find(|&(_, ch)| !same(ch))
        .map_or(text.len(), |(i, _)| at.end + i);
    start.min(at.start)..end
}
/// The 1-based character column of byte `offset` (a tab is one character): how automation names
/// a place in a line.
pub fn char_column(text: &str, offset: usize) -> usize {
    text.get(..offset)
        .map_or_else(|| text.chars().count(), |before| before.chars().count())
        + 1
}

/// The byte offset of 1-based character column `column`; the line's end for the column after
/// its last character, `None` further on (or for column 0).
pub fn char_offset(text: &str, column: usize) -> Option<usize> {
    let index = column.checked_sub(1)?;
    text.char_indices()
        .map(|(at, _)| at)
        .chain(std::iter::once(text.len()))
        .nth(index)
}

/// Columns `first..first + count` of `text`, with `tokens` (sorted, apart; ranges of `text`)
/// and `words` (sorted, apart; ranges of `text`) styled. A tab cut by either edge shows the
/// part of it in view.
pub fn visible(
    text: &str,
    tokens: &[Token],
    words: &[Range<usize>],
    first: usize,
    count: usize,
) -> Visible {
    let end = first.saturating_add(count);
    let mut out = Visible::default();
    let (mut token, mut word) = (0, 0);
    let mut column = 0;
    for (at, ch) in text.char_indices() {
        if column >= end {
            out.clipped = true;
            break;
        }
        let width = if ch == '\t' { TAB_WIDTH } else { 1 };
        let start_column = column;
        column += width;
        if column <= first {
            continue;
        }
        while tokens
            .get(token)
            .is_some_and(|t| (t.range.end as usize) <= at)
        {
            token += 1;
        }
        while words.get(word).is_some_and(|w| w.end <= at) {
            word += 1;
        }
        let style = Style {
            kind: tokens
                .get(token)
                .filter(|t| (t.range.start as usize) <= at)
                .map_or(TokenKind::Plain, |t| t.kind),
            word: words.get(word).is_some_and(|w| w.start <= at),
        };
        let start = out.text.len();
        out.map.push((start, at));
        if ch == '\t' {
            let shown = column.min(end) - start_column.max(first);
            out.text.extend(std::iter::repeat_n(' ', shown));
        } else {
            out.text.push(ch);
        }
        if style.kind == TokenKind::Plain && !style.word {
            continue;
        }
        let range = start..out.text.len();
        match out.runs.last_mut() {
            Some((last, last_style)) if *last_style == style && last.end == range.start => {
                last.end = range.end
            }
            _ => out.runs.push((range, style)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    /// The monospaced model: each byte drawn at its column.
    fn caret_offset(text: &str, column: f32) -> usize {
        caret_at(text, column, |at| column_at(text, at) as f32)
    }

    fn char_at_column(text: &str, column: f32) -> Option<Range<usize>> {
        unit_at(text, column, |at| column_at(text, at) as f32)
    }

    /// A shaped model where CJK and emoji take two cells and combining marks none.
    fn shaped_x(text: &str, offset: usize) -> f32 {
        text.char_indices()
            .take_while(|&(at, _)| at < offset)
            .map(|(_, ch)| match ch {
                ch if is_extending(ch) => 0.,
                '\u{3000}'..='\u{9FFF}' | '\u{1F300}'..='\u{1FAFF}' => 2.,
                _ => 1.,
            })
            .sum()
    }

    #[test]
    fn wide_characters_and_combining_marks_map_through_the_shaped_line() {
        let text = "名前 = \"😀\"; e\u{0301}x";
        let x_of = |at| shaped_x(text, at);
        // `名` takes cells 0–2, `前` 2–4: the middle of `前` is before it.
        assert_eq!(&text[unit_at(text, 2.5, x_of).unwrap()], "前");
        assert_eq!(caret_at(text, 2.5, x_of), 3);
        assert_eq!(caret_at(text, 3.5, x_of), 6);
        // The emoji is one unit two cells wide.
        let emoji = text.find('😀').unwrap();
        let at = shaped_x(text, emoji);
        assert_eq!(unit_at(text, at + 1.5, x_of), Some(emoji..emoji + 4));
        // An accent goes with its letter: the caret is never between them.
        let e = text.find('e').unwrap();
        let ex = shaped_x(text, e);
        assert_eq!(unit_at(text, ex + 0.5, x_of), Some(e..e + 3));
        assert_eq!(caret_at(text, ex + 0.9, x_of), e + 3);
        assert_eq!(&text[word_at(text, e..e + 3)], "e\u{0301}x");
    }

    #[test]
    fn the_painted_slice_maps_source_bytes_with_tabs_and_scrolling() {
        // Scrolled 3 columns in: the tab's last column, then `b`.
        let shown = visible("a\tbc", &[], &[], 3, 10);
        assert_eq!(shown.text, "  bc");
        assert_eq!(shown.map, [(0, 1), (2, 2), (3, 3)]);
        // `a`, before the slice, paints at its start; the tab's byte where its spaces start.
        assert_eq!(shown.painted_of(0), 0);
        assert_eq!(shown.painted_of(1), 0);
        assert_eq!(shown.painted_of(2), 2);
        assert_eq!(shown.painted_of(4), 4);
        // A byte paints at its source column less the scroll, the same column `column_at` names.
        assert_eq!(column_at("a\tbc", 2) - 3, shown.painted_of(2));
        let wide = visible("名前x", &[], &[], 0, 10);
        assert_eq!(wide.map, [(0, 0), (3, 3), (6, 6)]);
        assert_eq!(wide.painted_of(3), 3);
    }

    fn token(range: Range<u32>, kind: TokenKind) -> Token {
        Token { range, kind }
    }

    /// Byte ranges from `(start, end)` pairs.
    fn ranges(pairs: &[(usize, usize)]) -> Vec<Range<usize>> {
        pairs.iter().map(|&(start, end)| start..end).collect()
    }

    fn style(kind: TokenKind, word: bool) -> Style {
        Style { kind, word }
    }

    #[test]
    fn tabs_count_four_columns() {
        assert_eq!(columns("a\tb"), 6);
        assert_eq!(columns("héllo"), 5);
        assert_eq!(columns(""), 0);
    }

    #[test]
    fn tokens_and_words_merge_into_runs_that_never_overlap() {
        let text = "let x = 1;";
        let tokens = [
            token(0..3, TokenKind::Keyword),
            token(8..9, TokenKind::Number),
            token(9..10, TokenKind::Punctuation),
        ];
        // The word covers "x = 1" and cuts the number token's run.
        let words = ranges(&[(4, 9)]);
        let shown = visible(text, &tokens, &words, 0, 80);
        assert_eq!(shown.text, text);
        assert!(!shown.clipped);
        assert_eq!(
            shown.runs,
            [
                (0..3, style(TokenKind::Keyword, false)),
                (4..8, style(TokenKind::Plain, true)),
                (8..9, style(TokenKind::Number, true)),
                (9..10, style(TokenKind::Punctuation, false)),
            ]
        );
    }

    #[test]
    fn tabs_expand_and_ranges_follow_them() {
        let text = "\tfn x";
        let tokens = [token(1..3, TokenKind::Keyword)];
        let shown = visible(text, &tokens, &[], 0, 80);
        assert_eq!(shown.text, "    fn x");
        assert_eq!(shown.runs, [(4..6, style(TokenKind::Keyword, false))]);
    }

    #[test]
    fn only_the_columns_in_view_are_kept() {
        let text = "abcdefghij";
        let tokens = [token(2..6, TokenKind::String)];
        let shown = visible(text, &tokens, &[], 3, 4);
        assert_eq!(shown.text, "defg");
        assert_eq!(shown.runs, [(0..3, style(TokenKind::String, false))]);
        assert!(shown.clipped);
        // The last columns aren't clipped.
        assert!(!visible(text, &tokens, &[], 6, 4).clipped);
        // Past the end, nothing shows.
        assert_eq!(visible(text, &tokens, &[], 20, 4), Visible::default());
    }

    #[test]
    fn a_tab_cut_by_an_edge_shows_the_part_in_view() {
        let shown = visible("a\tb", &[], &[], 2, 2);
        assert_eq!(shown.text, "  ");
        assert!(shown.clipped);
        let shown = visible("a\tb", &[], &[], 3, 10);
        assert_eq!(shown.text, "  b");
    }

    #[test]
    fn multibyte_characters_keep_their_boundaries() {
        let text = "é = \"ü\"";
        // `"ü"` is bytes 5..9.
        let tokens = [token(5..9, TokenKind::String)];
        let shown = visible(text, &tokens, &ranges(&[(0, 2)]), 0, 80);
        assert_eq!(
            shown.runs,
            [
                (0..2, style(TokenKind::Plain, true)),
                (5..9, style(TokenKind::String, false)),
            ]
        );
        let cut = visible(text, &tokens, &[], 5, 1);
        assert_eq!(cut.text, "ü");
        assert_eq!(cut.runs, [(0..2, style(TokenKind::String, false))]);
    }

    #[test]
    fn columns_follow_tabs_and_multibyte_characters() {
        let text = "\tfn é()";
        assert_eq!(column_at(text, 0), 0);
        // After the tab: four columns in.
        assert_eq!(column_at(text, 1), 4);
        // `é` is two bytes, one column.
        assert_eq!(column_at(text, 4), 7);
        assert_eq!(column_at(text, 6), 8);
        assert_eq!(column_at(text, 99), columns(text));
    }

    #[test]
    fn a_pointer_puts_the_caret_before_the_character_under_it_until_past_its_bias() {
        let text = "ab\tc";
        // On `a`'s centre: before it; most of the way across: after it.
        assert_eq!(caret_offset(text, 0.5), 0);
        assert_eq!(caret_offset(text, 0.7), 1);
        // Inside the tab (columns 2..6): its start until 60% across, then after it.
        assert_eq!(caret_offset(text, 3.0), 2);
        assert_eq!(caret_offset(text, 4.5), 3);
        // Before the line and past its end.
        assert_eq!(caret_offset(text, -3.0), 0);
        assert_eq!(caret_offset(text, 40.0), text.len());
        // Multi-byte: `é` takes bytes 1..3.
        assert_eq!(caret_offset("aé", 1.2), 1);
        assert_eq!(caret_offset("aé", 1.8), 3);
    }

    #[test]
    fn the_character_under_a_column_is_found_past_the_end_too() {
        assert_eq!(char_at_column("a\tb", 0.2), Some(0..1));
        assert_eq!(char_at_column("a\tb", 3.9), Some(1..2));
        assert_eq!(char_at_column("a\tb", 5.5), Some(2..3));
        assert_eq!(char_at_column("a\tb", 50.), Some(2..3));
        assert_eq!(char_at_column("", 1.), None);
    }

    #[test]
    fn a_double_click_takes_the_run_of_its_class() {
        let text = "let foo_bar = a.b(é1);  // x";
        let word = |at: usize| {
            let end = at + text[at..].chars().next().unwrap().len_utf8();
            &text[word_at(text, at..end)]
        };
        assert_eq!(word(5), "foo_bar");
        assert_eq!(word(4), "foo_bar");
        assert_eq!(word(0), "let");
        assert_eq!(word(12), "=");
        assert_eq!(word(3), " ");
        // Identifiers keep multi-byte letters; punctuation runs together.
        assert_eq!(word(18), "é1");
        assert_eq!(word(21), ");");
        assert_eq!(word(26), "//");
        assert_eq!(word(24), "  ");
    }

    #[test]
    fn character_columns_count_characters_not_bytes() {
        let text = "\té = 1";
        assert_eq!(char_column(text, 0), 1);
        assert_eq!(char_column(text, 1), 2);
        assert_eq!(char_column(text, 3), 3);
        assert_eq!(char_offset(text, 1), Some(0));
        assert_eq!(char_offset(text, 3), Some(3));
        // The column after the last character is the line's end; past it, nothing.
        assert_eq!(char_offset(text, 7), Some(text.len()));
        assert_eq!(char_offset(text, 8), None);
        assert_eq!(char_offset(text, 0), None);
    }
}
