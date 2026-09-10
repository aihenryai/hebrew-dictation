//! Shared text helpers for Hebrew coming back from ASR.
//!
//! Both `punctuation` (spoken punctuation commands) and `lang_switch` (spoken
//! language-switch commands) have to look at the same transcript and ask "is
//! this token the word X?". They need identical normalization to agree, so it
//! lives here rather than being copied into each.

/// Strip Hebrew niqud/cantillation marks (U+0591–U+05C7 — points, dagesh,
/// rafe, shin/sin dots, cantillation). Base letters are U+05D0–U+05EA, a
/// disjoint range, so real content is never touched.
///
/// Confirmed necessary live (Henry, 2026-09-09): a short, isolated utterance
/// like "כתוב באנגלית" can come back from Deepgram FULLY NIQQUD
/// ("כְּתוֹב בַּאֲנָלִית") even though no other transcript that session showed
/// niqud — smart_format seems to reach for a "dictionary pronunciation"
/// rendering specifically when it has little surrounding context, which is
/// exactly the shape of a spoken command. Byte-for-byte matching against plain
/// text silently failed on every such segment.
pub(crate) fn strip_niqud(s: &str) -> String {
    s.chars()
        .filter(|c| !('\u{0591}'..='\u{05C7}').contains(c))
        .collect()
}

/// Punctuation that smart_format glues onto a word, and that we must look past
/// when asking whether a token IS a given word. Includes the Hebrew geresh and
/// gershayim, which Deepgram emits inside Hebrew text.
const GLUED_MARKS: &[char] = &[
    '.', ',', '!', '?', ':', ';', '"', '\'', '״', '׳', '(', ')', '…', '־', '-', '׃', '。',
];

/// A token as it appears for MATCHING: niqud stripped, glued punctuation
/// removed from both ends, lowercased (a no-op for Hebrew, load-bearing for the
/// English half of the language-switch lexicon).
pub(crate) fn normalize_token(token: &str) -> String {
    strip_niqud(token)
        .trim_matches(|c| GLUED_MARKS.contains(&c))
        .to_lowercase()
}

/// Byte spans of the whitespace-separated tokens of `s`, in order.
///
/// Spans rather than `split_whitespace` because callers need to rebuild the
/// original string minus a trailing run of tokens, preserving everything else
/// byte-for-byte.
pub(crate) fn token_spans(s: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in s.char_indices() {
        if c.is_whitespace() {
            if let Some(st) = start.take() {
                spans.push((st, i));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(st) = start {
        spans.push((st, s.len()));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_niqud_removes_points_but_keeps_every_base_letter() {
        assert_eq!(strip_niqud("כְּתוֹב בַּאֲנְגְּלִית"), "כתוב באנגלית");
        assert_eq!(strip_niqud("שלום"), "שלום", "plain text must pass through untouched");
    }

    #[test]
    fn normalize_token_strips_smart_format_punctuation_from_both_ends() {
        assert_eq!(normalize_token("נקודה."), "נקודה");
        assert_eq!(normalize_token("\"באנגלית\""), "באנגלית");
        assert_eq!(normalize_token("נקודה,"), "נקודה");
        assert_eq!(normalize_token("(סוגריים)"), "סוגריים");
    }

    #[test]
    fn normalize_token_lowercases_latin_so_the_english_lexicon_can_match() {
        assert_eq!(normalize_token("English!"), "english");
        assert_eq!(normalize_token("SWITCH"), "switch");
    }

    #[test]
    fn token_spans_round_trip_to_the_original_words() {
        let s = "  שלום   עולם יפה ";
        let words: Vec<&str> = token_spans(s).iter().map(|&(a, b)| &s[a..b]).collect();
        assert_eq!(words, vec!["שלום", "עולם", "יפה"]);
    }

    #[test]
    fn token_spans_handles_empty_and_whitespace_only() {
        assert!(token_spans("").is_empty());
        assert!(token_spans("   \n  ").is_empty());
    }

    #[test]
    fn token_spans_treats_newlines_as_separators() {
        let s = "שורה\nשנייה";
        assert_eq!(token_spans(s).len(), 2);
    }
}
