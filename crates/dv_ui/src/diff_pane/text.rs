//! A code line as it paints (DESIGN §4.1): tabs as four spaces, its syntax kinds and intraline
//! words merged into one list of styled runs, and only the columns in view, so a long line (a
//! minified file's) costs no more to draw than a short one.
//!
//! Columns count characters, which is right for the monospaced code font; a tab takes
//! [`TAB_WIDTH`]. Every range here is a byte range on character boundaries.

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
}

/// The columns `text` takes.
pub fn columns(text: &str) -> usize {
    text.chars()
        .map(|ch| if ch == '\t' { TAB_WIDTH } else { 1 })
        .sum()
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
}
