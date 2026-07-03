# SRT Export — Design Spec

Date: 2026-07-03
Status: Approved by Henry (conversational review)

## Problem

Batch transcription (`transcribe_file`) supports two routes — cloud (Deepgram)
and local (whisper) — but both discard timing information: `transcribe_deepgram_batch`
extracts only `alternatives[0].transcript`/`paragraphs.transcript`, and
`run_long_transcription` concatenates `get_segment(i)` text with
`params.set_no_timestamps(true)`, so no start/end times ever leave the whisper
engine. The existing export path (`export_history` → `export::write_txt`/`write_docx`)
operates on plain `{text, timestamp}` items where `timestamp` is wall-clock
creation time, not audio-position time — unusable for subtitle sync.

Goal: let a user who batch-transcribed an audio/video file export a standard
`.srt` subtitle file time-synced to that file, for use in video editors,
players, and YouTube upload.

## Scope (confirmed with Henry)

- **Both batch routes** get SRT support in v1: Deepgram cloud and local whisper.
- SRT is **not** offered for short-dictation history (Alt+D) — those items have
  no associated audio file, only a wall-clock creation timestamp, so there is
  nothing to sync against.
- SRT is offered **per single batch item** (one file → one `.srt`) **and** for
  the existing "יצא משולב" (combined) batch export, which today concatenates
  multiple files' transcripts into one TXT/DOCX. The combined SRT concatenates
  cues with a cumulative time offset per file (file B's cues start exactly at
  file A's audio duration — back-to-back, no artificial gap).
- Caption chunking: **short segments**, not one giant cue per sentence/paragraph.
  Target ~10 words / ~4s per cue (whichever limit is hit first), word-boundary
  aligned.
- Filename convention: same as the existing TXT/DOCX per-item/combined export —
  content-derived from the transcript's opening words (`firstWordsName` /
  `sanitize_filename`), **not** matched to the original video/audio filename.

## Architecture

### 1. Timed segments through the transcription pipeline

Both batch routes change from returning `String` to returning
`(String, Vec<TimedSegment>)` (full text unchanged for TXT/DOCX/inject; segments
new, used only for SRT).

```rust
// New, in a shared location (batch.rs or a new srt.rs)
pub struct TimedSegment {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}
```

**Deepgram (`api_transcribe.rs::transcribe_deepgram_batch`):** the API already
returns `results.channels[0].alternatives[0].words[]`, each `{word, start, end,
punctuated_word?}` in **floating-point seconds** — no request-side change
needed (`words` is present by default, no `words=true` param required).
Convert to `TimedSegment`'s `u64` millisecond fields via `(secs * 1000.0) as
u64` — a different conversion from whisper's centisecond×10 below; don't
conflate the two when implementing. Add a pure, testable chunking function:

```rust
pub fn chunk_words_to_cues(words: &[DeepgramWord], max_words: usize, max_ms: u64) -> Vec<TimedSegment>
```

Greedy accumulation: add words to the current cue until appending the next word
would exceed `max_words` (10) or the cue's span would exceed `max_ms` (4000),
then flush and start a new cue. A single word alone spanning more than `max_ms`
still ships as its own one-word cue (never drop content).

**Whisper (`whisper.rs::run_long_transcription`):** enable real timestamps and
let whisper.cpp do the chunking natively — this is the same mechanism
whisper.cpp's own `--max-len` SRT export uses, so no token-level API needed:

```rust
params.set_no_timestamps(false);   // was true — SRT needs real segment timing
params.set_max_len(42);            // classic subtitle line-length cap (chars)
params.set_split_on_word(true);    // never cut mid-word
```

Then iterate `state.get_segment(i)` as today, but also read
`segment.start_timestamp()` / `end_timestamp()` (centiseconds → ×10 for ms) per
segment to build `Vec<TimedSegment>` alongside the concatenated text.

**Open risk to verify at runtime, with fallback:** flipping `no_timestamps` from
`true`→`false` changes whisper.cpp's decoding behavior (it now predicts
timestamp tokens) for **every** local batch transcription, not just ones headed
to SRT — a regression here would silently degrade the core transcription
feature. Must confirm in manual testing (see Testing) that this doesn't
measurably hurt accuracy or speed on a real Hebrew recording before release.
**Fallback if it does regress:** keep `no_timestamps(true)` for the main decode
(untouched, zero risk to existing behavior) and instead enable
`set_token_timestamps(true)` — whisper.cpp's cross-attention-based per-token
timing, which is independent of decoder-predicted timestamp tokens. Build
`TimedSegment`s by grouping the resulting per-token times with the same
`chunk_words_to_cues`-style word/duration bucketing already used for Deepgram,
instead of relying on whisper.cpp's own `max_len`/`split_on_word` segmentation.
This confines the fallback to the SRT-timing path only.

**Cue-length parity between routes:** Deepgram cues are bucketed by an explicit
word/time budget (~10 words / ~4s). Whisper cues are bucketed by whisper.cpp's
own character-length cap (`max_len`, script-agnostic character count, not a
word count). These are two different proxies for the same goal — readable cue
duration — and won't produce identical cue boundaries for equivalent speech;
this is accepted for v1. The `max_len(42)` constant is the classic subtitle
line-length convention (chosen for reading-speed readability, the same reason
behind the ~10-word/~4s target), not a value re-derived word-by-word from the
Deepgram target — sanity-check during manual testing that whisper cues land in
a comparable ballpark (roughly 7-10 Hebrew words per cue) and adjust `max_len`
if they're consistently much shorter or longer.

### 2. New Tauri command: `export_srt`

Parallel structure to the existing `export_history` (same save-dialog pattern
via `tauri_plugin_dialog`, same `sanitize_filename`), but takes segment-bearing
items instead of plain text items:

```rust
#[tauri::command]
async fn export_srt(
    app: AppHandle,
    state: State<'_, AppState>,
    items: Vec<Vec<TimedSegment>>,  // one Vec per source file, in order
    suggested_name: Option<String>,
) -> Result<String, String>
```

Mirrors `export_history`'s early-return guard (`lib.rs:1024-1026`): if `items`
is empty, or every inner `Vec` is empty (zero cues total), return a clear
Hebrew error instead of writing a 0-byte/malformed SRT. Must also be added to
the `tauri::generate_handler!` command list alongside the other commands
(`lib.rs:1671-1684`).

For a single-item call, `items.len() == 1`. For the combined export, the
frontend passes all done items' segments in order; the backend computes each
file's total duration (`last cue's end_ms`, or 0 if empty) and adds it as a
running offset to every subsequent file's cue timestamps before writing one
continuous, correctly-numbered SRT.

Writes standard SRT: sequential 1-based index, `HH:MM:SS,mmm --> HH:MM:SS,mmm`
(comma, not period, per the SRT spec), text, blank line. UTF-8, no RLM/bidi
control characters — modern SRT consumers (VLC, Premiere, YouTube) handle
Hebrew RTL correctly on their own; this is unrelated to the burned-in-caption
RTL rendering rules used elsewhere in the video pipeline (that's a pixel/canvas
rendering concern, this is plain text).

### 3. Frontend (`App.tsx`)

- `BatchResult` gains `segments?: TimedSegment[]`, populated from the (now
  richer) `transcribe_file` response alongside the existing `transcript` string.
- A third **"SRT"** button appears next to the existing TXT/DOCX buttons:
  - Per-item (in each result card) → calls `export_srt` with that one item's
    segments, suggested name = `firstWordsName(transcript)`.
  - Combined action bar (existing "יצא משולב" pattern) → calls `export_srt`
    with all `done` items' segments in order, suggested name = same
    `generateExportName` used for combined TXT/DOCX today.
- The SRT button is hidden/disabled when `segments` is empty or absent
  (defensive — protects a future route that might not populate timing).
- **Edited-transcript desync:** the result card's `<textarea>` already lets the
  user hand-edit `result.transcript` (`onChange` writes straight to
  `transcript`); `segments` are derived from the original ASR output and are
  never re-synced to those edits. Exporting SRT after an edit would silently
  ship stale (pre-edit) text/timing while TXT/DOCX would reflect the edit.
  Fix: set a per-item `edited: true` flag the first time the textarea's
  `onChange` fires, and treat that the same as "no segments" — hide/disable
  the SRT button for that item once edited (TXT/DOCX stay unaffected, since
  they already export the live `transcript` string). Out of scope: re-timing
  edited text — not possible without re-running ASR.

## Testing

**Rust unit tests (pure functions, TDD):**
- `chunk_words_to_cues`: empty input, single word, exactly at the word/time
  threshold, one word alone exceeding `max_ms`.
- SRT timestamp formatter: `ms → HH:MM:SS,mmm` at 0, sub-second, >1h boundary.
- Combined-offset accumulation: 2–3 synthetic file segment-lists → correct
  cumulative start/end times and continuous cue numbering.

**Manual runtime verification (Henry, `npm run tauri dev`):**
- Local whisper batch on a real Hebrew recording → export SRT → open in VLC,
  confirm captions are in sync and readable (not too long/short per cue).
- Cloud Deepgram batch → same check.
- Combined export of 2 files → SRT with correct offset for the second file's
  cues.
- Confirm local whisper accuracy/speed is not visibly worse with
  `no_timestamps(false)`.

## Out of scope (this version)

- SRT for short-dictation history (no associated audio file).
- Matching the SRT filename to the original video/audio filename.
- Configurable chunking (word/duration thresholds are fixed constants for v1).
- VTT or other subtitle formats.
