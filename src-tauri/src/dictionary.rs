//! Personal dictionary: words the user wants recognised and written exactly.
//!
//! One line per entry, stored in settings as plain strings:
//!   `Kubernetes`            - a term to favour while listening
//!   `קוברנטס => Kubernetes`  - whenever the engine writes the left side, write the right side
//!
//! Held in a process-wide list so every transcription path (streaming, file,
//! local whisper) reads the same dictionary without threading it through each call.
//! Cloud engines get the words as Deepgram `keyterm` hints; local whisper gets
//! them as its initial prompt; replacements run on the finished text everywhere.

use std::sync::RwLock;

const MAX_ENTRIES: usize = 100;
const MAX_TERM_CHARS: usize = 60;
/// Deepgram rejects keyterm lists that are too long, so only the first few go in the URL.
const MAX_KEYTERMS: usize = 40;

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    heard: Option<String>,
    written: String,
}

static DICTIONARY: RwLock<Vec<Entry>> = RwLock::new(Vec::new());

fn parse_line(line: &str) -> Option<Entry> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let (heard, written) = match line.split_once("=>") {
        Some((l, r)) => (Some(l.trim().to_string()), r.trim().to_string()),
        None => (None, line.to_string()),
    };
    if written.is_empty() || written.chars().count() > MAX_TERM_CHARS {
        return None;
    }
    let heard = heard.filter(|h| !h.is_empty() && h.chars().count() <= MAX_TERM_CHARS);
    Some(Entry { heard, written })
}

/// Replace the active dictionary. Blank, oversized and surplus lines are dropped.
pub fn set(lines: &[String]) {
    let entries: Vec<Entry> = lines.iter().filter_map(|l| parse_line(l)).take(MAX_ENTRIES).collect();
    if let Ok(mut d) = DICTIONARY.write() {
        *d = entries;
    }
}

/// Lines cleaned the same way `set` cleans them, for saving back to settings.
pub fn normalize(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|l| parse_line(l))
        .take(MAX_ENTRIES)
        .map(|e| match e.heard {
            Some(h) => format!("{} => {}", h, e.written),
            None => e.written,
        })
        .collect()
}

fn terms() -> Vec<String> {
    let Ok(d) = DICTIONARY.read() else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    for e in d.iter() {
        if !out.contains(&e.written) {
            out.push(e.written.clone());
        }
    }
    out
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*byte as char),
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

/// `&keyterm=...` query parameters for Deepgram Nova-3, or "" when the dictionary is empty.
pub fn keyterm_params() -> String {
    let mut out = String::new();
    for t in terms().into_iter().take(MAX_KEYTERMS) {
        out.push_str("&keyterm=");
        out.push_str(&percent_encode(&t));
    }
    out
}

/// Initial prompt for local whisper: the words, comma separated. None when empty.
pub fn whisper_prompt() -> Option<String> {
    let t = terms();
    if t.is_empty() {
        None
    } else {
        Some(t.join(", "))
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Case-insensitive replace of `needle` where it stands as a whole word.
fn replace_word(text: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let pat: Vec<char> = needle.chars().collect();
    let eq = |a: char, b: char| a == b || a.to_lowercase().eq(b.to_lowercase());
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let fits = i + pat.len() <= chars.len() && pat.iter().enumerate().all(|(k, p)| eq(chars[i + k], *p));
        if fits {
            let before_ok = i == 0 || !is_word_char(chars[i - 1]);
            let after_ok = i + pat.len() == chars.len() || !is_word_char(chars[i + pat.len()]);
            if before_ok && after_ok {
                out.push_str(with);
                i += pat.len();
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Apply the user's "heard => written" replacements to finished text.
pub fn apply(text: &str) -> String {
    let Ok(d) = DICTIONARY.read() else { return text.to_string() };
    let mut out = text.to_string();
    for e in d.iter() {
        if let Some(h) = &e.heard {
            out = replace_word(&out, h, &e.written);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_replacement_lines() {
        assert_eq!(parse_line("  Kubernetes "), Some(Entry { heard: None, written: "Kubernetes".into() }));
        assert_eq!(
            parse_line("קוברנטס=>Kubernetes"),
            Some(Entry { heard: Some("קוברנטס".into()), written: "Kubernetes".into() })
        );
        assert_eq!(parse_line("   "), None);
        assert_eq!(parse_line("abc =>"), None);
    }

    #[test]
    fn replaces_whole_words_only() {
        assert_eq!(replace_word("אני עובד עם קוברנטס היום", "קוברנטס", "Kubernetes"), "אני עובד עם Kubernetes היום");
        assert_eq!(replace_word("cat concat Cat.", "cat", "dog"), "dog concat dog.");
    }

    #[test]
    fn encodes_hebrew_keyterms() {
        assert_eq!(percent_encode("שלום"), "%D7%A9%D7%9C%D7%95%D7%9D");
    }

    #[test]
    fn normalize_drops_blanks_and_tidies() {
        let n = normalize(&["  a=>b ".to_string(), "".to_string(), "term".to_string()]);
        assert_eq!(n, vec!["a => b".to_string(), "term".to_string()]);
    }
}

#[cfg(test)]
mod live_tests {
    /// Real-hardware check, run by hand: `cargo test --lib live_mic -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_mic_reports_device_and_level() {
        let mut rec = crate::audio::AudioRecorder::new();
        rec.set_vad_enabled(false);
        rec.set_max_recording_secs(10.0);
        rec.start_recording().expect("start");
        std::thread::sleep(std::time::Duration::from_millis(2000));
        let samples = rec.stop_recording().expect("stop");
        println!("samples={} peak={}", samples.len(), crate::audio::peak_amplitude(&samples));
        assert!(!samples.is_empty());
    }
}
