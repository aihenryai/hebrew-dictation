//! Spoken punctuation for Hebrew dictation: saying "נקודה" types "." instead of
//! the word.
//!
//! # Why this is ours to build
//!
//! Deepgram has exactly this feature — `dictation=true` — and it is **English
//! only** (verified against the API docs, 2026-09-10). With `language=he` the
//! word comes back as a word, which is precisely what the user reported.
//!
//! # The false-positive problem, which is the whole design
//!
//! "נקודה" is an extremely common Hebrew NOUN ("point"). Henry's own dictation
//! reporting this bug literally opens with "ועוד נקודה, לא מספיק ברור..." —
//! "and another point, it isn't clear enough". A naive find-and-replace would
//! corrupt that sentence. So this module is deliberately conservative:
//!
//! * **Rule A — terminal position only.** A command is recognized only as the
//!   final run of tokens in a FINAL segment. Nothing mid-sentence is touched.
//!   That alone disqualifies "ועוד נקודה, לא מספיק ברור..." — six more words
//!   follow, so the word is never even a candidate.
//! * **Rule B — exact token equality** after niqud-stripping and removal of
//!   glued punctuation. No prefixes, no plurals: "נקודת מפנה", "הנקודה" and
//!   "שתי נקודות" all fail to match.
//! * **Rule C — noun-context blocklist** for ambiguous commands. Covers what
//!   Rule A cannot: a sentence that genuinely ENDS with the noun, e.g.
//!   "אני רוצה להוסיף עוד נקודה".
//! * **Rule D — longest match first**, so "נקודה פסיק" wins over "נקודה".
//! * **Rule E — iterate**, so "סוגריים סגורים נקודה" becomes ")." .
//! * **Rule F — replace smart_format's guess.** The user SAID which mark they
//!   want; whatever Deepgram inferred and glued on is superseded, so
//!   "להגיד נקודה." can never become "להגיד..".
//!
//! A missed command types a word the user must delete. A false positive
//! silently eats a word they meant. The second is worse, and every rule above
//! trades recall for that.

use crate::hebrew::{normalize_token, token_spans};

/// A spoken punctuation command and the mark it produces.
struct Command {
    /// Normalized tokens, in order. 1 or 2 entries.
    spoken: &'static [&'static str],
    mark: &'static str,
    /// True when the phrase is also a normal Hebrew noun phrase, so Rule C's
    /// blocklist must be consulted before matching it.
    ambiguous: bool,
}

/// Deliberately small. Every entry is a phrase with no plausible reading other
/// than "insert this mark" once Rules A-C have applied.
///
/// **Not included, on purpose:** "מרכאות" — one spoken word would produce a
/// single quote character while users almost always mean a pair, and there is
/// no way to know which side they are on. Half a feature is worse than none.
const COMMANDS: &[Command] = &[
    // Two-token first — Rule D depends on this ordering being checked by
    // length, and the table is sorted longest-first for readability.
    Command { spoken: &["נקודה", "פסיק"], mark: ";", ambiguous: false },
    Command { spoken: &["סימן", "שאלה"], mark: "?", ambiguous: true },
    Command { spoken: &["סימן", "קריאה"], mark: "!", ambiguous: true },
    Command { spoken: &["שורה", "חדשה"], mark: "\n", ambiguous: false },
    Command { spoken: &["פסקה", "חדשה"], mark: "\n\n", ambiguous: false },
    Command { spoken: &["שלוש", "נקודות"], mark: "...", ambiguous: false },
    Command { spoken: &["פתח", "סוגריים"], mark: "(", ambiguous: false },
    Command { spoken: &["סוגריים", "פתוחים"], mark: "(", ambiguous: false },
    Command { spoken: &["סגור", "סוגריים"], mark: ")", ambiguous: false },
    Command { spoken: &["סוגריים", "סגורים"], mark: ")", ambiguous: false },
    // One-token.
    Command { spoken: &["נקודה"], mark: ".", ambiguous: true },
    // Niqqud spelling נְקֻדָּה uses qubuts instead of the letter vav.
    // Stripping its marks therefore produces נקדה, not נקודה.
    Command { spoken: &["נקדה"], mark: ".", ambiguous: true },
    Command { spoken: &["פסיק"], mark: ",", ambiguous: false },
    Command { spoken: &["נקודתיים"], mark: ":", ambiguous: false },
    Command { spoken: &["מקף"], mark: "-", ambiguous: true },
];

/// Words that turn a following ambiguous command word back into a plain noun.
/// Quantifiers, demonstratives and determiners — the things that can only
/// precede a noun, never a dictation command.
const NOUN_CONTEXT: &[&str] = &[
    "עוד", "ועוד", "כל", "בכל", "איזו", "איזה", "אותה", "אותו", "זו", "זאת", "הרבה", "כמה", "אף",
    "שום", "אותן", "שתי", "שלוש", "ארבע", "חמש", "אחת", "עם", "בלי", "ללא",
];

/// Marks smart_format may have glued onto the end of the remaining text, and
/// that Rule F drops in favour of the mark the user actually asked for.
const SMART_FORMAT_MARKS: &[char] = &['.', ',', '!', '?', ':', ';', '…'];

/// Longest command, in tokens. Bounds the trailing window we inspect.
const MAX_COMMAND_TOKENS: usize = 2;

/// Guard against a pathological segment of nothing but commands.
const MAX_ITERATIONS: usize = 4;

/// Marks that must sit flush against the preceding text with no space.
const HUGS_LEFT: &[char] = &['.', ',', '?', '!', ':', ';', ')', '…', '\n'];

/// Resolve trailing spoken-punctuation commands in one FINAL segment.
///
/// Hebrew only — for any other language the input is returned unchanged (the
/// English path gets Deepgram's own `dictation=true` instead). The result is
/// what should be BOTH typed and accumulated into the transcript, so the screen,
/// the history and the local API can never disagree about what was dictated.
pub fn apply_spoken_punctuation(segment: &str, lang: &str) -> String {
    if lang != "he" {
        return segment.to_string();
    }

    let mut rest = segment.trim_end().to_string();
    // Collected right-to-left as we peel commands off the end.
    let mut marks: Vec<&'static str> = Vec::new();

    for _ in 0..MAX_ITERATIONS {
        match take_trailing_command(&rest) {
            Some((remaining, mark)) => {
                rest = remaining;
                marks.push(mark);
            }
            None => break,
        }
    }

    if marks.is_empty() {
        return segment.to_string();
    }

    // Rule F.
    let mut out = rest
        .trim_end()
        .trim_end_matches(SMART_FORMAT_MARKS)
        .trim_end()
        .to_string();
    for mark in marks.iter().rev() {
        out.push_str(mark);
    }
    out
}

/// If `text` ends with a punctuation command, return the text without it plus
/// the mark it produces.
fn take_trailing_command(text: &str) -> Option<(String, &'static str)> {
    let spans = token_spans(text);
    if spans.is_empty() {
        return None;
    }

    // Rule D: longest first.
    for n in (1..=MAX_COMMAND_TOKENS.min(spans.len())).rev() {
        let tail = &spans[spans.len() - n..];
        let tokens: Vec<String> = tail
            .iter()
            .map(|&(a, b)| normalize_token(&text[a..b]))
            .collect();

        let Some(cmd) = COMMANDS
            .iter()
            .find(|c| c.spoken.len() == n && c.spoken.iter().zip(&tokens).all(|(w, t)| *w == t))
        else {
            continue;
        };

        // Rule C. Once a phrase matched at THIS length, a blocked match stops
        // the search entirely rather than falling through to a shorter one —
        // "עוד נקודה" must not become "עוד" + "." via the 1-token entry.
        if cmd.ambiguous && spans.len() > n {
            let (a, b) = spans[spans.len() - n - 1];
            if NOUN_CONTEXT.contains(&normalize_token(&text[a..b]).as_str()) {
                return None;
            }
        }

        let cut = tail[0].0;
        return Some((text[..cut].to_string(), cmd.mark));
    }

    None
}

/// The separator to type before `payload`, given the last character this app
/// typed (`None` = nothing yet).
///
/// The streaming path used to append a trailing space to every segment. A
/// trailing space cannot know that the next thing typed will be a "." that must
/// hug the word before it — so the separator moved to the front, where it can.
/// This also removes the stray space every dictation used to end with.
pub fn leading_separator(last_injected: Option<char>, payload: &str) -> &'static str {
    match (last_injected, payload.chars().next()) {
        (_, None) => "",
        (_, Some(c)) if HUGS_LEFT.contains(&c) => "",
        (None, _) => "",
        (Some('\n'), _) | (Some('('), _) => "",
        (Some(c), _) if c.is_whitespace() => "",
        _ => " ",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn he(s: &str) -> String {
        apply_spoken_punctuation(s, "he")
    }

    // ---- Rule A / B / C: the false positives that must never fire ----

    /// Henry's actual dictation reporting this bug. "נקודה" is a NOUN here.
    #[test]
    fn henrys_real_sentence_is_left_completely_alone() {
        let s = "ועוד נקודה, לא מספיק ברור מה הכפתורים של בחירת שפות עושים עכשיו";
        assert_eq!(he(s), s);
    }

    #[test]
    fn a_sentence_ending_in_the_noun_is_blocked_by_the_context_list() {
        // Rule A can't help here — the noun IS the last token. Rule C does.
        assert_eq!(he("אני רוצה להוסיף עוד נקודה"), "אני רוצה להוסיף עוד נקודה");
        assert_eq!(he("יש לי עוד נקודה"), "יש לי עוד נקודה");
        assert_eq!(he("זו נקודה"), "זו נקודה");
        assert_eq!(he("כמה נקודה"), "כמה נקודה");
    }

    #[test]
    fn inflected_and_plural_forms_never_match() {
        assert_eq!(he("נקודת מפנה"), "נקודת מפנה");
        assert_eq!(he("הנקודה"), "הנקודה");
        assert_eq!(he("שתי נקודות"), "שתי נקודות");
        assert_eq!(he("בנקודה"), "בנקודה");
    }

    #[test]
    fn a_question_mark_noun_phrase_is_blocked_too() {
        assert_eq!(he("יש כאן עוד סימן שאלה"), "יש כאן עוד סימן שאלה");
    }

    #[test]
    fn a_command_word_in_the_middle_is_never_touched() {
        let s = "אמרתי נקודה ואז המשכתי לדבר";
        assert_eq!(he(s), s);
    }

    // ---- Positives ----

    #[test]
    fn a_terminal_command_becomes_its_mark() {
        assert_eq!(he("זה מה שרציתי להגיד נקודה"), "זה מה שרציתי להגיד.");
        assert_eq!(he("אתה בא פסיק"), "אתה בא,");
        assert_eq!(he("מה השעה סימן שאלה"), "מה השעה?");
        assert_eq!(he("תפסיק סימן קריאה"), "תפסיק!");
        assert_eq!(he("הרשימה נקודתיים"), "הרשימה:");
    }

    #[test]
    fn a_command_that_is_the_whole_segment_becomes_just_the_mark() {
        assert_eq!(he("נקודה"), ".");
        assert_eq!(he("סימן שאלה"), "?");
        assert_eq!(he("שורה חדשה"), "\n");
        assert_eq!(he("פסקה חדשה"), "\n\n");
    }

    #[test]
    fn longest_match_wins_over_its_own_prefix_and_suffix() {
        // Rule D — "נקודה פסיק" must not resolve as "נקודה" + a stray "פסיק".
        assert_eq!(he("המשפט הראשון נקודה פסיק"), "המשפט הראשון;");
        assert_eq!(he("שלוש נקודות"), "...");
    }

    #[test]
    fn commands_stack_right_to_left() {
        // Rule E.
        assert_eq!(he("סוגריים סגורים נקודה"), ").");
        assert_eq!(he("הערה סגור סוגריים נקודה"), "הערה).");
    }

    #[test]
    fn brackets_both_phrasings_work() {
        assert_eq!(he("פתח סוגריים"), "(");
        assert_eq!(he("סוגריים פתוחים"), "(");
        assert_eq!(he("סגור סוגריים"), ")");
        assert_eq!(he("סוגריים סגורים"), ")");
    }

    // ---- Rule F: smart_format's own mark must not double up ----

    #[test]
    fn smart_formats_guess_is_replaced_not_appended() {
        assert_eq!(he("להגיד נקודה."), "להגיד.");
        assert_eq!(he("להגיד. נקודה"), "להגיד.");
        assert_eq!(he("נקודה."), ".");
        assert_eq!(he("מה השעה, סימן שאלה"), "מה השעה?");
    }

    // ---- niqud: the v2.13.6 failure class, applied to this feature ----

    #[test]
    fn a_fully_niqqud_command_still_matches() {
        assert_eq!(he("כותב נְקֻדָּה"), "כותב.");
        assert_eq!(he("נְקֻדָּה"), ".");
        assert_eq!(he("עוד נְקֻדָּה"), "עוד נְקֻדָּה");
    }

    // ---- language gate ----

    #[test]
    fn non_hebrew_is_returned_untouched() {
        let s = "this is a period";
        assert_eq!(apply_spoken_punctuation(s, "en"), s);
        assert_eq!(apply_spoken_punctuation("אמרתי נקודה", "en"), "אמרתי נקודה");
    }

    #[test]
    fn empty_and_whitespace_are_safe() {
        assert_eq!(he(""), "");
        assert_eq!(he("   "), "   ");
    }

    #[test]
    fn a_segment_of_nothing_but_commands_stops_at_the_iteration_cap() {
        // MAX_ITERATIONS guard — must terminate, and must stop peeling rather
        // than recursing until the segment is gone.
        assert_eq!(he("נקודה נקודה נקודה נקודה נקודה נקודה"), "נקודה נקודה....");
    }

    // ---- leading_separator ----

    #[test]
    fn first_injection_of_the_process_gets_no_leading_space() {
        assert_eq!(leading_separator(None, "שלום"), "");
    }

    #[test]
    fn a_following_word_gets_one_separating_space() {
        assert_eq!(leading_separator(Some('ם'), "עולם"), " ");
    }

    #[test]
    fn a_hugging_mark_never_gets_a_leading_space() {
        for payload in [".", ",", "?", "!", ":", ";", ")", "\n"] {
            assert_eq!(leading_separator(Some('ם'), payload), "", "payload {:?}", payload);
        }
        assert_eq!(leading_separator(Some('ם'), "..."), "");
    }

    #[test]
    fn nothing_follows_a_newline_or_an_open_bracket_with_a_space() {
        assert_eq!(leading_separator(Some('\n'), "שלום"), "");
        assert_eq!(leading_separator(Some('('), "שלום"), "");
    }

    #[test]
    fn an_existing_trailing_space_is_not_doubled() {
        assert_eq!(leading_separator(Some(' '), "שלום"), "");
    }

    #[test]
    fn an_empty_payload_never_produces_a_stray_space() {
        assert_eq!(leading_separator(Some('ם'), ""), "");
    }
}
