//! SRT subtitle export — timed-segment chunking and SRT text rendering.
//! Pure, dependency-free functions; no Tauri/file I/O here (see lib.rs
//! `export_srt` for the file-writing command).

/// One subtitle cue: text plus its start/end time within the source audio,
/// in milliseconds. Serialized across the Tauri IPC boundary in both
/// directions — `transcribe_file` returns these to the frontend, and
/// `export_srt` receives them back for writing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct TimedSegment {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
    /// Diarization speaker index (0-based, from Deepgram `diarize=true`).
    /// `None` when diarization is off or unavailable (local whisper never
    /// sets it). `#[serde(default)]` so cues produced by routes that don't
    /// set it — or serialized before this field existed — round-trip through
    /// the frontend and back into `export_srt` without a missing-field error.
    #[serde(default)]
    pub speaker: Option<u32>,
}

/// A single transcribed word with its timing, as reported by Deepgram's
/// `words[]` array (seconds in the API, converted to ms by the caller).
#[derive(Debug, Clone)]
pub struct TimedWord {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
    /// Diarization speaker index for this word (0-based), when Deepgram's
    /// `diarize=true` is active; `None` otherwise.
    pub speaker: Option<u32>,
}

/// Target cue size (see spec's "Cue-length parity between routes" note —
/// whisper approximates the same readability goal via a character cap
/// instead, these constants are Deepgram-side only).
pub const SRT_MAX_WORDS_PER_CUE: usize = 10;
pub const SRT_MAX_MS_PER_CUE: u64 = 4000;

/// Bucket words into short subtitle cues: accumulate words into the current
/// cue until either `max_words` is reached or adding the next word would
/// push the cue's span past `max_ms`, then flush and start a new cue. A
/// single word whose own span already exceeds `max_ms` still ships alone
/// (content is never dropped).
pub fn chunk_words_to_cues(words: &[TimedWord], max_words: usize, max_ms: u64) -> Vec<TimedSegment> {
    let mut cues = Vec::new();
    let mut current: Vec<&TimedWord> = Vec::new();

    for w in words {
        if !current.is_empty() {
            let span = w.end_ms.saturating_sub(current[0].start_ms);
            // A cue belongs to one speaker: force a flush when the speaker
            // changes. With diarization off every word is `None`, so
            // `None != None` is false and this never triggers — behavior
            // identical to before.
            let speaker_changed = w.speaker != current[0].speaker;
            if current.len() >= max_words || span > max_ms || speaker_changed {
                cues.push(flush_cue(&current));
                current.clear();
            }
        }
        current.push(w);
    }
    if !current.is_empty() {
        cues.push(flush_cue(&current));
    }
    cues
}

fn flush_cue(words: &[&TimedWord]) -> TimedSegment {
    TimedSegment {
        text: words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "),
        start_ms: words.first().map(|w| w.start_ms).unwrap_or(0),
        end_ms: words.last().map(|w| w.end_ms).unwrap_or(0),
        // All words in a cue share a speaker (chunk_words_to_cues splits on
        // change), so the first word's speaker labels the whole cue.
        speaker: words.first().and_then(|w| w.speaker),
    }
}

/// Format milliseconds as an SRT timestamp: `HH:MM:SS,mmm` (comma, not
/// period — SRT spec).
pub fn format_srt_timestamp(ms: u64) -> String {
    let hours = ms / 3_600_000;
    let minutes = (ms % 3_600_000) / 60_000;
    let seconds = (ms % 60_000) / 1_000;
    let millis = ms % 1_000;
    format!("{:02}:{:02}:{:02},{:03}", hours, minutes, seconds, millis)
}

/// The Call-mode side label for a channel-index speaker: interleaved channel 0
/// is the local mic, any other channel is the far end. Single source of truth,
/// shared by `render_srt`'s Call arm and the multichannel transcript builder.
pub fn call_side_label(speaker: u32) -> &'static str {
    if speaker == 0 {
        "אני"
    } else {
        "הצד השני"
    }
}

/// How `render_srt` labels each cue with its speaker. Crosses the Tauri IPC
/// boundary — the frontend picks a style per exported file (serde renders the
/// unit variants as the strings `"Diarization"` / `"Call"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SpeakerLabelStyle {
    /// Diarization export: prefix cues only when a file has ≥2 distinct
    /// speakers, using 1-based `"דובר {n+1}:"`. Single-speaker files stay
    /// byte-for-byte clean. This is the historical `render_srt` behavior.
    Diarization,
    /// Call export: always prefix every cue (even a single-speaker call where
    /// one side was silent), mapping interleaved channel 0 → `"אני:"` and any
    /// other channel → `"הצד השני:"` (see `call_side_label`).
    Call,
}

/// Render one or more files' cue lists into a single SRT document. Each
/// file's cues are offset by the cumulative end time of all files before
/// it (files play back-to-back, no artificial gap), and cue numbers are
/// sequential across the whole document.
///
/// `styles[i]` selects how file `i`'s cues are labeled; a missing entry falls
/// back to `Diarization`, so `render_srt(&files, &[])` reproduces the historical
/// output exactly. Style is chosen **per file**, not per document, because a
/// combined export can mix a Call recording with plain dictations.
pub fn render_srt(files: &[Vec<TimedSegment>], styles: &[SpeakerLabelStyle]) -> String {
    let mut out = String::new();
    let mut index = 1u32;
    let mut offset_ms: u64 = 0;

    for (file_idx, cues) in files.iter().enumerate() {
        let style = styles
            .get(file_idx)
            .copied()
            .unwrap_or(SpeakerLabelStyle::Diarization);

        // Whether this file's cues get a speaker prefix depends on the style.
        // Diarization labels only a genuinely multi-speaker file (single-speaker
        // dictation stays byte-for-byte clean); Call always labels, so a call in
        // which one side stayed silent still reads "אני:"/"הצד השני:".
        let label_speakers = match style {
            SpeakerLabelStyle::Diarization => {
                let distinct_speakers: std::collections::BTreeSet<u32> =
                    cues.iter().filter_map(|c| c.speaker).collect();
                distinct_speakers.len() >= 2
            }
            SpeakerLabelStyle::Call => true,
        };

        for cue in cues {
            out.push_str(&index.to_string());
            out.push('\n');
            out.push_str(&format_srt_timestamp(cue.start_ms + offset_ms));
            out.push_str(" --> ");
            out.push_str(&format_srt_timestamp(cue.end_ms + offset_ms));
            out.push('\n');
            if label_speakers {
                if let Some(spk) = cue.speaker {
                    let prefix = match style {
                        // Deepgram speaker indices are 0-based; display 1-based.
                        SpeakerLabelStyle::Diarization => format!("דובר {}: ", spk + 1),
                        // Call channels: 0 = local mic, any other = far end.
                        SpeakerLabelStyle::Call => format!("{}: ", call_side_label(spk)),
                    };
                    out.push_str(&prefix);
                }
            }
            out.push_str(&cue.text);
            out.push_str("\n\n");
            index += 1;
        }
        offset_ms += cues.last().map(|c| c.end_ms).unwrap_or(0);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start_ms: u64, end_ms: u64) -> TimedWord {
        TimedWord { text: text.to_string(), start_ms, end_ms, speaker: None }
    }

    fn word_spk(text: &str, start_ms: u64, end_ms: u64, speaker: Option<u32>) -> TimedWord {
        TimedWord { text: text.to_string(), start_ms, end_ms, speaker }
    }

    #[test]
    fn chunk_empty_input_yields_no_cues() {
        let words: Vec<TimedWord> = vec![];
        assert!(chunk_words_to_cues(&words, 10, 4000).is_empty());
    }

    #[test]
    fn chunk_single_word_yields_one_cue() {
        let words = vec![word("שלום", 0, 500)];
        let cues = chunk_words_to_cues(&words, 10, 4000);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "שלום");
        assert_eq!(cues[0].start_ms, 0);
        assert_eq!(cues[0].end_ms, 500);
    }

    #[test]
    fn chunk_splits_exactly_at_max_words() {
        let words: Vec<TimedWord> = (0..11u64)
            .map(|i| word(&format!("w{i}"), i * 100, i * 100 + 100))
            .collect();
        // Huge max_ms so only the word-count limit is exercised.
        let cues = chunk_words_to_cues(&words, 10, 100_000);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text.split(' ').count(), 10);
        assert_eq!(cues[1].text, "w10");
    }

    #[test]
    fn chunk_keeps_overlong_single_word_alone() {
        // First word alone spans 5s, already over the 4s max_ms budget.
        let words = vec![word("ארוכה", 0, 5000), word("הבא", 5000, 5300)];
        let cues = chunk_words_to_cues(&words, 10, 4000);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "ארוכה");
        assert_eq!(cues[0].end_ms, 5000);
        assert_eq!(cues[1].text, "הבא");
    }

    #[test]
    fn chunk_splits_when_speaker_changes() {
        // Two speakers within the same time/word budget must NOT share a cue —
        // a cue belongs to exactly one speaker, and its `speaker` is recorded.
        let words = vec![
            word_spk("שלום", 0, 500, Some(0)),
            word_spk("עולם", 500, 1000, Some(0)),
            word_spk("היי", 1000, 1500, Some(1)),
        ];
        let cues = chunk_words_to_cues(&words, 10, 4000);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].speaker, Some(0));
        assert_eq!(cues[0].text, "שלום עולם");
        assert_eq!(cues[1].speaker, Some(1));
        assert_eq!(cues[1].text, "היי");
    }

    #[test]
    fn format_timestamp_zero() {
        assert_eq!(format_srt_timestamp(0), "00:00:00,000");
    }

    #[test]
    fn format_timestamp_sub_second() {
        assert_eq!(format_srt_timestamp(1234), "00:00:01,234");
    }

    #[test]
    fn format_timestamp_over_one_hour() {
        // 1h 2m 3.456s
        assert_eq!(format_srt_timestamp(3_723_456), "01:02:03,456");
    }

    #[test]
    fn render_single_file_zero_offset() {
        let file = vec![TimedSegment { text: "היי".to_string(), start_ms: 0, end_ms: 900, speaker: None }];
        let srt = render_srt(&[file], &[SpeakerLabelStyle::Diarization]);
        assert_eq!(srt, "1\n00:00:00,000 --> 00:00:00,900\nהיי\n\n");
    }

    #[test]
    fn render_combines_files_with_cumulative_offset() {
        let file1 = vec![
            TimedSegment { text: "קובץ אחד".to_string(), start_ms: 0, end_ms: 1000, speaker: None },
            TimedSegment { text: "עוד קטע".to_string(), start_ms: 1000, end_ms: 2500, speaker: None },
        ];
        let file2 = vec![TimedSegment { text: "קובץ שתיים".to_string(), start_ms: 0, end_ms: 800, speaker: None }];

        let srt = render_srt(&[file1, file2], &[SpeakerLabelStyle::Diarization, SpeakerLabelStyle::Diarization]);

        let expected = "1\n00:00:00,000 --> 00:00:01,000\nקובץ אחד\n\n\
                         2\n00:00:01,000 --> 00:00:02,500\nעוד קטע\n\n\
                         3\n00:00:02,500 --> 00:00:03,300\nקובץ שתיים\n\n";
        assert_eq!(srt, expected);
    }

    #[test]
    fn render_labels_speakers_when_multiple() {
        // Two distinct speakers in the file → every cue gets a 1-based
        // "דובר N:" prefix (Deepgram speaker 0 → "דובר 1").
        let file = vec![
            TimedSegment { text: "שלום".to_string(), start_ms: 0, end_ms: 500, speaker: Some(0) },
            TimedSegment { text: "היי".to_string(), start_ms: 500, end_ms: 1000, speaker: Some(1) },
        ];
        let srt = render_srt(&[file], &[SpeakerLabelStyle::Diarization]);
        let expected = "1\n00:00:00,000 --> 00:00:00,500\nדובר 1: שלום\n\n\
                         2\n00:00:00,500 --> 00:00:01,000\nדובר 2: היי\n\n";
        assert_eq!(srt, expected);
    }

    #[test]
    fn render_single_speaker_has_no_labels() {
        // Only one speaker in the file → no labels at all. Single-speaker
        // dictation must stay byte-for-byte clean; labeling is opt-in on the
        // presence of a second speaker, not on diarization being active.
        let file = vec![
            TimedSegment { text: "שלום".to_string(), start_ms: 0, end_ms: 500, speaker: Some(0) },
            TimedSegment { text: "עולם".to_string(), start_ms: 500, end_ms: 1000, speaker: Some(0) },
        ];
        let srt = render_srt(&[file], &[SpeakerLabelStyle::Diarization]);
        let expected = "1\n00:00:00,000 --> 00:00:00,500\nשלום\n\n\
                         2\n00:00:00,500 --> 00:00:01,000\nעולם\n\n";
        assert_eq!(srt, expected);
    }

    #[test]
    fn render_call_labels_both_sides() {
        // Call always labels: channel 0 → "אני:", channel 1 → "הצד השני:".
        let file = vec![
            TimedSegment { text: "שלום".to_string(), start_ms: 0, end_ms: 500, speaker: Some(0) },
            TimedSegment { text: "היי".to_string(), start_ms: 500, end_ms: 1000, speaker: Some(1) },
        ];
        let srt = render_srt(&[file], &[SpeakerLabelStyle::Call]);
        let expected = "1\n00:00:00,000 --> 00:00:00,500\nאני: שלום\n\n\
                         2\n00:00:00,500 --> 00:00:01,000\nהצד השני: היי\n\n";
        assert_eq!(srt, expected);
    }

    #[test]
    fn render_call_labels_single_speaker_when_one_side_silent() {
        // A call where only one side spoke has a single distinct speaker, which
        // would suppress labels under Diarization's ≥2 gate. Call bypasses the
        // gate and still labels every cue "אני:".
        let file = vec![
            TimedSegment { text: "שלום".to_string(), start_ms: 0, end_ms: 500, speaker: Some(0) },
            TimedSegment { text: "עולם".to_string(), start_ms: 500, end_ms: 1000, speaker: Some(0) },
        ];
        let srt = render_srt(&[file], &[SpeakerLabelStyle::Call]);
        let expected = "1\n00:00:00,000 --> 00:00:00,500\nאני: שלום\n\n\
                         2\n00:00:00,500 --> 00:00:01,000\nאני: עולם\n\n";
        assert_eq!(srt, expected);
    }

    #[test]
    fn render_defaults_to_diarization_when_styles_missing() {
        // An empty styles slice reproduces the historical one-arg behavior, so a
        // frontend that omits `styles` exports byte-for-byte as it does today.
        let file = vec![
            TimedSegment { text: "שלום".to_string(), start_ms: 0, end_ms: 500, speaker: Some(0) },
            TimedSegment { text: "היי".to_string(), start_ms: 500, end_ms: 1000, speaker: Some(1) },
        ];
        let srt = render_srt(&[file], &[]);
        let expected = "1\n00:00:00,000 --> 00:00:00,500\nדובר 1: שלום\n\n\
                         2\n00:00:00,500 --> 00:00:01,000\nדובר 2: היי\n\n";
        assert_eq!(srt, expected);
    }

    #[test]
    fn render_applies_style_per_file() {
        // A combined export can mix a Call recording with a plain dictation:
        // file 0 must read "אני:/הצד השני:", file 1 falls back to "דובר N:".
        let call_file = vec![
            TimedSegment { text: "שלום".to_string(), start_ms: 0, end_ms: 500, speaker: Some(0) },
            TimedSegment { text: "היי".to_string(), start_ms: 500, end_ms: 1000, speaker: Some(1) },
        ];
        let diar_file = vec![
            TimedSegment { text: "אחד".to_string(), start_ms: 0, end_ms: 400, speaker: Some(0) },
            TimedSegment { text: "שתיים".to_string(), start_ms: 400, end_ms: 900, speaker: Some(1) },
        ];
        let srt = render_srt(
            &[call_file, diar_file],
            &[SpeakerLabelStyle::Call, SpeakerLabelStyle::Diarization],
        );
        let expected = "1\n00:00:00,000 --> 00:00:00,500\nאני: שלום\n\n\
                         2\n00:00:00,500 --> 00:00:01,000\nהצד השני: היי\n\n\
                         3\n00:00:01,000 --> 00:00:01,400\nדובר 1: אחד\n\n\
                         4\n00:00:01,400 --> 00:00:01,900\nדובר 2: שתיים\n\n";
        assert_eq!(srt, expected);
    }
}
