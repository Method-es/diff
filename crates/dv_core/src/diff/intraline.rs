//! Intraline (word-level) ranges for paired changed lines (docs/PLAN.md §6 step 3, salvaged per
//! PLAN §15).
//!
//! Within each run of changed lines, the i-th removed line pairs with the i-th added line, as on
//! GitHub; lines left over on either side stay unpaired. A pair's texts (line endings already
//! stripped) are split into word, whitespace and punctuation tokens, the token sequences are
//! diffed with imara-diff's histogram algorithm, and the changed token spans become byte ranges.
//! Pathological pairs are left unmarked rather than slowing the diff down: very long token
//! sequences ([`MAX_TOKEN_PRODUCT`]) and mostly rewritten lines ([`MAX_CHANGED_FRACTION`]).

use std::ops::Range;

use imara_diff::{Algorithm, Diff, InternedInput, TokenSource};

use super::{DiffLine, LineKind};

/// Above this product of the two token counts a pair is left unmarked, rather than paying for a
/// quadratic token diff of, say, a minified line.
pub const MAX_TOKEN_PRODUCT: usize = 10_000;

/// A pair where more than this fraction of either line's bytes changed reads as rewritten:
/// word-level marks would speckle nearly the whole line and hide that, so neither side is
/// marked (the row tint still shows the change).
pub const MAX_CHANGED_FRACTION: f32 = 0.7;

/// Fills [`DiffLine::words`] for every paired removed/added line in `lines`.
pub fn mark_intraline(lines: &mut [DiffLine]) {
    let mut start = 0;
    while start < lines.len() {
        if !lines[start].kind.is_change() {
            start += 1;
            continue;
        }
        let end = lines[start..]
            .iter()
            .position(|line| !line.kind.is_change())
            .map_or(lines.len(), |offset| start + offset);
        let block = start..end;
        let removed: Vec<usize> = block
            .clone()
            .filter(|&i| lines[i].kind == LineKind::Removed)
            .collect();
        let added: Vec<usize> = block
            .filter(|&i| lines[i].kind == LineKind::Added)
            .collect();
        for (&r, &a) in removed.iter().zip(&added) {
            let (removed_words, added_words) = diff_pair(&lines[r].text, &lines[a].text);
            lines[r].words = removed_words;
            lines[a].words = added_words;
        }
        start = end;
    }
}

/// The byte ranges that differ between a removed line and the added line paired with it: (the
/// removed line's, the added line's). Both are empty when the lines are identical, too long to
/// diff cheaply, or mostly rewritten.
pub fn diff_pair(removed: &str, added: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let removed_tokens = tokenize(removed);
    let added_tokens = tokenize(added);
    if removed_tokens.len().saturating_mul(added_tokens.len()) > MAX_TOKEN_PRODUCT {
        return (Vec::new(), Vec::new());
    }

    let removed_strs: Vec<&str> = removed_tokens.iter().map(|r| &removed[r.clone()]).collect();
    let added_strs: Vec<&str> = added_tokens.iter().map(|r| &added[r.clone()]).collect();
    let input = InternedInput::new(Tokens(&removed_strs), Tokens(&added_strs));
    let diff = Diff::compute(Algorithm::Histogram, &input);

    let mut removed_ranges = Vec::new();
    let mut added_ranges = Vec::new();
    for hunk in diff.hunks() {
        let span = |tokens: &[Range<usize>], range: Range<u32>| {
            tokens[range.start as usize].start..tokens[range.end as usize - 1].end
        };
        if !hunk.before.is_empty() {
            push_merged(&mut removed_ranges, span(&removed_tokens, hunk.before));
        }
        if !hunk.after.is_empty() {
            push_merged(&mut added_ranges, span(&added_tokens, hunk.after));
        }
    }

    // Trailing whitespace doesn't count towards a line's length, so padding can't sneak a
    // rewritten line under the threshold. A pair that differs only in trailing whitespace has
    // changed bytes past that length and is always left unmarked; the row tint still shows it.
    let rewritten = |ranges: &[Range<usize>], len: usize| {
        len > 0
            && ranges.iter().map(|r| r.len()).sum::<usize>() as f32 / len as f32
                > MAX_CHANGED_FRACTION
    };
    // Either side crossing the threshold unmarks both: a one-sided mark reads worse than none.
    if rewritten(&removed_ranges, removed.trim_end().len())
        || rewritten(&added_ranges, added.trim_end().len())
    {
        return (Vec::new(), Vec::new());
    }
    (removed_ranges, added_ranges)
}

/// Appends `range`, merging it into the previous one when they touch.
fn push_merged(ranges: &mut Vec<Range<usize>>, range: Range<usize>) {
    if let Some(last) = ranges.last_mut()
        && range.start <= last.end
    {
        last.end = last.end.max(range.end);
        return;
    }
    ranges.push(range);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Other,
}

fn classify(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

/// Splits `text` into byte ranges: runs of word characters, runs of whitespace, and one token per
/// other character (punctuation never runs together, so `((` is two tokens).
fn tokenize(text: &str) -> Vec<Range<usize>> {
    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        let class = classify(c);
        let mut end = start + c.len_utf8();
        if class != Class::Other {
            while let Some(&(_, next)) = chars.peek() {
                if classify(next) != class {
                    break;
                }
                end += next.len_utf8();
                chars.next();
            }
        }
        tokens.push(start..end);
    }
    tokens
}

/// A pre-split token list as imara-diff input.
struct Tokens<'a>(&'a [&'a str]);

impl<'a> TokenSource for Tokens<'a> {
    type Token = &'a str;
    type Tokenizer = std::iter::Copied<std::slice::Iter<'a, &'a str>>;

    fn tokenize(&self) -> Self::Tokenizer {
        self.0.iter().copied()
    }

    fn estimate_tokens(&self) -> u32 {
        u32::try_from(self.0.len()).unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One range as a word list (a `vec![a..b]` literal reads as a mistake to clippy).
    fn one(range: Range<usize>) -> Vec<Range<usize>> {
        vec![range]
    }

    fn line(kind: LineKind, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old_no: None,
            new_no: None,
            text: text.to_string(),
            words: Vec::new(),
        }
    }

    #[test]
    fn tokenize_groups_words_and_spaces_and_splits_punctuation() {
        let text = "let count = 5;";
        let tokens: Vec<&str> = tokenize(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(tokens, ["let", " ", "count", " ", "=", " ", "5", ";"]);
        assert_eq!(tokenize("(("), [0..1, 1..2]);
        assert!(tokenize("").is_empty());
        let text = "größe_x = ü;";
        let tokens: Vec<&str> = tokenize(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(tokens, ["größe_x", " ", "=", " ", "ü", ";"]);
    }

    #[test]
    fn identical_lines_have_no_ranges() {
        assert_eq!(diff_pair("same", "same"), (vec![], vec![]));
    }

    #[test]
    fn a_small_edit_marks_only_the_changed_token() {
        assert_eq!(
            diff_pair("let count = 5;", "let count = 6;"),
            (one(12..13), one(12..13))
        );
    }

    #[test]
    fn ranges_are_on_char_boundaries() {
        let (removed, added) = diff_pair("let größe = 1;", "let größe = 22;");
        for range in &removed {
            let _ = &"let größe = 1;"[range.clone()];
        }
        for range in &added {
            let _ = &"let größe = 22;"[range.clone()];
        }
        assert_eq!(added, one("let größe = ".len().."let größe = 22".len()));
    }

    #[test]
    fn a_huge_token_product_is_left_unmarked() {
        // 400 × 400 tokens is far past MAX_TOKEN_PRODUCT.
        let (removed, added) = diff_pair(&"x ".repeat(200), &"y ".repeat(200));
        assert!(removed.is_empty() && added.is_empty());
    }

    #[test]
    fn a_mostly_rewritten_pair_is_left_unmarked() {
        let (removed, added) = diff_pair(
            "let old_name = compute_thing(alpha);",
            "self.registry.insert(key, value);",
        );
        assert!(removed.is_empty() && added.is_empty());
        // One side crossing the threshold unmarks both.
        let (removed, added) = diff_pair("x", "a much longer replacement line entirely");
        assert!(removed.is_empty() && added.is_empty());
    }

    #[test]
    fn trailing_whitespace_does_not_dilute_the_fraction() {
        let padding = " ".repeat(30);
        let (removed, added) = diff_pair(&format!("abc{padding}"), &format!("xyz{padding}"));
        assert!(removed.is_empty() && added.is_empty());
    }

    #[test]
    fn a_moderate_edit_stays_marked() {
        let (removed, added) = diff_pair("prefix common alpha beta", "prefix common gamma delta");
        assert!(!removed.is_empty() && !added.is_empty());
    }

    #[test]
    fn pairs_removed_and_added_lines_positionally_within_each_block() {
        let mut lines = vec![
            line(LineKind::Context, "fn a() {"),
            line(LineKind::Removed, "    let x = 1;"),
            line(LineKind::Removed, "    let y = 2;"),
            line(LineKind::Added, "    let x = 10;"),
            line(LineKind::Context, "}"),
            line(LineKind::Removed, "    let z = 3;"),
            line(LineKind::Added, "    let z = 4;"),
            line(LineKind::Added, "    extra();"),
        ];
        mark_intraline(&mut lines);
        let words: Vec<_> = lines.iter().map(|line| line.words.clone()).collect();
        assert_eq!(
            words,
            [
                vec![],
                one(12..13),
                vec![],
                one(12..14),
                vec![],
                one(12..13),
                one(12..13),
                vec![],
            ]
        );
    }
}
