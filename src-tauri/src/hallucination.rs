//! Clean the stock output Whisper produces on silence and noise.
//!
//! Whisper was trained on subtitled video, so given silence or a muddy signal
//! it "hears" closing lines ("תודה רבה.", "תודה שצפיתם") and falls into loops
//! ("תודה רבה. תודה רבה. תודה רבה.", "אהההההההה"). A Mac user's history was full
//! of exactly this (09.11.2026). Two passes:
//! 1. Always: collapse a sentence repeated back to back, and a letter stretched
//!    over 4+ times. Real speech never does either.
//! 2. Only when the audio held under a second of speech: drop a result that is
//!    nothing but stock phrases, because then it was not said.

const STOCK_PHRASES: &[&str] = &[
    "תודה רבה",
    "תודה רבה לכם",
    "תודה רבה לך",
    "תודה",
    "תודה שצפיתם",
    "תודה על הצפייה",
    "תודה על ההקשבה",
    "כתוביות",
    "תרגום",
    "שלום",
    "ביי",
    "thank you",
    "thanks for watching",
    "thank you for watching",
    "you",
];

/// Sentences with their ending punctuation ("שלום. מה נשמע?" -> ["שלום.", " מה נשמע?"]).
fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if matches!(c, '.' | '!' | '?' | '…') {
            let end = i + c.len_utf8();
            out.push(&text[start..end]);
            start = end;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

fn key(sentence: &str) -> String {
    sentence
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// "אהההההה" -> "אה": any character repeated 4+ times in a row becomes one.
fn collapse_stretched_letters(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut j = i;
        while j < chars.len() && chars[j] == c {
            j += 1;
        }
        let run = j - i;
        if run >= 4 && c.is_alphabetic() {
            out.push(c);
        } else {
            out.extend(std::iter::repeat(c).take(run));
        }
        i = j;
    }
    out
}

/// Drop a sentence that repeats the one right before it.
fn collapse_repeated_sentences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_key = String::new();
    for s in sentences(text) {
        let k = key(s);
        if k.is_empty() {
            out.push_str(s);
            continue;
        }
        if k == last_key {
            continue;
        }
        last_key = k;
        out.push_str(s);
    }
    out
}

fn only_stock_phrases(text: &str) -> bool {
    let parts: Vec<String> = sentences(text)
        .into_iter()
        .flat_map(|s| s.split(','))
        .map(key)
        .filter(|k| !k.is_empty())
        .collect();
    !parts.is_empty() && parts.iter().all(|p| STOCK_PHRASES.contains(&p.as_str()))
}

/// Clean a batch transcript. `speech_secs` is how much of the audio was speech
/// (from `audio::prepare_for_asr`).
pub fn clean(text: &str, speech_secs: f32) -> String {
    let cleaned = collapse_repeated_sentences(&collapse_stretched_letters(text));
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if speech_secs < 1.0 && only_stock_phrases(&cleaned) {
        return String::new();
    }
    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_thanks_loop_collapses_to_one() {
        assert_eq!(clean("תודה רבה. תודה רבה. תודה רבה.", 5.0), "תודה רבה.");
    }

    #[test]
    fn stretched_filler_shrinks() {
        assert_eq!(
            clean("אהההההההההההההה ומי שמחברת את העת", 5.0),
            "אה ומי שמחברת את העת"
        );
    }

    #[test]
    fn stock_phrase_on_near_silence_is_dropped() {
        assert_eq!(clean("תודה רבה. תודה רבה.", 0.3), "");
        assert_eq!(clean("תודה שצפיתם!", 0.0), "");
    }

    #[test]
    fn real_thanks_with_real_speech_stays() {
        assert_eq!(clean("תודה רבה.", 2.5), "תודה רבה.");
    }

    #[test]
    fn normal_dictation_is_untouched() {
        let t = "טוב, אז אנחנו כותבים בתוך החלון הפעיל. אני רואה שזה עובד.";
        assert_eq!(clean(t, 4.0), t);
    }

    #[test]
    fn non_adjacent_repeats_and_numbers_stay() {
        let t = "כן. לא. כן. 1000 שקל.";
        assert_eq!(clean(t, 3.0), t);
    }
}
