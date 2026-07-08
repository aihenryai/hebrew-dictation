# SRT Export Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user export a time-synced `.srt` subtitle file from a batch-transcribed audio/video file (cloud Deepgram or local whisper), per-file or combined across multiple files.

**Architecture:** Both batch transcription routes are changed to return timed cues (`TimedSegment { text, start_ms, end_ms }`) alongside the existing plain-text transcript — Deepgram by bucketing its native `words[]` array, whisper by leaning on whisper.cpp's own `max_len`/`split_on_word` segmentation (no per-token digging needed). A new pure `srt.rs` module renders cues (single or multi-file, with cumulative offset) into standard SRT text; a new `export_srt` Tauri command writes it via the same save-dialog pattern as `export_history`. The frontend gets a third "SRT" export button next to the existing TXT/Word buttons, gated on the item having un-edited timed segments.

**Tech Stack:** Rust (Tauri v2, whisper-rs 0.16, reqwest/serde_json for Deepgram), React/TypeScript (Tauri `invoke`).

**Spec:** `docs/superpowers/specs/2026-07-03-srt-export-design.md` — read this first for the full rationale (scope decisions, accepted cue-length parity gap between routes, edited-transcript desync handling, `no_timestamps` fallback plan).

---

## Chunk 1: Backend — timed segments through the transcription pipeline

### Task 1: `srt.rs` — pure cue-chunking and SRT-rendering module (TDD)

**Files:**
- Create: `src-tauri/src/srt.rs`
- Modify: `src-tauri/src/lib.rs:1` (module declarations block, currently `mod api_transcribe;` … `mod whisper;`) — add `mod srt;` alphabetically after `mod settings;` and before `mod streaming;`

This module has zero dependencies on Tauri/AppState/whisper-rs/reqwest — it's pure data transformation, fully unit-testable without mocking anything.

- [ ] **Step 1: Create `src-tauri/src/srt.rs` with types and function signatures (bodies `todo!()`), plus the test module**

```rust
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
}

/// A single transcribed word with its timing, as reported by Deepgram's
/// `words[]` array (seconds in the API, converted to ms by the caller).
#[derive(Debug, Clone)]
pub struct TimedWord {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
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
    todo!()
}

/// Format milliseconds as an SRT timestamp: `HH:MM:SS,mmm` (comma, not
/// period — SRT spec).
pub fn format_srt_timestamp(ms: u64) -> String {
    todo!()
}

/// Render one or more files' cue lists into a single SRT document. Each
/// file's cues are offset by the cumulative end time of all files before
/// it (files play back-to-back, no artificial gap), and cue numbers are
/// sequential across the whole document. A single-file call is just
/// `render_srt(&[cues])` with a zero offset.
pub fn render_srt(files: &[Vec<TimedSegment>]) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start_ms: u64, end_ms: u64) -> TimedWord {
        TimedWord { text: text.to_string(), start_ms, end_ms }
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
        let file = vec![TimedSegment { text: "היי".to_string(), start_ms: 0, end_ms: 900 }];
        let srt = render_srt(&[file]);
        assert_eq!(srt, "1\n00:00:00,000 --> 00:00:00,900\nהיי\n\n");
    }

    #[test]
    fn render_combines_files_with_cumulative_offset() {
        let file1 = vec![
            TimedSegment { text: "קובץ אחד".to_string(), start_ms: 0, end_ms: 1000 },
            TimedSegment { text: "עוד קטע".to_string(), start_ms: 1000, end_ms: 2500 },
        ];
        let file2 = vec![TimedSegment { text: "קובץ שתיים".to_string(), start_ms: 0, end_ms: 800 }];

        let srt = render_srt(&[file1, file2]);

        let expected = "1\n00:00:00,000 --> 00:00:01,000\nקובץ אחד\n\n\
                         2\n00:00:01,000 --> 00:00:02,500\nעוד קטע\n\n\
                         3\n00:00:02,500 --> 00:00:03,300\nקובץ שתיים\n\n";
        assert_eq!(srt, expected);
    }
}
```

- [ ] **Step 2: Add `mod srt;` to `src-tauri/src/lib.rs`**

In the module declarations at the top of the file (currently lines 1-12: `mod api_transcribe;` through `mod whisper;`), add alphabetically:

```rust
mod settings;
mod srt;
mod streaming;
```

- [ ] **Step 3: Run tests to verify they fail on the `todo!()` panics**

Run (from `src-tauri/`): `cargo test srt::`
Expected: all `srt::tests::*` tests **panic** with `not yet implemented` (compiles fine — this confirms the test harness is wired up correctly before you implement).

- [ ] **Step 4: Implement `chunk_words_to_cues`**

```rust
pub fn chunk_words_to_cues(words: &[TimedWord], max_words: usize, max_ms: u64) -> Vec<TimedSegment> {
    let mut cues = Vec::new();
    let mut current: Vec<&TimedWord> = Vec::new();

    for w in words {
        if !current.is_empty() {
            let span = w.end_ms.saturating_sub(current[0].start_ms);
            if current.len() >= max_words || span > max_ms {
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
    }
}
```

- [ ] **Step 5: Implement `format_srt_timestamp`**

```rust
pub fn format_srt_timestamp(ms: u64) -> String {
    let hours = ms / 3_600_000;
    let minutes = (ms % 3_600_000) / 60_000;
    let seconds = (ms % 60_000) / 1_000;
    let millis = ms % 1_000;
    format!("{:02}:{:02}:{:02},{:03}", hours, minutes, seconds, millis)
}
```

- [ ] **Step 6: Implement `render_srt`**

```rust
pub fn render_srt(files: &[Vec<TimedSegment>]) -> String {
    let mut out = String::new();
    let mut index = 1u32;
    let mut offset_ms: u64 = 0;

    for cues in files {
        for cue in cues {
            out.push_str(&index.to_string());
            out.push('\n');
            out.push_str(&format_srt_timestamp(cue.start_ms + offset_ms));
            out.push_str(" --> ");
            out.push_str(&format_srt_timestamp(cue.end_ms + offset_ms));
            out.push('\n');
            out.push_str(&cue.text);
            out.push_str("\n\n");
            index += 1;
        }
        offset_ms += cues.last().map(|c| c.end_ms).unwrap_or(0);
    }

    out
}
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test srt::`
Expected: `test result: ok. 9 passed; 0 failed`

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/srt.rs src-tauri/src/lib.rs
git commit -m "feat(srt): add pure cue-chunking + SRT rendering module (TDD)"
```

---

### Task 2: Deepgram batch route returns timed segments

**Files:**
- Modify: `src-tauri/src/api_transcribe.rs` — `transcribe_deepgram_batch` (currently ~lines 252-296)
- Modify: `src-tauri/src/lib.rs` — its one caller in `run_transcribe_file`'s `CloudDeepgram` branch (currently ~lines 413-420)

No new unit tests here — `chunk_words_to_cues` is already covered; this task is wiring plus a manual smoke check later (Task 6).

- [ ] **Step 1: Change `transcribe_deepgram_batch`'s return type and parse `words[]`**

Replace the function in `src-tauri/src/api_transcribe.rs` (keep everything before the final `let alt = ...` block unchanged):

```rust
pub(crate) async fn transcribe_deepgram_batch(
    client: &reqwest::Client,
    samples: &[f32],
    api_key: &str,
    language: &str,
) -> Result<(String, Vec<crate::srt::TimedSegment>), ApiError> {
    let wav_data = samples_to_wav(samples, 16000);
    let lang = if language == "auto" { "he" } else { language };
    let url = format!(
        "https://api.deepgram.com/v1/listen?model=nova-3&language={}&smart_format=true&punctuate=true&paragraphs=true",
        lang
    );

    let response = client
        .post(&url)
        .header("Authorization", format!("Token {}", api_key))
        .header("Content-Type", "audio/wav")
        .body(wav_data)
        .send()
        .await
        .map_err(|e| classify_request_error(&e))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(classify_status(status, &body));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| ApiError::Other(format!("Failed to parse Deepgram response: {}", e)))?;

    let alt = &body["results"]["channels"][0]["alternatives"][0];
    // paragraphs=true gives a newline-formatted transcript; fall back to the flat one.
    let transcript = alt["paragraphs"]["transcript"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| alt["transcript"].as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    // words[] is present by default (no extra request param needed), each
    // {word, start, end, punctuated_word?} with start/end in fractional seconds.
    let words: Vec<crate::srt::TimedWord> = alt["words"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|w| {
                    let text = w["punctuated_word"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .or_else(|| w["word"].as_str())?;
                    let start = w["start"].as_f64()?;
                    let end = w["end"].as_f64()?;
                    Some(crate::srt::TimedWord {
                        text: text.to_string(),
                        start_ms: (start * 1000.0) as u64,
                        end_ms: (end * 1000.0) as u64,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let segments = crate::srt::chunk_words_to_cues(
        &words,
        crate::srt::SRT_MAX_WORDS_PER_CUE,
        crate::srt::SRT_MAX_MS_PER_CUE,
    );

    Ok((transcript, segments))
}
```

- [ ] **Step 2: Update the caller in `src-tauri/src/lib.rs`**

In `run_transcribe_file`'s `CloudDeepgram` branch, change:

```rust
            let notify = state.batch_cancel_notify.clone();
            let fut = api_transcribe::transcribe_deepgram_batch(&client, &samples, &key, &opts.language);
            let text = tokio::select! {
                r = fut => r.map_err(|e| e.to_string())?,
                _ = notify.notified() => return Err(batch::CANCELLED.to_string()),
            };
            let _ = app.emit("batch-progress", serde_json::json!({ "stage": "done", "pct": 100 }));
            Ok(text)
```

to:

```rust
            let notify = state.batch_cancel_notify.clone();
            let fut = api_transcribe::transcribe_deepgram_batch(&client, &samples, &key, &opts.language);
            let (text, segments) = tokio::select! {
                r = fut => r.map_err(|e| e.to_string())?,
                _ = notify.notified() => return Err(batch::CANCELLED.to_string()),
            };
            let _ = app.emit("batch-progress", serde_json::json!({ "stage": "done", "pct": 100 }));
            Ok(TranscribeFileResult { text, segments })
```

(`TranscribeFileResult` is defined in Task 4 — this file won't compile standalone until Task 4 lands; that's expected, verify compilation at the end of Task 4 instead of here.)

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/api_transcribe.rs src-tauri/src/lib.rs
git commit -m "feat(srt): Deepgram batch route emits timed cues from words[]"
```

---

### Task 3: Local whisper route returns timed segments

**Files:**
- Modify: `src-tauri/src/whisper.rs` — `run_long_transcription` (currently ~lines 179-235)
- Modify: `src-tauri/src/lib.rs` — its one caller in `run_transcribe_file`'s `Local` branch (currently ~lines 449-467)

- [ ] **Step 1: Change `run_long_transcription`'s params and return type**

In `src-tauri/src/whisper.rs`, inside `run_long_transcription`, change:

```rust
    params.set_translate(false);
    params.set_no_timestamps(true);
    params.set_print_progress(false);
```

to:

```rust
    params.set_translate(false);
    // SRT needs real segment timing (was `true`) — see spec's "Open risk to
    // verify at runtime, with fallback" if this regresses accuracy/speed.
    params.set_no_timestamps(false);
    // whisper.cpp's own SRT-chunking mechanism (same as its `--max-len` CLI
    // flag): caps each segment's length and never cuts mid-word, so
    // get_segment/start_timestamp/end_timestamp below already come back
    // pre-chunked into short, readable cues — no token-level API needed.
    params.set_max_len(42);
    params.set_split_on_word(true);
    params.set_print_progress(false);
```

Then change the function signature and the segment-collection loop at the end. Signature:

```rust
pub fn run_long_transcription<F: FnMut(i32) + 'static>(
    mut state: WhisperState,
    model_name: &str,
    samples: &[f32],
    language: &str,
    cancel: Arc<AtomicBool>,
    on_progress: F,
) -> Result<(String, Vec<crate::srt::TimedSegment>), String> {
```

End of function (replace the existing text-collection loop and final `Ok(...)`):

```rust
    let n = state.full_n_segments();
    let mut text = String::new();
    let mut segments = Vec::new();
    for i in 0..n {
        if let Some(segment) = state.get_segment(i) {
            if let Ok(s) = segment.to_str_lossy() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    text.push_str(&s);
                    // start_timestamp/end_timestamp are in centiseconds (10ms units).
                    segments.push(crate::srt::TimedSegment {
                        text: trimmed.to_string(),
                        start_ms: (segment.start_timestamp().max(0) as u64) * 10,
                        end_ms: (segment.end_timestamp().max(0) as u64) * 10,
                    });
                }
            }
        }
    }
    Ok((text.trim().to_string(), segments))
```

- [ ] **Step 2: Update the caller in `src-tauri/src/lib.rs`**

In `run_transcribe_file`'s `Local` branch, change:

```rust
            let text = tokio::task::spawn_blocking(move || {
                whisper::run_long_transcription(
                    wstate,
                    &model_name,
                    &samples_owned,
                    &lang,
                    cancel,
                    move |p| {
                        let _ = tx.send(p);
                    },
                )
            })
            .await
            .map_err(|e| format!("שגיאת משימת תמלול: {}", e))??;

            progress_task.abort();
            let _ = app.emit("batch-progress", serde_json::json!({ "stage": "done", "pct": 100 }));
            Ok(text)
```

to:

```rust
            let (text, segments) = tokio::task::spawn_blocking(move || {
                whisper::run_long_transcription(
                    wstate,
                    &model_name,
                    &samples_owned,
                    &lang,
                    cancel,
                    move |p| {
                        let _ = tx.send(p);
                    },
                )
            })
            .await
            .map_err(|e| format!("שגיאת משימת תמלול: {}", e))??;

            progress_task.abort();
            let _ = app.emit("batch-progress", serde_json::json!({ "stage": "done", "pct": 100 }));
            Ok(TranscribeFileResult { text, segments })
```

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/whisper.rs src-tauri/src/lib.rs
git commit -m "feat(srt): local whisper route emits timed cues via max_len/split_on_word"
```

---

### Task 4: `transcribe_file` returns `{ text, segments }`

**Files:**
- Modify: `src-tauri/src/lib.rs` — add `TranscribeFileResult` struct, update `transcribe_file` + `run_transcribe_file` signatures (currently ~lines 329-351)

This is the task where Tasks 2 and 3's callers actually compile — do this task, then build.

- [ ] **Step 1: Add the `TranscribeFileResult` struct**

Directly above `#[tauri::command]\nasync fn transcribe_file(` in `src-tauri/src/lib.rs`, add:

```rust
/// Response shape for `transcribe_file`: the plain transcript (unchanged
/// consumer for inject/copy/TXT/DOCX) plus timed cues for SRT export.
/// `segments` is empty only if a route produced no timed cues (defensive —
/// the frontend treats empty `segments` as "SRT unavailable for this item").
#[derive(Debug, Clone, serde::Serialize)]
struct TranscribeFileResult {
    text: String,
    segments: Vec<srt::TimedSegment>,
}
```

- [ ] **Step 2: Update `transcribe_file` and `run_transcribe_file` signatures**

Change:

```rust
#[tauri::command]
async fn transcribe_file(
    app: AppHandle,
    state: State<'_, AppState>,
    file_path: String,
    opts: batch::BatchOpts,
) -> Result<String, String> {
```

to:

```rust
#[tauri::command]
async fn transcribe_file(
    app: AppHandle,
    state: State<'_, AppState>,
    file_path: String,
    opts: batch::BatchOpts,
) -> Result<TranscribeFileResult, String> {
```

and:

```rust
async fn run_transcribe_file(
    app: &AppHandle,
    state: &State<'_, AppState>,
    file_path: String,
    opts: batch::BatchOpts,
) -> Result<String, String> {
```

to:

```rust
async fn run_transcribe_file(
    app: &AppHandle,
    state: &State<'_, AppState>,
    file_path: String,
    opts: batch::BatchOpts,
) -> Result<TranscribeFileResult, String> {
```

(The function body's early `return Err(...)` lines are untouched — only the two `Ok(...)` success paths, already updated in Tasks 2/3, need to match this new `Ok(TranscribeFileResult { .. })` shape.)

- [ ] **Step 3: Build to verify the whole chain compiles**

Run (from `src-tauri/`): `cargo check`
Expected: `Finished` with 0 errors. If it doesn't compile, the likely culprits are a mismatched `Ok(...)` shape in one of the two branches from Tasks 2/3 — fix those, not this task's code.

- [ ] **Step 4: Run the full test suite**

Run: `cargo test`
Expected: all existing tests plus the 9 new `srt::tests::*` tests pass (previous count + 9, 0 failed).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat(srt): transcribe_file returns {text, segments}"
```

---

## Chunk 2: `export_srt` command + frontend wiring

### Task 5: `export_srt` Tauri command

**Files:**
- Modify: `src-tauri/src/lib.rs` — new `export_srt` command (place directly after `export_history`, currently ending ~line 1069), plus one entry in the `tauri::generate_handler!` list (currently ~line 1679, `export_history,`)

No unit test here — `srt::render_srt` is already covered; this is I/O glue tested manually in Task 7 (SRT files aren't meaningfully mockable without a real save dialog, which the existing `export_history` also doesn't unit-test).

- [ ] **Step 1: Add the `export_srt` command**

Directly after the `export_history` function's closing brace in `src-tauri/src/lib.rs`:

```rust
/// `items` is one Vec<TimedSegment> per source file, in original order — a
/// single-file export is `items.len() == 1`; a combined export lists every
/// done file's cues so `srt::render_srt` can offset and renumber them.
/// `suggested_name` is an optional content-derived filename (no extension);
/// falls back to a timestamp, matching `export_history`'s convention.
#[tauri::command]
async fn export_srt(
    app: AppHandle,
    state: State<'_, AppState>,
    items: Vec<Vec<srt::TimedSegment>>,
    suggested_name: Option<String>,
) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;

    if items.is_empty() || items.iter().all(|f| f.is_empty()) {
        return Err("אין כתוביות לייצוא — תמלל קודם קובץ אודיו/וידאו.".to_string());
    }

    let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M");
    let default_name = match suggested_name.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(name) => format!("{}.srt", sanitize_filename(name)),
        None => format!("hebrew-dictation-subtitles_{}.srt", timestamp),
    };

    let restore_on_top = state.settings.lock().map(|s| s.always_on_top).unwrap_or(true);
    set_main_on_top(&app, false);

    let (tx, rx) = tokio::sync::oneshot::channel::<Option<std::path::PathBuf>>();
    app.dialog()
        .file()
        .set_title("שמור כתוביות כקובץ SRT")
        .set_file_name(&default_name)
        .add_filter("כתוביות SRT", &["srt"])
        .save_file(move |result| {
            let path = result.and_then(|fp| fp.into_path().ok());
            let _ = tx.send(path);
        });

    let path = rx.await.map_err(|_| "דיאלוג השמירה נסגר ללא תגובה".to_string());
    set_main_on_top(&app, restore_on_top);
    let path = path?;
    let path = match path {
        Some(p) => p,
        None => return Err("הייצוא בוטל".to_string()),
    };

    let content = srt::render_srt(&items);
    std::fs::write(&path, content.as_bytes())
        .map_err(|e| format!("שגיאה בכתיבת קובץ SRT: {}", e))?;

    Ok(path.to_string_lossy().to_string())
}
```

- [ ] **Step 2: Register the command**

In the `tauri::generate_handler![...]` list, add `export_srt,` directly after `export_history,`:

```rust
            export_history,
            export_srt,
            get_audio_devices,
```

- [ ] **Step 3: Build**

Run (from `src-tauri/`): `cargo check`
Expected: `Finished` with 0 errors.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat(srt): export_srt command — write cues to a standard .srt file"
```

---

### Task 6: Frontend — SRT buttons, edited-transcript guard

**Files:**
- Modify: `src/App.tsx` — `BatchResult`/new `TimedSegment` types (~line 46), both `transcribe_file` call sites (~line 970 multi-file loop, ~line 1060 record-and-transcribe), textarea `onChange` (~line 2440), export handlers (~line 1118 area, after `exportBatch`), per-item action buttons (~line 2464-2477), combined action bar (~line 2488-2497)

No automated frontend tests exist in this project (confirmed: no `.test.tsx`/`.spec.tsx` files) — this task is verified manually in Task 7, consistent with how the existing TXT/Word export buttons were shipped.

- [ ] **Step 1: Add `TimedSegment` type and extend `BatchResult`**

Directly above `interface BatchResult` (~line 46) in `src/App.tsx`:

```ts
interface TimedSegment {
  text: string;
  start_ms: number;
  end_ms: number;
}
```

Then extend `BatchResult`:

```ts
interface BatchResult {
  id: number;
  fileName: string;
  filePath: string;
  transcript: string;
  status: BatchFileStatus;
  error?: string;
  segments?: TimedSegment[];
  /** True once the user hand-edits `transcript` in the textarea — segments
   * no longer match the (unedited) text, so SRT export is hidden for this item. */
  edited?: boolean;
}
```

- [ ] **Step 2: Update the multi-file `transcribe_file` call site**

Change (currently ~line 969-976):

```ts
      try {
        const text = await invoke<string>("transcribe_file", {
          filePath: initial[i].filePath,
          opts: { mode: batchMode, language: "he", inject: false },
        });
        setBatchResults((prev) =>
          prev.map((r) => r.id === curId ? { ...r, status: "done", transcript: text } : r)
        );
      } catch (e) {
```

to:

```ts
      try {
        const { text, segments } = await invoke<{ text: string; segments: TimedSegment[] }>(
          "transcribe_file",
          { filePath: initial[i].filePath, opts: { mode: batchMode, language: "he", inject: false } }
        );
        setBatchResults((prev) =>
          prev.map((r) => r.id === curId ? { ...r, status: "done", transcript: text, segments } : r)
        );
      } catch (e) {
```

- [ ] **Step 3: Update the record-and-transcribe `transcribe_file` call site**

Change (currently ~line 1059-1064):

```ts
    try {
      const text = await invoke<string>("transcribe_file", {
        filePath,
        opts: { mode: batchMode, language: "he", inject: false },
      });
      setBatchResults((prev) => prev.map((r) => r.id === newId ? { ...r, status: "done", transcript: text } : r));
    } catch (e) {
```

to:

```ts
    try {
      const { text, segments } = await invoke<{ text: string; segments: TimedSegment[] }>(
        "transcribe_file",
        { filePath, opts: { mode: batchMode, language: "he", inject: false } }
      );
      setBatchResults((prev) => prev.map((r) => r.id === newId ? { ...r, status: "done", transcript: text, segments } : r));
    } catch (e) {
```

- [ ] **Step 4: Set `edited: true` in the textarea's `onChange`**

Change (currently ~line 2440-2445):

```tsx
                      onChange={(e) => {
                        const val = e.target.value;
                        setBatchResults((prev) =>
                          prev.map((r, i) => i === idx ? { ...r, transcript: val } : r)
                        );
                      }}
```

to:

```tsx
                      onChange={(e) => {
                        const val = e.target.value;
                        setBatchResults((prev) =>
                          prev.map((r, i) => i === idx ? { ...r, transcript: val, edited: true } : r)
                        );
                      }}
```

- [ ] **Step 5: Add `exportSingleSrt` and `exportBatchSrt` handlers**

Directly after the existing `exportBatch` callback (~line 1131, right after its closing `}, [batchResults]);`), add:

```ts
  const exportSingleSrt = useCallback(async (
    segments: TimedSegment[],
    transcriptForName: string,
    onErr: (msg: string) => void,
  ) => {
    if (segments.length === 0) return;
    try {
      await invoke<string>("export_srt", {
        items: [segments],
        suggested_name: firstWordsName(transcriptForName),
      });
    } catch (e) {
      const msg = String(e);
      if (!msg.includes("הייצוא בוטל")) onErr(`ייצוא נכשל: ${msg}`);
    }
  }, []);

  const exportBatchSrt = useCallback(async () => {
    const eligible = batchResults.filter(
      (r) => r.status === "done" && !r.edited && r.segments && r.segments.length > 0
    );
    if (eligible.length === 0) return;
    try {
      const items = eligible.map((r) => r.segments!);
      const suggested_name = generateExportName(eligible);
      const path = await invoke<string>("export_srt", { items, suggested_name });
      setExportNotice(`✅ נשמר: ${path}`);
      window.setTimeout(() => setExportNotice(null), 6000);
    } catch (e) {
      const msg = String(e);
      if (!msg.includes("הייצוא בוטל")) setBatchError(`ייצוא נכשל: ${msg}`);
    }
  }, [batchResults]);
```

- [ ] **Step 6: Add the per-item SRT button**

Directly after the existing "📝 Word" button in the per-item action bar (currently ~line 2471-2477):

```tsx
                      <button
                        className="btn-secondary btn-sm"
                        onClick={() => exportSingle(result.transcript, "docx", setBatchError)}
                        title="ייצוא מקטע זה כמסמך Word"
                      >
                        📝 Word
                      </button>
                      {result.segments && result.segments.length > 0 && !result.edited && (
                        <button
                          className="btn-secondary btn-sm"
                          onClick={() => exportSingleSrt(result.segments!, result.transcript, setBatchError)}
                          title="ייצוא כתוביות SRT למקטע זה"
                        >
                          🎬 SRT
                        </button>
                      )}
```

- [ ] **Step 7: Add the combined SRT button**

Change the combined action bar (currently ~line 2488-2497):

```tsx
        {!batchRunning && !batchRecording && batchResults.length > 0 && (
          <div className="batch-action-bar">
            {doneCount > 1 && (
              <>
                <span className="batch-export-all-label">ייצוא הכל:</span>
                <button className="btn-secondary btn-sm" onClick={() => exportBatch("txt")}>📄 TXT</button>
                <button className="btn-secondary btn-sm" onClick={() => exportBatch("docx")}>📝 Word</button>
              </>
            )}
            <button className="btn-secondary btn-sm batch-clear-btn" onClick={() => setBatchResults([])}>נקה</button>
          </div>
        )}
```

to:

```tsx
        {!batchRunning && !batchRecording && batchResults.length > 0 && (
          <div className="batch-action-bar">
            {doneCount > 1 && (
              <>
                <span className="batch-export-all-label">ייצוא הכל:</span>
                <button className="btn-secondary btn-sm" onClick={() => exportBatch("txt")}>📄 TXT</button>
                <button className="btn-secondary btn-sm" onClick={() => exportBatch("docx")}>📝 Word</button>
                {batchResults.filter((r) => r.status === "done" && !r.edited && r.segments && r.segments.length > 0).length > 1 && (
                  <button className="btn-secondary btn-sm" onClick={() => exportBatchSrt()}>🎬 SRT</button>
                )}
              </>
            )}
            <button className="btn-secondary btn-sm batch-clear-btn" onClick={() => setBatchResults([])}>נקה</button>
          </div>
        )}
```

(The combined SRT button only appears when more than one *unedited, timed* item exists — same spirit as the existing `doneCount > 1` gate, extended with the edited/segments guard from Task 6 Step 1.)

- [ ] **Step 8: Type-check**

Run (from repo root): `npx tsc --noEmit`
Expected: exit code 0, no errors.

- [ ] **Step 9: Commit**

```bash
git add src/App.tsx
git commit -m "feat(srt): frontend SRT export buttons (per-item + combined), edited-transcript guard"
```

---

### Task 7: Manual runtime verification (Henry, `npm run tauri dev`)

Not automatable — requires real audio and a real save dialog. Check each item and note pass/fail; if the `no_timestamps(false)` item regresses, apply the fallback described in the spec's "Open risk to verify at runtime, with fallback" section before shipping.

- [ ] Local whisper batch on a real Hebrew recording → transcript accuracy/speed feel the same as before this change (this is the `no_timestamps(false)` risk check)
- [ ] Local whisper batch → export SRT (per-item) → open in VLC → captions are in sync, each cue is short and readable (not one giant block)
- [ ] Cloud Deepgram batch → export SRT (per-item) → same VLC check
- [ ] Batch-transcribe 2+ files → "יצא משולב" → SRT → confirm the second file's cues start exactly at the first file's audio duration (no overlap, no gap)
- [ ] Edit a transcript in the textarea after transcription completes → confirm the SRT button disappears for that item (and, if it was part of eligibility for the combined button, that the combined button also updates)
- [ ] Cancel a batch transcription mid-run → confirm no SRT button appears for the cancelled/errored item (only for `status === "done"` items, already the existing gate)

- [ ] **Step: Commit any fixes found during manual verification separately, each with its own focused commit message.**
