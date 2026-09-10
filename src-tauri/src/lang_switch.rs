//! Spoken language-switch commands: say "כתוב באנגלית" (or "switch to Hebrew")
//! mid-dictation and the next words are transcribed in the other language.
//!
//! Deepgram fixes `language` when the websocket opens, so a switch means
//! reconnecting — the frontend does that. This module only answers one
//! question: **is this final segment a switch command, and to which language?**
//!
//! # Why the old exact-match matcher failed
//!
//! It compared the whole trimmed segment against four Hebrew string literals.
//! Three things broke it in the field:
//!
//! 1. **Phrasing.** "תכתוב לי באנגלית", "עבור לאנגלית", "באנגלית בבקשה" all miss.
//! 2. **ASR damage.** Deepgram was captured mishearing "אנגלית" as "אנלית"
//!    (dropped ג). No amount of normalization fixes a wrong letter.
//! 3. **Direction.** There were no English triggers at all — the old test suite
//!    even asserted that "write in hebrew" must NOT match.
//!
//! And a miss is destructive: the unmatched command is typed into the user's
//! document as ordinary text.
//!
//! # Why this one can be loose without becoming trigger-happy
//!
//! The safety does not come from being strict about the phrase; it comes from
//! being strict about **every other token**. A segment is a command only if it
//! is at most `MAX_TOKENS` long and **every single token** is classifiable as a
//! verb, a language word, or a filler. One unknown word and the answer is No.
//! That is what keeps a real sentence which merely mentions writing Hebrew from
//! being swallowed — structurally, not by luck:
//!
//! | segment                              | first unknown token | result |
//! |--------------------------------------|---------------------|--------|
//! | אני אוהב לכתוב בעברית כל יום          | אני                 | None   |
//! | הוא אמר לי כתוב בעברית ואני כתבתי     | הוא                 | None   |
//! | כתוב בעברית ותשלח לי                  | ותשלח               | None   |
//!
//! Note the third: only four tokens, so a length limit alone would have let it
//! through. The whitelist is what does the work.
//!
//! Fuzzy matching is applied to the LANGUAGE words only, never to verbs or
//! fillers — a fuzzy verb would blow the whitelist open.

use crate::hebrew::{normalize_token, strip_niqud, token_spans};

/// A command longer than this is prose, not a command.
const MAX_TOKENS: usize = 5;

/// Hebrew "letters of service" that attach directly to a word (ב ל ה מ ו כ ש).
/// Stripped one at a time so "באנגלית", "לאנגלית" and "אנגלית" all resolve to
/// the same language word.
const PREFIX_LETTERS: &[char] = &['ב', 'ל', 'ה', 'מ', 'ו', 'כ', 'ש'];

/// Verbs that can carry a switch instruction, in both languages. Both lexicons
/// are always active regardless of the session language: a cross-script
/// accident that still comes back recognizable should still work.
const VERBS: &[&str] = &[
    // Hebrew
    // Imperatives only. The infinitive "לכתוב" is deliberately absent — it is
    // far more often prose ("רוצה לכתוב בעברית") than an instruction.
    "כתוב", "תכתוב", "כתבי", "עבור", "תעבור", "עברי", "החלף", "תחליף", "דבר", "תדבר",
    // English
    "write", "switch", "change", "speak", "type", "go", "talk", "dictate",
];

/// Words that may appear inside a command without carrying meaning.
const FILLERS: &[&str] = &[
    // Hebrew
    "לי", "בבקשה", "עכשיו", "את", "מעכשיו", "הלאה", "אנא", "כן",
    // English
    "in", "to", "please", "now", "me", "back", "lets", "let's", "over", "the", "into", "ok",
];

/// Language words. The first element of each pair is compared after prefix
/// stripping; `code` is what a match resolves to.
struct LangWord {
    word: &'static str,
    code: &'static str,
}

const LANG_WORDS: &[LangWord] = &[
    LangWord { word: "עברית", code: "he" },
    LangWord { word: "אנגלית", code: "en" },
    // Hebrew-letter transliterations — what nova-3-Hebrew actually produces
    // when someone says the English name of the language out loud.
    LangWord { word: "אנגליש", code: "en" },
    LangWord { word: "אינגליש", code: "en" },
    LangWord { word: "היברו", code: "he" },
    // English
    LangWord { word: "hebrew", code: "he" },
    LangWord { word: "english", code: "en" },
    LangWord { word: "ivrit", code: "he" },
    LangWord { word: "anglit", code: "en" },
];

enum Token {
    Verb,
    Filler,
    Lang(&'static str),
}

/// Detect a spoken language-switch command in one FINAL segment.
///
/// Returns the Deepgram language code to switch to, or `None` when the segment
/// is ordinary dictated content.
pub fn detect_language_switch(transcript: &str) -> Option<&'static str> {
    let stripped = strip_niqud(transcript);
    let spans = token_spans(&stripped);
    if spans.is_empty() || spans.len() > MAX_TOKENS {
        return None;
    }

    let mut target: Option<&'static str> = None;
    let mut saw_verb_or_filler = false;

    for &(a, b) in &spans {
        match classify(&normalize_token(&stripped[a..b]))? {
            Token::Verb | Token::Filler => saw_verb_or_filler = true,
            Token::Lang(code) => {
                // Two different languages in one breath is not an instruction.
                if target.is_some_and(|t| t != code) {
                    return None;
                }
                target = Some(code);
            }
        }
    }

    // A bare language word ("אנגלית") is far too easy to say by accident — it
    // is a normal noun. Require something that makes it an instruction.
    if !saw_verb_or_filler {
        return None;
    }

    target
}

/// `None` here means "not a word a command may contain", which aborts the whole
/// match via `?` in the caller. That is the safety property of this module.
fn classify(token: &str) -> Option<Token> {
    if token.is_empty() {
        return None;
    }
    if VERBS.contains(&token) {
        return Some(Token::Verb);
    }
    if FILLERS.contains(&token) {
        return Some(Token::Filler);
    }
    if let Some(code) = match_language_word(token) {
        return Some(Token::Lang(code));
    }
    None
}

/// A language word, allowing one attached Hebrew prefix letter and a small
/// edit distance to survive ASR damage.
fn match_language_word(token: &str) -> Option<&'static str> {
    for candidate in [token, strip_one_prefix(token)] {
        for lw in LANG_WORDS {
            if candidate == lw.word {
                return Some(lw.code);
            }
        }
    }
    // Fuzzy pass, only after every exact form failed. Language words only.
    for candidate in [token, strip_one_prefix(token)] {
        for lw in LANG_WORDS {
            if levenshtein(candidate, lw.word) <= fuzz_budget(lw.word) {
                return Some(lw.code);
            }
        }
    }
    None
}

/// Remove one leading letter of service, but never turn a word into a stub —
/// the remainder must still be long enough to be a language word.
fn strip_one_prefix(token: &str) -> &str {
    let mut chars = token.chars();
    match chars.next() {
        Some(c) if PREFIX_LETTERS.contains(&c) => {
            let rest = chars.as_str();
            if rest.chars().count() >= 4 {
                rest
            } else {
                token
            }
        }
        _ => token,
    }
}

/// How many character edits a language word may absorb.
///
/// The 6-character floor is load-bearing, not a round number: "עברית" is five
/// characters and sits one edit from "עברת" ("you passed"), a perfectly normal
/// Hebrew word. A budget of 1 there would let an ordinary sentence fragment
/// register as a language word. Six characters up covers the failure we
/// actually captured — "אנגלית" misheard as "אנלית" — with no such neighbour.
fn fuzz_budget(word: &str) -> usize {
    match word.chars().count() {
        n if n >= 8 => 2,
        n if n >= 6 => 1,
        _ => 0,
    }
}

/// Plain two-row Levenshtein. ~20 lines beats a dependency for this.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- The load-bearing negatives, carried over verbatim from the old
    // matcher. A sentence that merely MENTIONS writing Hebrew or English must
    // never be swallowed as a command and eaten from the user's document. ----

    #[test]
    fn never_matches_a_substring_of_a_real_sentence() {
        assert_eq!(detect_language_switch("אני אוהב לכתוב בעברית כל יום"), None);
        assert_eq!(detect_language_switch("הוא אמר לי כתוב בעברית ואני כתבתי"), None);
        assert_eq!(detect_language_switch("כתוב בעברית ותשלח לי"), None);
    }

    #[test]
    fn ignores_unrelated_text() {
        assert_eq!(detect_language_switch(""), None);
        assert_eq!(detect_language_switch("   "), None);
        assert_eq!(detect_language_switch("שלום עולם"), None);
        assert_eq!(detect_language_switch("this is a normal english sentence"), None);
    }

    /// A bare language word is a normal noun — "אנגלית" alone must not switch.
    #[test]
    fn a_bare_language_word_is_not_an_instruction() {
        assert_eq!(detect_language_switch("אנגלית"), None);
        assert_eq!(detect_language_switch("עברית"), None);
        assert_eq!(detect_language_switch("english"), None);
    }

    /// "עברת" ("you passed") is one edit from "עברית". The 6-character fuzz
    /// floor is what keeps it out — pinned here so lowering that floor breaks
    /// a test instead of quietly eating a word from someone's document.
    #[test]
    fn a_near_miss_hebrew_word_in_prose_never_switches() {
        assert_eq!(detect_language_switch("עברת"), None);
        assert_eq!(detect_language_switch("עברת את"), None);
        assert_eq!(detect_language_switch("עברת את המבחן"), None);
        assert_eq!(detect_language_switch("כבר עברת שלוש פעמים"), None);
        assert_eq!(match_language_word("עברת"), None);
    }

    #[test]
    fn two_languages_in_one_breath_is_not_an_instruction() {
        assert_eq!(detect_language_switch("כתוב בעברית ובאנגלית"), None);
    }

    // ---- Everything the old matcher already supported still works ----

    #[test]
    fn the_original_four_triggers_still_match() {
        assert_eq!(detect_language_switch("כתוב בעברית"), Some("he"));
        assert_eq!(detect_language_switch("תכתוב בעברית"), Some("he"));
        assert_eq!(detect_language_switch("כתוב באנגלית"), Some("en"));
        assert_eq!(detect_language_switch("תכתוב באנגלית"), Some("en"));
    }

    #[test]
    fn tolerates_smart_format_punctuation_and_whitespace() {
        assert_eq!(detect_language_switch("כתוב בעברית."), Some("he"));
        assert_eq!(detect_language_switch("  כתוב בעברית  "), Some("he"));
        assert_eq!(detect_language_switch("כתוב באנגלית!"), Some("en"));
        assert_eq!(detect_language_switch("כתוב, באנגלית"), Some("en"));
    }

    #[test]
    fn strips_the_niqud_deepgram_sometimes_adds() {
        assert_eq!(detect_language_switch("כְּתוֹב בַּאֲנְגְּלִית."), Some("en"));
        assert_eq!(detect_language_switch("כְּתוֹב בְּעִבְרִית"), Some("he"));
    }

    // ---- The three failures that motivated the rewrite ----

    #[test]
    fn free_phrasings_now_work() {
        assert_eq!(detect_language_switch("תכתוב לי באנגלית"), Some("en"));
        assert_eq!(detect_language_switch("עבור לאנגלית"), Some("en"));
        assert_eq!(detect_language_switch("באנגלית בבקשה"), Some("en"));
        assert_eq!(detect_language_switch("תעבור עכשיו לעברית"), Some("he"));
        assert_eq!(detect_language_switch("החלף לעברית"), Some("he"));
    }

    #[test]
    fn english_commands_work_in_both_directions() {
        assert_eq!(detect_language_switch("write in Hebrew"), Some("he"));
        assert_eq!(detect_language_switch("switch to English"), Some("en"));
        assert_eq!(detect_language_switch("in Hebrew please"), Some("he"));
        assert_eq!(detect_language_switch("speak English now"), Some("en"));
    }

    /// The exact captured failure: Deepgram heard "אנלית" (dropped ג).
    #[test]
    fn survives_the_captured_asr_mishearing() {
        assert_eq!(detect_language_switch("כתוב באנלית"), Some("en"));
        assert_eq!(detect_language_switch("כְּתוֹב בַּאֲנָלִית."), Some("en"));
    }

    #[test]
    fn a_segment_longer_than_the_cap_is_prose() {
        assert_eq!(
            detect_language_switch("כתוב לי בבקשה עכשיו מעכשיו והלאה באנגלית"),
            None,
            "six whitelisted tokens is still past the cap"
        );
    }

    // ---- units ----

    #[test]
    fn levenshtein_is_correct_on_the_cases_this_relies_on() {
        assert_eq!(levenshtein("אנלית", "אנגלית"), 1);
        assert_eq!(levenshtein("עברית", "עברת"), 1);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
    }

    #[test]
    fn prefix_stripping_never_produces_a_stub() {
        assert_eq!(strip_one_prefix("באנגלית"), "אנגלית");
        assert_eq!(strip_one_prefix("בית"), "בית", "too short to strip");
        assert_eq!(strip_one_prefix("english"), "english");
    }

    #[test]
    fn fuzz_budget_never_lets_a_short_word_drift() {
        assert_eq!(fuzz_budget("עברית"), 0, "five chars — see the doc comment");
        assert_eq!(fuzz_budget("ivrit"), 0);
        assert_eq!(fuzz_budget("abc"), 0);
        assert_eq!(fuzz_budget("אנגלית"), 1);
        assert_eq!(fuzz_budget("english"), 1);
    }
}
