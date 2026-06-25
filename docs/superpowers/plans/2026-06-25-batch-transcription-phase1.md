# Batch Transcription — Phase 1 (MVP) Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an additive "file upload → transcribe" pipeline to Hebrew Dictation: pick an audio file → transcribe it (cloud Deepgram **or** local whisper, offline) → editable RTL textarea → export TXT/DOCX, inject, or copy. No change to the existing short-dictation path.

**Architecture:** A new `decode.rs` (pure-Rust symphonia 0.6 + rubato 3 → 16 kHz mono f32, no ffmpeg) feeds either a new long-timeout Deepgram batch request **or** a new cancellable local whisper run. A thin `batch.rs` holds pure routing logic; the orchestration lives in two new Tauri commands in `lib.rs` (`transcribe_file`, `cancel_batch`) plus a `pick_audio_file` dialog command. Heavy work runs off the UI thread (`spawn_blocking`); the local run never holds the `whisper_engine` mutex during `state.full()`, so short dictation stays responsive. Progress + cancellation flow through a new `batch-progress` Tauri event and an `Arc<AtomicBool>` / `Notify` pair on `AppState`. A new React panel in the main view drives it.

**Tech Stack:** Rust / Tauri v2, `symphonia = "0.6"`, `rubato = "3"`, `whisper-rs = "0.16"` (existing), `reqwest` (existing), React/TypeScript frontend.

**Source of truth:** spec `docs/superpowers/specs/2026-06-22-batch-transcription-design.md` (rev2 — §14 resolutions are authoritative). This plan implements **Phase 1 only** (§10 Phase 1). Phases 2–4 (Groq chunking, save-next-to-file, long in-app recording, disk-sink hardening) are separate later plans.

**Scope guardrails (Phase 1):**
- Cloud batch = **Deepgram single sync request only**. Groq cloud + chunking = Phase 2. If the user is in cloud mode without a Deepgram key, return a clear Hebrew error pointing to settings or local mode.
- Recording a long meeting in-app = Phase 3. Phase 1 is **file upload only**.
- Manual export reuses the existing `export_history` command as-is (it carries a "היסטוריית תמלול" header — acceptable for MVP; the header-less `save_transcript_next_to` is Phase 2).
- **Errors are Hebrew `String`s with a `"בוטל"` cancel sentinel** (mirrors the existing `export_history` `"הייצוא בוטל"` pattern). A dedicated `BatchError` enum (spec §14.2-H) is intentionally **deferred** — every batch error just needs a distinct Hebrew message, and the codebase already returns `Result<_, String>` at every command boundary. Revisit if Phase 2 error handling grows. (Reviewer recommended the enum; this is a deliberate YAGNI deviation for the MVP.)

---

## File Structure

| File | Phase-1 responsibility |
|---|---|
| `src-tauri/Cargo.toml` | **Modify** — add `symphonia` (8 features) + `rubato = "3"`. |
| `src-tauri/capabilities/default.json` | **Modify** — add `"dialog:allow-open"` (file picker). |
| `src-tauri/src/decode.rs` | **Create** — `decode_file_to_16k_mono(path, cancel, on_progress) -> Result<Vec<f32>, String>` (symphonia decode + mono mixdown + rubato resample) + unit tests. |
| `src-tauri/src/api_transcribe.rs` | **Modify** — expose `samples_to_wav` as `pub(crate)`; add `pub(crate) async fn transcribe_deepgram_batch(client, samples, key, lang)` (reuses in-module helpers, paragraph-formatted transcript, timeout on the injected client). Short-dictation fns untouched. |
| `src-tauri/src/whisper.rs` | **Modify** — add `WhisperEngine::create_long_state()` + free fn `run_long_transcription(state, model_name, samples, lang, cancel, on_progress)` (cancellable, progress, no 180s timeout, no lock held). Existing `transcribe()` untouched. |
| `src-tauri/src/batch.rs` | **Create** — `BatchOpts`, `CANCELLED` sentinel, `pick_batch_route()` + unit test. Pure logic only. |
| `src-tauri/src/lib.rs` | **Modify** — `mod decode; mod batch;`; add `AppState` fields (`batch_cancel`, `batch_cancel_notify`, `batch_in_progress`); add commands `transcribe_file`, `cancel_batch`, `pick_audio_file`; register them in `invoke_handler!`. |
| `src/App.tsx` | **Modify** — batch state + `batch-progress` listener + a "תמלול קובץ" panel in the main view + handlers (reusing `injectText`, `export_history`). |
| `src/App.css` | **Modify** — styles for the batch panel / progress bar / textarea. |

**Build/test commands** (run from `src-tauri/` for Rust, repo root for the app):
- Rust tests: `cargo test` (in `src-tauri/`)
- Rust compile check: `cargo build` (in `src-tauri/`)
- App dev run (manual verification): `npm run tauri dev` (repo root)

---

## Chunk 1: Foundations — deps, capability, decode.rs

### Task 1.1: Add `symphonia` + `rubato` dependencies

**Files:**
- Modify: `src-tauri/Cargo.toml` (dependencies section, after `chrono` line ~45)

- [ ] **Step 1: Verify the versions resolve**

Run (in `src-tauri/`):
```bash
cargo add symphonia --features mp3,aac,isomp4,alac,vorbis,ogg,wav,flac --dry-run
cargo add rubato --dry-run
```
Expected: both resolve to `symphonia 0.6.x` and `rubato 3.x`. If `symphonia` resolves to 0.5.x, STOP — the decode code below is written for the 0.6 rewrite API and will not compile against 0.5. (Verified 2026-06-25: symphonia 0.6.0 published 2026-05-15, rubato 3.0.0 published 2026-05-20.)

- [ ] **Step 2: Add the dependencies**

Add to `src-tauri/Cargo.toml` `[dependencies]`:
```toml
# Pure-Rust audio decode (file upload → 16kHz mono f32). No ffmpeg.
# aac + isomp4 are OFF by default and are REQUIRED for iPhone .m4a (AAC-LC in MP4).
symphonia = { version = "0.6", features = ["mp3", "aac", "isomp4", "alac", "vorbis", "ogg", "wav", "flac"] }
# Resample native rate → 16000 Hz. v3 = full rewrite (Fft, audioadapter, Indexing flush).
rubato = "3"
```

- [ ] **Step 3: Confirm it builds**

Run (in `src-tauri/`): `cargo build`
Expected: compiles (downloads the new crates). No code uses them yet, so only the dep graph changes.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "build(batch): add symphonia 0.6 + rubato 3 for audio file decode"
```

---

### Task 1.2: Add the `dialog:allow-open` capability

**Files:**
- Modify: `src-tauri/capabilities/default.json`

The file picker (`pick_audio_file`, Task 2.5) uses `tauri-plugin-dialog`'s open dialog. Only `dialog:allow-save` is currently granted — without `allow-open` the picker silently fails in production (spec §14.1-C).

- [ ] **Step 1: Add the permission**

In `src-tauri/capabilities/default.json`, change the `permissions` array — add `"dialog:allow-open"` after `"dialog:allow-save"`:
```json
    "notification:default",
    "dialog:allow-save",
    "dialog:allow-open"
```

- [ ] **Step 2: Confirm the schema accepts it**

Run (in `src-tauri/`): `cargo build`
Expected: compiles (Tauri validates capability files at build time). If it errors that `dialog:allow-open` is unknown, check the exact permission id in `src-tauri/gen/schemas/desktop-schema.json` and use the listed open-dialog permission.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/capabilities/default.json
git commit -m "feat(batch): grant dialog:allow-open for the audio file picker"
```

---

### Task 1.3: Expose `samples_to_wav` as `pub(crate)`

**Files:**
- Modify: `src-tauri/src/api_transcribe.rs:7`

`decode.rs` tests build WAV fixtures with this helper, and the Deepgram batch path (Task 2.1) reuses it. It is currently private.

- [ ] **Step 1: Change visibility**

In `src-tauri/src/api_transcribe.rs`, line 7, change:
```rust
fn samples_to_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
```
to:
```rust
pub(crate) fn samples_to_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
```

- [ ] **Step 2: Confirm it builds**

Run (in `src-tauri/`): `cargo build`
Expected: compiles (a `dead_code` warning is fine until `decode.rs` uses it).

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/api_transcribe.rs
git commit -m "refactor(api): expose samples_to_wav as pub(crate) for batch reuse"
```

---

### Task 1.4: Create `decode.rs` — decode + resample to 16 kHz mono (TDD)

**Files:**
- Create: `src-tauri/src/decode.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod decode;` near the other `mod` lines, ~line 4)
- Test: inline `#[cfg(test)]` module in `decode.rs`

> **API note:** symphonia 0.6 and rubato 3 are post-rewrite. The 0.5-style `AudioBufferRef`/`SampleBuffer`/`get_probe().format()`/`FftFixedIn`/`process_partial` APIs are GONE. Code below was verified against docs.rs 0.6.0 / 3.0.0 (see citations). The exact `use` paths (especially the `Audio` trait providing `.spec()`/`.frames()`) may need a one-line adjustment that `cargo check` will point out — fix per the compiler if so.

- [ ] **Step 1: Register the module**

In `src-tauri/src/lib.rs`, add to the `mod` block (after `mod audio;`):
```rust
mod decode;
```

- [ ] **Step 2: Write the failing tests**

Create `src-tauri/src/decode.rs` with ONLY the test module first (so the test fails to compile → then we add the impl):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_transcribe::samples_to_wav;
    use std::sync::atomic::AtomicBool;

    fn write_temp_wav(name: &str, samples: &[f32], rate: u32) -> std::path::PathBuf {
        let wav = samples_to_wav(samples, rate);
        let mut p = std::env::temp_dir();
        p.push(format!("hd_decode_{}.wav", name));
        std::fs::write(&p, wav).unwrap();
        p
    }

    #[test]
    fn passthrough_16k_mono() {
        let samples: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let p = write_temp_wav("16k", &samples, 16_000);
        let cancel = AtomicBool::new(false);
        let out = decode_file_to_16k_mono(&p, &cancel, |_| {}).unwrap();
        // ~1s of 16k audio; small tolerance for decoder framing.
        assert!((out.len() as i64 - 16_000).abs() < 200, "got {}", out.len());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn downsample_48k_to_16k_length() {
        let samples: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.02).sin() * 0.5).collect();
        let p = write_temp_wav("48k", &samples, 48_000);
        let cancel = AtomicBool::new(false);
        let out = decode_file_to_16k_mono(&p, &cancel, |_| {}).unwrap();
        // 1s @48k → ~16000 @16k, within resampler latency tolerance.
        assert!((out.len() as i64 - 16_000).abs() < 800, "got {}", out.len());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_file_errors() {
        let mut p = std::env::temp_dir();
        p.push("hd_decode_garbage.bin");
        std::fs::write(&p, b"not audio at all").unwrap();
        let cancel = AtomicBool::new(false);
        assert!(decode_file_to_16k_mono(&p, &cancel, |_| {}).is_err());
        let _ = std::fs::remove_file(&p);
    }
}
```

- [ ] **Step 3: Run the tests — expect a COMPILE failure**

Run (in `src-tauri/`): `cargo test decode::tests`
Expected: FAIL — `cannot find function decode_file_to_16k_mono`.

- [ ] **Step 4: Implement `decode.rs` (above the test module)**

Prepend to `src-tauri/src/decode.rs`:
```rust
//! Decode an arbitrary audio file to 16 kHz mono f32 (the format whisper-rs and
//! the WAV-for-API path both consume). Pure Rust: symphonia 0.6 decode +
//! rubato 3 resample. No ffmpeg. Cancellable per-packet; reports 0–100 progress.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use symphonia::core::audio::GenericAudioBufferRef; // .spec()/.frames()/.copy_to_vec_interleaved are inherent — do NOT import the `Audio` trait (unused → warning → fails under -D warnings)
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

const TARGET_RATE: u32 = 16_000;

/// Public entry point: file path → 16 kHz mono f32.
pub fn decode_file_to_16k_mono(
    path: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u8),
) -> Result<Vec<f32>, String> {
    let (mono, native_rate) = decode_file_to_mono_f32(path, cancel, &mut on_progress)?;
    if mono.is_empty() {
        return Ok(mono);
    }
    resample_to_16k(&mono, native_rate)
}

/// Decode to mono f32 at the file's native sample rate.
fn decode_file_to_mono_f32(
    path: &Path,
    cancel: &AtomicBool,
    on_progress: &mut impl FnMut(u8),
) -> Result<(Vec<f32>, u32), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("פתיחת הקובץ נכשלה: {}", e))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    // symphonia 0.6: Probe::probe (NOT format); options by value; returns the reader.
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|_| "פורמט אודיו לא נתמך או קובץ פגום".to_string())?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| "לא נמצא ערוץ אודיו בקובץ".to_string())?;
    let track_id = track.id;
    let total_frames: Option<u64> = track.num_frames; // may be None (VBR/streamed)

    let audio_params = match track.codec_params.as_ref() {
        Some(CodecParameters::Audio(a)) => a,
        _ => return Err("הקובץ אינו אודיו נתמך".to_string()),
    };
    let native_rate = audio_params
        .sample_rate
        .ok_or_else(|| "קצב דגימה לא ידוע".to_string())?;

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(audio_params, &AudioDecoderOptions::default())
        .map_err(|_| "הקודק של הקובץ לא נתמך (ייתכן HE-AAC — נתמך רק AAC-LC)".to_string())?;

    let mut mono: Vec<f32> = Vec::new();
    let mut scratch: Vec<f32> = Vec::new();
    let mut decoded_frames: u64 = 0;
    let mut last_pct: u8 = 0;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("בוטל".to_string());
        }
        // EOF in 0.6 = Ok(None), NOT an UnexpectedEof IoError.
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(Error::ResetRequired) => {
                return Err("הקובץ דורש reset של הזרם — לא נתמך".to_string())
            }
            Err(e) => return Err(format!("קריאת הקובץ נכשלה: {}", e)),
        };
        if packet.track_id != track_id {
            // NOTE: track_id is a public FIELD on symphonia 0.6 Packet, not a method.
            continue;
        }
        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                let frames = audio_buf.frames() as u64;
                let chans = audio_buf.spec().channels().count().max(1);
                scratch.clear();
                // Handles S16/S32/F32/... internally → interleaved f32.
                audio_buf.copy_to_vec_interleaved::<f32>(&mut scratch);
                if chans == 1 {
                    mono.extend_from_slice(&scratch);
                } else {
                    mono.reserve(scratch.len() / chans);
                    for frame in scratch.chunks_exact(chans) {
                        let sum: f32 = frame.iter().copied().sum();
                        mono.push(sum / chans as f32);
                    }
                }
                decoded_frames += frames;
                if let Some(total) = total_frames {
                    if total > 0 {
                        let pct = ((decoded_frames * 100) / total).min(100) as u8;
                        if pct != last_pct {
                            last_pct = pct;
                            on_progress(pct);
                        }
                    }
                }
            }
            // Per-packet decode/IO hiccups are recoverable — skip and continue.
            Err(Error::DecodeError(_)) => continue,
            Err(Error::IoError(_)) => continue,
            Err(e) => return Err(format!("פענוח נכשל: {}", e)),
        }
    }
    Ok((mono, native_rate))
}

/// Resample mono f32 to 16 kHz. Short-circuits when already 16 kHz.
fn resample_to_16k(mono: &[f32], src_rate: u32) -> Result<Vec<f32>, String> {
    if src_rate == TARGET_RATE {
        return Ok(mono.to_vec());
    }
    if mono.is_empty() {
        return Ok(Vec::new());
    }

    const CHUNK: usize = 1024;
    let mut resampler = Fft::<f32>::new(
        src_rate as usize,
        TARGET_RATE as usize,
        CHUNK,
        2, // sub_chunks
        1, // mono
        FixedSync::Input,
    )
    .map_err(|e| format!("אתחול resampler נכשל: {}", e))?;

    let out_cap = (mono.len() as u64 * TARGET_RATE as u64 / src_rate as u64) as usize
        + resampler.output_frames_max()
        + 64;
    let mut out = vec![0.0f32; out_cap];

    let input =
        InterleavedSlice::new(mono, 1, mono.len()).map_err(|e| format!("input adapter: {}", e))?;
    let mut output = InterleavedSlice::new_mut(&mut out, 1, out_cap)
        .map_err(|e| format!("output adapter: {}", e))?;

    let mut idx = Indexing {
        input_offset: 0,
        output_offset: 0,
        active_channels_mask: None,
        partial_len: None,
    };
    let mut remaining = mono.len();
    let mut written = 0usize;
    let mut need = resampler.input_frames_next();
    while remaining >= need {
        let (nin, nout) = resampler
            .process_into_buffer(&input, &mut output, Some(&idx))
            .map_err(|e| format!("resample: {}", e))?;
        idx.input_offset += nin;
        idx.output_offset += nout;
        remaining -= nin;
        written += nout;
        need = resampler.input_frames_next();
    }
    // Final partial block (remainder treated as silence).
    if remaining > 0 {
        idx.partial_len = Some(remaining);
        let (nin, nout) = resampler
            .process_into_buffer(&input, &mut output, Some(&idx))
            .map_err(|e| format!("resample tail: {}", e))?;
        idx.input_offset += nin;
        idx.output_offset += nout;
        written += nout;
    }
    // Flush the resampler's internal latency, else the trailing audio is dropped.
    let delay = resampler.output_delay();
    idx.partial_len = Some(0);
    while written < idx.output_offset + delay
        && idx.output_offset + resampler.output_frames_max() <= out_cap
    {
        let (_nin, nout) = resampler
            .process_into_buffer(&input, &mut output, Some(&idx))
            .map_err(|e| format!("resample flush: {}", e))?;
        if nout == 0 {
            break;
        }
        idx.output_offset += nout;
        written += nout;
    }
    out.truncate(written);
    Ok(out)
}
```

API citations (verify with `cargo check` if a `use` path moved):
`Probe::probe` + `FormatReader::{next_packet→Ok(None), default_track(TrackType)}` + `Track.num_frames`: docs.rs/symphonia-core/0.6.0 formats module · `GenericAudioBufferRef::copy_to_vec_interleaved`/`.spec()`/`.frames()`: docs.rs/symphonia-core/0.6.0/symphonia_core/audio · `make_audio_decoder`: docs.rs/symphonia-core/0.6.0 codecs/registry · rubato `Fft::new`/`FixedSync`/`Indexing.partial_len`/`output_delay`: docs.rs/rubato/3.0.0.

- [ ] **Step 5: Run the tests — expect PASS**

Run (in `src-tauri/`): `cargo test decode::tests`
Expected: 3 tests PASS. If `passthrough`/`downsample` lengths are off, adjust tolerances; if a `use` path errors, fix per the compiler and re-run.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/decode.rs src-tauri/src/lib.rs
git commit -m "feat(batch): decode.rs — symphonia 0.6 + rubato 3 → 16kHz mono f32 (TDD)"
```

---

### Task 1.5: Manual decode smoke with a real .m4a (no automated test)

iPhone `.m4a` (AAC-LC in MP4) is the highest-risk format (needs both `aac` + `isomp4` features). Verify once manually before building on top.

- [ ] **Step 1: Add a temporary smoke test**

Temporarily add to `decode.rs` tests (point it at a real file you have):
```rust
#[test]
#[ignore]
fn smoke_real_m4a() {
    let p = std::path::PathBuf::from(r"C:\Users\אורח\Downloads\sample.m4a"); // a real iPhone voice memo
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let out = decode_file_to_16k_mono(&p, &cancel, |pct| eprintln!("decode {}%", pct)).unwrap();
    assert!(out.len() > 16_000, "decoded {} samples", out.len());
}
```

- [ ] **Step 2: Run it**

Run (in `src-tauri/`): `cargo test decode::tests::smoke_real_m4a -- --ignored --nocapture`
Expected: PASS, progress prints, sample count ≈ duration_seconds × 16000. If it errors "פורמט לא נתמך", the `isomp4` feature is missing; if "קודק לא נתמך", the file is HE-AAC (expected — surface the clear error).

- [ ] **Step 3: Remove the smoke test, commit nothing (or commit its removal)**

Delete the `smoke_real_m4a` test (it hard-codes a local path). No commit needed if you only added/removed it within this task.

---

## Chunk 2: Backend transcription — Deepgram batch, local long run, orchestration

### Task 2.1: Add `transcribe_deepgram_batch` to `api_transcribe.rs`

**Files:**
- Modify: `src-tauri/src/api_transcribe.rs` (add after `transcribe_deepgram_inner`, ~line 245)

A network call — no unit test (covered by manual smoke in Task 3.6). It reuses the in-module private `samples_to_wav`/`classify_status`/`classify_request_error` (same module, no extra visibility needed). The 900 s timeout lives on the **client** passed by the caller (the short path keeps its own 30 s client). Always Hebrew-monolingual (`language=he`) per spec §14.1-A — `multi` is NOT passed.

- [ ] **Step 1: Add the function**

```rust
/// Long-file Deepgram request for batch transcription. The caller supplies a
/// client with a long (e.g. 900s) timeout; this fn does NOT set its own timeout.
/// Uses paragraph formatting for readable long-meeting output, falling back to the
/// flat transcript. `language` should be "he" (Deepgram nova-3 multilingual does
/// NOT include Hebrew — see spec §14.1-A; never pass "multi" for Hebrew).
pub(crate) async fn transcribe_deepgram_batch(
    client: &reqwest::Client,
    samples: &[f32],
    api_key: &str,
    language: &str,
) -> Result<String, ApiError> {
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

    Ok(transcript)
}
```

- [ ] **Step 2: Confirm it builds**

Run (in `src-tauri/`): `cargo build`
Expected: compiles (a `dead_code` warning until `lib.rs` calls it is fine).

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/api_transcribe.rs
git commit -m "feat(batch): transcribe_deepgram_batch — long-file Deepgram request (he-only, injectable client)"
```

---

### Task 2.2: Add cancellable local long-run to `whisper.rs`

**Files:**
- Modify: `src-tauri/src/whisper.rs`

Add a way to create a fresh decode state (so the caller locks the engine only briefly) and a free fn to run a long, cancellable transcription with progress and **no 180 s timeout** — without holding the `AppState` mutex (spec §14.1-D). The existing `transcribe()` is untouched (short dictation keeps its timeout).

Hard to unit-test without a model file → verification is `cargo build` + the manual smoke in Task 3.6.

- [ ] **Step 1: Add imports + the new code**

At the top of `whisper.rs`, **REPLACE the existing `use whisper_rs::{...};` line (line 4)** with the version below (it just adds `WhisperState`), and ADD the two new `std::sync` lines. Do **NOT** add a second `whisper_rs` use line — a duplicate import is a hard error (E0252):
```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};
```
(Keep the existing `use std::path::Path;`, `use std::sync::mpsc;`, `use std::time::Duration;` — the short path still uses them.)

Add a method on `WhisperEngine` (inside the existing `impl WhisperEngine`):
```rust
    /// Create a fresh per-run state for a long transcription, returning it plus the
    /// model name. The caller locks the engine only long enough to call this, then
    /// runs `run_long_transcription` on the returned state OFF the AppState lock —
    /// so a multi-hour batch never blocks short dictation / model management.
    pub fn create_long_state(&self) -> Result<(WhisperState, String), String> {
        let state = self
            .ctx
            .create_state()
            .map_err(|e| format!("Failed to create whisper state: {}", e))?;
        Ok((state, self.model_name.clone()))
    }
```

Add a free function at the end of `whisper.rs` (outside the `impl`):
```rust
/// Run a long, cancellable transcription on a pre-created state. Holds NO external
/// lock (the caller already dropped the engine mutex). No fixed timeout — stops via
/// `cancel` (whisper.cpp polls the abort callback before each compute step).
/// `on_progress` receives overall percent 0–100 (whisper-rs progress is percent,
/// NOT per-segment — spec §14.1-E).
pub fn run_long_transcription<F: FnMut(i32) + 'static>(
    mut state: WhisperState,
    model_name: &str,
    samples: &[f32],
    language: &str,
    cancel: Arc<AtomicBool>,
    on_progress: F,
) -> Result<String, String> {
    // ivrit.ai models require the language token forced to Hebrew.
    let effective_lang = if model_name.starts_with("ivrit-") {
        "he".to_string()
    } else {
        language.to_string()
    };

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    if effective_lang == "auto" {
        params.set_language(None);
    } else {
        params.set_language(Some(&effective_lang));
    }
    params.set_translate(false);
    params.set_no_timestamps(true);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);

    params.set_progress_callback_safe(on_progress);
    let cancel_for_abort = cancel.clone();
    params.set_abort_callback_safe(move || cancel_for_abort.load(Ordering::Relaxed));

    let full_res = state.full(params, samples);
    // If the user cancelled, report it cleanly regardless of how full() returned.
    if cancel.load(Ordering::Relaxed) {
        return Err("בוטל".to_string());
    }
    full_res.map_err(|e| format!("Transcription failed: {}", e))?;

    let n = state.full_n_segments();
    let mut text = String::new();
    for i in 0..n {
        if let Some(segment) = state.get_segment(i) {
            if let Ok(s) = segment.to_str_lossy() {
                text.push_str(&s);
            }
        }
    }
    Ok(text.trim().to_string())
}
```

> Verify during `cargo build`: that `whisper_rs::WhisperState` has no lifetime param (the existing `transcribe()` already moves a state into a `std::thread::spawn`, proving it is `'static + Send`), and that `set_progress_callback_safe` / `set_abort_callback_safe` exist on `FullParams` in 0.16 (confirmed via docs.rs/whisper-rs/0.16.0). If `set_language(Some(&effective_lang))` complains about lifetimes, keep `effective_lang` alive until after `state.full(...)` (it already is).

- [ ] **Step 2: Confirm it builds**

Run (in `src-tauri/`): `cargo build`
Expected: compiles. Warnings about unused `run_long_transcription`/`create_long_state` are fine until Task 2.4 wires them.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/whisper.rs
git commit -m "feat(batch): whisper run_long_transcription — cancellable, progress, no-timeout, lock-free"
```

---

### Task 2.3: Create `batch.rs` — routing + opts (TDD)

**Files:**
- Create: `src-tauri/src/batch.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod batch;`)
- Test: inline `#[cfg(test)]` in `batch.rs`

- [ ] **Step 1: Register the module**

In `src-tauri/src/lib.rs`, add (after `mod audio;`/`mod decode;`):
```rust
mod batch;
```

- [ ] **Step 2: Write the failing test**

Create `src-tauri/src/batch.rs`:
```rust
//! Pure, testable batch-transcription routing + options. Orchestration (decode,
//! cloud/local dispatch, progress, cancel) lives in lib.rs where AppState is reachable.

use serde::Deserialize;

/// Options sent from the frontend for a batch transcription.
#[derive(Debug, Clone, Deserialize)]
pub struct BatchOpts {
    /// "cloud" | "local". Defaults from the user's transcription mode on the UI side.
    pub mode: String,
    #[serde(default = "default_language")]
    pub language: String,
    /// Reserved for a future "inject on completion" toggle; the UI handles inject in Phase 1.
    #[serde(default)]
    pub inject: bool,
}

fn default_language() -> String {
    "he".to_string()
}

/// Sentinel error string for user cancellation. The frontend shows it as a calm
/// notice, NOT an error toast (mirrors export_history's "הייצוא בוטל").
pub const CANCELLED: &str = "בוטל";

/// Phase 1 routing: cloud → Deepgram single request; local → whisper.
/// (Groq cloud + chunking is Phase 2.)
#[derive(Debug, PartialEq, Eq)]
pub enum BatchRoute {
    CloudDeepgram,
    Local,
}

pub fn pick_batch_route(mode: &str) -> BatchRoute {
    match mode {
        "local" => BatchRoute::Local,
        _ => BatchRoute::CloudDeepgram,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_local_and_cloud() {
        assert_eq!(pick_batch_route("local"), BatchRoute::Local);
        assert_eq!(pick_batch_route("cloud"), BatchRoute::CloudDeepgram);
        // Unknown/empty mode defaults to cloud.
        assert_eq!(pick_batch_route("whatever"), BatchRoute::CloudDeepgram);
    }
}
```

- [ ] **Step 3: Run the test — expect PASS (it includes the impl)**

Run (in `src-tauri/`): `cargo test batch::tests`
Expected: PASS. (This task's "failing" state is the missing module; once created, the test passes immediately.)

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/batch.rs src-tauri/src/lib.rs
git commit -m "feat(batch): batch.rs — BatchOpts + pick_batch_route + CANCELLED sentinel (TDD)"
```

---

### Task 2.4: Wire `AppState` + `transcribe_file` + `cancel_batch` in `lib.rs`

**Files:**
- Modify: `src-tauri/src/lib.rs` (AppState struct ~line 26; `.manage(...)` init ~line 1092; new commands; `invoke_handler!` ~line 1234)

- [ ] **Step 1: Add `AppState` fields**

In the `struct AppState { ... }` (lib.rs ~line 26), add:
```rust
    /// Set true to abort the in-flight batch (decode + local whisper read it; the
    /// cloud path races against `batch_cancel_notify`).
    batch_cancel: Arc<AtomicBool>,
    /// Wakes the cloud request's `select!` so cancel drops the in-flight HTTP future.
    batch_cancel_notify: Arc<tokio::sync::Notify>,
    /// Guards against two concurrent batch jobs.
    batch_in_progress: Arc<AtomicBool>,
```
(`Arc`, `AtomicBool`, `Ordering` are already imported at lib.rs:13-14.)

- [ ] **Step 2: Initialize them in `.manage(...)`**

In the `AppState { ... }` literal inside `.manage(...)` (~line 1092), add:
```rust
                batch_cancel: Arc::new(AtomicBool::new(false)),
                batch_cancel_notify: Arc::new(tokio::sync::Notify::new()),
                batch_in_progress: Arc::new(AtomicBool::new(false)),
```

- [ ] **Step 3: Add the commands**

Add near the other transcription commands in `lib.rs` (e.g. after `enhance_text`, ~line 310):
```rust
#[tauri::command]
async fn transcribe_file(
    app: AppHandle,
    state: State<'_, AppState>,
    file_path: String,
    opts: batch::BatchOpts,
) -> Result<String, String> {
    // One batch at a time.
    if state.batch_in_progress.swap(true, Ordering::SeqCst) {
        return Err("תמלול ארוך כבר רץ — המתן לסיומו או בטל אותו".to_string());
    }
    state.batch_cancel.store(false, Ordering::SeqCst);
    let result = run_transcribe_file(&app, &state, file_path, opts).await;
    state.batch_in_progress.store(false, Ordering::SeqCst);
    result
}

async fn run_transcribe_file(
    app: &AppHandle,
    state: &State<'_, AppState>,
    file_path: String,
    opts: batch::BatchOpts,
) -> Result<String, String> {
    // 0) Fail fast: cloud mode needs a Deepgram key — check BEFORE the (possibly long)
    //    decode so the user isn't made to wait only to learn a key is missing.
    if matches!(batch::pick_batch_route(&opts.mode), batch::BatchRoute::CloudDeepgram) {
        let has_key = {
            let s = state.settings.lock().map_err(|e| e.to_string())?;
            s.deepgram_api_key.as_ref().is_some_and(|k| !k.is_empty())
        };
        if !has_key {
            return Err("תמלול ענן ארוך דורש מפתח Deepgram. הוסף אותו בהגדרות, או עבור למצב \"פרטי (במכשיר)\".".to_string());
        }
    }

    // 1) Decode → 16kHz mono f32, off the UI thread, with progress + cancel.
    let cancel = state.batch_cancel.clone();
    let app_dec = app.clone();
    let path = std::path::PathBuf::from(&file_path);
    let samples = tokio::task::spawn_blocking(move || {
        decode::decode_file_to_16k_mono(&path, &cancel, |pct| {
            let _ = app_dec.emit(
                "batch-progress",
                serde_json::json!({ "stage": "decoding", "pct": pct }),
            );
        })
    })
    .await
    .map_err(|e| format!("שגיאת משימת פענוח: {}", e))??;

    if state.batch_cancel.load(Ordering::SeqCst) {
        return Err(batch::CANCELLED.to_string());
    }
    if samples.is_empty() {
        return Err("הקובץ ריק או פגום — לא נמצא אודיו לתמלול".to_string());
    }
    // Batch-specific guard (spec §14.2-N): a valid-format but near-silent file would
    // otherwise hit a generic API 400. Reuse the existing detector but with a batch
    // message — NOT the mic-permission message (this is a file, not the live mic).
    if audio::is_effectively_silent(&samples, 0.01) {
        return Err("לא נמצא דיבור בקובץ (שקט) — ודא שהקובץ מכיל אודיו מדובר.".to_string());
    }

    // 2) Route.
    match batch::pick_batch_route(&opts.mode) {
        batch::BatchRoute::CloudDeepgram => {
            let key = {
                let s = state.settings.lock().map_err(|e| e.to_string())?;
                s.deepgram_api_key.clone()
            };
            let key = key.filter(|k| !k.is_empty()).ok_or_else(|| {
                "תמלול ענן ארוך דורש מפתח Deepgram. הוסף אותו בהגדרות, או עבור למצב \"פרטי (במכשיר)\".".to_string()
            })?;

            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(900))
                .build()
                .map_err(|e| format!("שגיאת לקוח רשת: {}", e))?;

            let _ = app.emit(
                "batch-progress",
                serde_json::json!({ "stage": "transcribing", "pct": 0 }),
            );

            let notify = state.batch_cancel_notify.clone();
            let fut = api_transcribe::transcribe_deepgram_batch(&client, &samples, &key, &opts.language);
            let text = tokio::select! {
                r = fut => r.map_err(|e| e.to_string())?,
                _ = notify.notified() => return Err(batch::CANCELLED.to_string()),
            };
            let _ = app.emit("batch-progress", serde_json::json!({ "stage": "done", "pct": 100 }));
            Ok(text)
        }
        batch::BatchRoute::Local => {
            // Lock the engine ONLY to create a fresh state, then drop it so the
            // multi-hour run never blocks short dictation / model management.
            let (wstate, model_name) = {
                let guard = state.whisper_engine.lock().map_err(|e| e.to_string())?;
                match guard.as_ref() {
                    Some(e) => e.create_long_state()?,
                    None => {
                        return Err("המודל המקומי לא טעון — הורד מודל Whisper בהגדרות לפני תמלול מקומי".to_string())
                    }
                }
            };

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<i32>();
            let app_prog = app.clone();
            let progress_task = tokio::spawn(async move {
                while let Some(p) = rx.recv().await {
                    let _ = app_prog.emit(
                        "batch-progress",
                        serde_json::json!({ "stage": "transcribing", "pct": p }),
                    );
                }
            });

            let cancel = state.batch_cancel.clone();
            let lang = opts.language.clone();
            let samples_owned = samples;
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
        }
    }
}

#[tauri::command]
fn cancel_batch(state: State<AppState>) -> Result<(), String> {
    state.batch_cancel.store(true, Ordering::SeqCst);
    state.batch_cancel_notify.notify_waiters();
    Ok(())
}
```

- [ ] **Step 4: Register the commands**

In `invoke_handler![ ... ]` (~line 1234), add:
```rust
            transcribe_file,
            cancel_batch,
```

- [ ] **Step 5: Build**

Run (in `src-tauri/`): `cargo build`
Expected: compiles. Resolve any `use` / type errors per the compiler.

- [ ] **Step 6: Run the full test suite (no regressions)**

Run (in `src-tauri/`): `cargo test`
Expected: existing tests + `decode::tests` + `batch::tests` all PASS.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat(batch): transcribe_file + cancel_batch commands; AppState cancel/in-progress wiring"
```

---

### Task 2.5: Add the `pick_audio_file` dialog command

**Files:**
- Modify: `src-tauri/src/lib.rs` (add near `export_history`, ~line 620; register in `invoke_handler!`)

Mirrors `export_history`'s Rust-side dialog pattern (callback + oneshot) so we don't add a JS dialog dependency.

- [ ] **Step 1: Add the command**

```rust
/// Open a native file picker for an audio file. Returns the chosen path, or None
/// if the user cancelled. The path is opened Rust-side by symphonia in transcribe_file
/// (no fs-read capability needed — only dialog:allow-open).
#[tauri::command]
async fn pick_audio_file(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let (tx, rx) = tokio::sync::oneshot::channel::<Option<std::path::PathBuf>>();
    app.dialog()
        .file()
        .set_title("בחר קובץ אודיו לתמלול")
        .add_filter("אודיו", &["mp3", "m4a", "wav", "ogg", "flac", "aac", "mp4"])
        .pick_file(move |result| {
            let _ = tx.send(result.and_then(|fp| fp.into_path().ok()));
        });

    let path = rx
        .await
        .map_err(|_| "דיאלוג הבחירה נסגר ללא תגובה".to_string())?;
    Ok(path.map(|p| p.to_string_lossy().to_string()))
}
```

- [ ] **Step 2: Register it**

In `invoke_handler![ ... ]`, add:
```rust
            pick_audio_file,
```

- [ ] **Step 3: Build**

Run (in `src-tauri/`): `cargo build`
Expected: compiles. If `pick_file` signature differs in the installed `tauri-plugin-dialog`, adjust per the compiler (it mirrors `save_file` used in `export_history`).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat(batch): pick_audio_file dialog command"
```

---

## Chunk 3: Frontend — batch panel + wiring + manual verification

> The frontend has no unit-test harness (this is a Tauri app; tests live in `cargo test`). Frontend tasks are implement → **manual verification via `npm run tauri dev`** → commit.

### Task 3.1: Add a `stageLabel` helper + batch state

**Files:**
- Modify: `src/App.tsx`

- [ ] **Step 1: Add the stage label helper at module scope**

Near the top of `src/App.tsx` (after `const APP_VERSION = ...`):
```typescript
function stageLabel(stage: string): string {
  switch (stage) {
    case "decoding": return "מפענח אודיו…";
    case "uploading": return "מעלה…";
    case "transcribing": return "מתמלל…";
    case "done": return "הושלם";
    default: return "מעבד…";
  }
}
```

- [ ] **Step 2: Add batch state inside the `App` component**

Near the other `useState` declarations in `App` (e.g. after the history/export state, ~line 362):
```typescript
const [batchTranscript, setBatchTranscript] = useState("");
const [batchRunning, setBatchRunning] = useState(false);
const [batchStage, setBatchStage] = useState("");
const [batchPct, setBatchPct] = useState(0);
const [batchMode, setBatchMode] = useState<"cloud" | "local">("cloud");
const [batchError, setBatchError] = useState("");
```

- [ ] **Step 3: Default `batchMode` from the saved transcription mode**

In `initApp` (~line 699), where settings are unpacked into state, add:
```typescript
    setBatchMode(settings.transcription_mode === "local" ? "local" : "cloud");
```

- [ ] **Step 4: Build check (type only)**

Run (repo root): `npm run build` (or rely on the dev server's typecheck in Task 3.5)
Expected: no TypeScript errors. (Unused-var warnings until wired are fine.)

- [ ] **Step 5: Commit**

```bash
git add src/App.tsx
git commit -m "feat(batch-ui): batch panel state + stageLabel helper"
```

---

### Task 3.2: Subscribe to `batch-progress`

**Files:**
- Modify: `src/App.tsx`

- [ ] **Step 1: Add the listener `useEffect`**

Near the other `listen(...)` effects (~line 540–595):
```typescript
useEffect(() => {
  const unlisten = listen<{ stage: string; pct: number }>("batch-progress", (event) => {
    setBatchStage(event.payload.stage);
    setBatchPct(event.payload.pct ?? 0);
  });
  return () => {
    unlisten.then((fn) => fn());
  };
}, []);
```

- [ ] **Step 2: Commit**

```bash
git add src/App.tsx
git commit -m "feat(batch-ui): listen to batch-progress events"
```

---

### Task 3.3: Add the handlers

**Files:**
- Modify: `src/App.tsx`

- [ ] **Step 1: Add the handlers (near `exportHistory`/`injectText`, ~line 835)**

```typescript
const handlePickAndTranscribe = useCallback(async () => {
  setBatchError("");
  let filePath: string | null = null;
  try {
    filePath = await invoke<string | null>("pick_audio_file");
  } catch (e) {
    setBatchError(`בחירת הקובץ נכשלה: ${e}`);
    return;
  }
  if (!filePath) return; // user cancelled the picker
  setBatchRunning(true);
  setBatchPct(0);
  setBatchStage("decoding");
  setBatchTranscript("");
  try {
    const text = await invoke<string>("transcribe_file", {
      filePath,
      opts: { mode: batchMode, language: "he", inject: false },
    });
    setBatchTranscript(text);
    setBatchStage("done");
  } catch (e) {
    const msg = String(e);
    if (msg !== "בוטל") setBatchError(`התמלול נכשל: ${msg}`);
  } finally {
    setBatchRunning(false);
  }
}, [batchMode]);

const handleCancelBatch = useCallback(async () => {
  try {
    await invoke("cancel_batch");
  } catch {
    /* ignore */
  }
}, []);

const exportBatch = useCallback(async (format: "txt" | "docx") => {
  if (!batchTranscript.trim()) return;
  try {
    const items = [{ text: batchTranscript, timestamp: new Date().toISOString() }];
    const path = await invoke<string>("export_history", { items, format });
    setExportNotice(`✅ נשמר: ${path}`);
    window.setTimeout(() => setExportNotice(null), 6000);
  } catch (e) {
    const msg = String(e);
    if (msg !== "הייצוא בוטל") setBatchError(`ייצוא נכשל: ${msg}`);
  }
}, [batchTranscript]);
```

- [ ] **Step 2: Commit**

```bash
git add src/App.tsx
git commit -m "feat(batch-ui): pick/transcribe/cancel/export handlers"
```

---

### Task 3.4: Render the batch panel in the main view

**Files:**
- Modify: `src/App.tsx` (main view return, after the history section ~line 2188, before the footer)
- Modify: `src/App.css`

- [ ] **Step 1: Add the panel JSX**

Insert into the main-view `<main className="container compact" dir="rtl">` return, after the history block:
```tsx
{/* ── Batch: file transcription ── */}
<div className="settings-section batch-panel" dir="rtl">
  <h3>📁 תמלול קובץ</h3>
  <p className="settings-hint">
    העלה קובץ אודיו (mp3, m4a, wav, ogg, flac) ← תמלול ← עריכה / ייצוא / הזרקה.
    עובד בענן (מהיר, Deepgram) וגם במכשיר (פרטי, ללא אינטרנט).
  </p>

  <div className="batch-mode-toggle">
    <label className="toggle-label">
      <input type="radio" name="batchMode" checked={batchMode === "cloud"} disabled={batchRunning}
        onChange={() => setBatchMode("cloud")} />
      <span className="toggle-text">מהיר (ענן — Deepgram, המפתח שלך)</span>
    </label>
    <label className="toggle-label">
      <input type="radio" name="batchMode" checked={batchMode === "local"} disabled={batchRunning}
        onChange={() => setBatchMode("local")} />
      <span className="toggle-text">פרטי (במכשיר — איטי, ללא אינטרנט)</span>
    </label>
  </div>

  {!batchRunning && (
    <button className="btn-primary" onClick={handlePickAndTranscribe}>בחר קובץ ותמלל</button>
  )}

  {batchRunning && (
    <div className="batch-progress">
      <div className="batch-progress-bar">
        <div className="batch-progress-fill" style={{ width: `${batchPct}%` }} />
      </div>
      <span className="batch-progress-label">
        {stageLabel(batchStage)} {batchPct > 0 ? `${batchPct}%` : ""}
      </span>
      <button className="btn-secondary btn-sm" onClick={handleCancelBatch}>בטל</button>
    </div>
  )}

  {batchError && <p className="error" onClick={() => setBatchError("")}>❌ {batchError}</p>}

  {batchTranscript && (
    <div className="batch-result">
      <textarea
        dir="rtl"
        className="batch-textarea"
        value={batchTranscript}
        onChange={(e) => setBatchTranscript(e.target.value)}
        rows={10}
      />
      <div className="batch-actions">
        <button className="btn-secondary btn-sm" onClick={() => injectText(batchTranscript)} title="הדבק בשדה הפעיל">⌨️ הדבק</button>
        <button className="btn-secondary btn-sm" onClick={() => navigator.clipboard.writeText(batchTranscript)} title="העתק">📋 העתק</button>
        <button className="btn-secondary btn-sm" onClick={() => exportBatch("txt")}>📄 TXT</button>
        <button className="btn-secondary btn-sm" onClick={() => exportBatch("docx")}>📝 Word</button>
      </div>
    </div>
  )}
</div>
```

- [ ] **Step 2: Add the CSS**

Append to `src/App.css`:
```css
/* ── Batch file transcription panel ── */
.batch-mode-toggle {
  display: flex;
  flex-direction: column;
  gap: 0.3rem;
  margin: 0.5rem 0 0.75rem;
}
.batch-progress {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  margin-top: 0.75rem;
}
.batch-progress-bar {
  flex: 1;
  height: 6px;
  background: #0e1630;
  border-radius: 3px;
  overflow: hidden;
}
.batch-progress-fill {
  height: 100%;
  background: #4da6ff;
  transition: width 0.2s;
}
.batch-progress-label {
  font-size: 0.8rem;
  color: #aaa;
  white-space: nowrap;
}
.batch-textarea {
  width: 100%;
  box-sizing: border-box;
  margin-top: 0.75rem;
  padding: 0.6rem;
  background: #0e1630;
  color: #e8e8e8;
  border: 1px solid #2a3a5e;
  border-radius: 8px;
  font-family: inherit;
  font-size: 0.95rem;
  resize: vertical;
}
.batch-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 0.4rem;
  margin-top: 0.5rem;
}
```

- [ ] **Step 3: Commit**

```bash
git add src/App.tsx src/App.css
git commit -m "feat(batch-ui): file-transcription panel in main view + styles"
```

---

### Task 3.5: Manual verification — the full Phase-1 loop

- [ ] **Step 1: Run the app**

Run (repo root): `npm run tauri dev`
Expected: app launches, no console/Rust errors; the "📁 תמלול קובץ" panel appears in the main view.

- [ ] **Step 2: Cloud path (Deepgram)** — requires a Deepgram key configured
  - Select "מהיר (ענן)", click "בחר קובץ ותמלל", pick a short (~30s–2min) mp3.
  - Expect: progress shows "מפענח אודיו…" then "מתמלל…", then Hebrew transcript fills the textarea. Edit it, click 📄 TXT → save dialog → file written.

- [ ] **Step 3: Local path (offline)** — requires a downloaded model
  - Select "פרטי (במכשיר)", pick the same short file.
  - Expect: "מתמלל…" with a percentage climbing; transcript appears. (Use a SHORT clip — local is slow.)

- [ ] **Step 4: Cancel** — start a longer file, click "בטל" mid-run.
  - Expect: the run stops within ~1–2s; NO error toast (cancel is the calm `"בוטל"` sentinel). Panel returns to the idle button.

- [ ] **Step 5: iPhone .m4a** — pick a real `.m4a` voice memo (cloud and local).
  - Expect: decodes + transcribes. (If "קודק לא נתמך" → that file is HE-AAC; AAC-LC works.)

- [ ] **Step 6: Regression — short dictation still works**
  - Press Alt+D, dictate a sentence, confirm it injects as before. Then start a LOCAL batch and, while it runs, confirm the app/floating bar stays responsive (the engine mutex is not held during the long run).

- [ ] **Step 7: Empty/garbage file** — pick a tiny non-audio file renamed `.wav`.
  - Expect: clear Hebrew error "הקובץ ריק או פגום…" (or "פורמט לא נתמך…"), not a crash.

- [ ] **Step 8: Commit any fixes found during verification**

```bash
git add -A
git commit -m "fix(batch): address issues found in manual verification"
```

---

## Test coverage note (be honest about what's automated)

Automated `cargo test` covers the two **pure** modules: `decode.rs` (passthrough / downsample / corrupt-file) and `batch.rs` (routing). The **network/model-dependent** paths — Deepgram batch request, local `run_long_transcription`, the orchestrator's `tokio::select!` cancel, and **cancel-mid-cloud-request** (spec §14.2-P) — have **no automated tests** and are verified by the manual smoke in Task 3.5 only. This is a deliberate MVP trade-off (mocking a 900 s HTTP future / loading a 1.6 GB model in CI is disproportionate for Phase 1), not an oversight. If cloud-cancel regresses later, add a unit test that races a fake future against `Notify`.

## Done criteria (Phase 1)

- `cargo test` green (existing + `decode::tests` + `batch::tests`).
- A user can pick an audio file and get a Hebrew transcript via **either** cloud (Deepgram) **or** local (offline whisper), edit it, and export/inject/copy.
- Cancel works for decode, cloud, and local; cancelling shows a calm "בוטל", not an error.
- Short dictation is unaffected and stays responsive during a local batch.
- No `multi` language sent to Deepgram; no references to a non-existent "ivrit small" model.

**NOT in Phase 1 (later plans):** Groq cloud + chunking, save-next-to-file (header-less export), long in-app meeting recording, disk-streaming sink, Opus encode, accuracy toggle, the `BatchError` enum.
