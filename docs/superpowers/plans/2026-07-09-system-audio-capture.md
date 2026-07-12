# System Audio Capture Implementation Plan

> **Agentic workers are REQUIRED to execute this plan with `superpowers:subagent-driven-development`** (independent tasks in the current session) **or `superpowers:executing-plans`** (separate session with review checkpoints). Do not free-hand the work. Every task follows strict red→green TDD: write the failing test, run it to see it fail for the right reason, write the minimal implementation, run it green, commit. Each step is a `- [ ]` checkbox — check it off only when done. Work the chunks in order; within a chunk, work the tasks in order.

**Goal:** Add three recording sources to the Hebrew dictation app — `Mic` (existing, unchanged), `System` (WASAPI loopback of the default render device), and `Call` (mic + system captured together and transcribed as separated channels, "אני" vs "הצד השני") — as a batch record→stop→transcribe flow, Windows-only for System/Call, with zero regression to the existing mic path.

**Architecture:** A new Windows-only `SystemAudioRecorder` captures loopback audio in parallel with the existing cpal `AudioRecorder`; for `Call`, the two mono buffers are interleaved to stereo, encoded to a 2-channel WAV in memory, and POSTed to Deepgram with `multichannel=true` (never through the mono `transcribe_file`/`decode_file_to_16k_mono` path, which would collapse the channels). Each Deepgram channel is parsed independently, stamped with its channel index as the `speaker`, merged chronologically by `start_ms`, and rendered with an explicit `SpeakerLabelStyle`. Orchestration lives in `lib.rs` behind a new `source` parameter and a second `AppState.system_recorder`; the frontend gains a Windows-gated Mic/System/Call selector.

**Tech Stack:** cpal (existing mic capture), wasapi 0.23 (Windows loopback capture), Deepgram batch (`/v1/listen` multichannel), Tauri v2 (commands + `AppState`), rubato (existing file-decode resampler in `decode.rs` — note: System/Call reuse the mic's linear-interpolation `resample`, **not** rubato, per spec §4.1).

---

## ⏱️ Execution Progress — READ FIRST

| Chunk | Tasks | Status |
|---|---|---|
| 1. Pure stereo helpers | 1-2 | ✅ **DONE** — `4d1bf03`, `55e1922` |
| 2. Deepgram multichannel transcribe | 3-5 | ✅ **DONE** — `06f6a19`, `e021281`, `09b5b8f` |
| 3. `render_srt` `SpeakerLabelStyle` | 6 | ✅ **DONE** — `68a688a` |
| 4. `SystemAudioRecorder` (WASAPI) | 7-9 | ✅ **DONE** — `2d3c768`, `87a073c`, `b180faa` (wasapi API fix, see Task 9 note) |
| 5. `lib.rs` orchestration + `source` | 10-16 | ✅ **DONE** — `0ee8661`,`963aa78`,`6e27981`,`8de6dcb`,`a183b00`,`9807998`,`fbc7678` |
| 6. Frontend source selector | 17-20 | ✅ **DONE** — `cf2cb78`,`56516fb`,`8f15109`,`0073d8b` |
| Post-review hardening | — | ✅ **DONE** — `af30355` (Critical: cancel drains system recorder) + `dfb14d7` (2 deep-review fixes: Call cloud-transparency note, poison-path drain/rollback, `resample(0)` guard, System silence msg, frontend `speaker` type) |

**State (2026-07-12): 🎉 ALL 20 TASKS DONE + PUSHED to origin/main (`68a688a`→`dfb14d7`; Henry approved the push, NOT released to users).** `cargo build` = **0 warnings**, `cargo test` = **50 passed, 1 ignored** (the `#[ignore]`d MANUAL-VERIFY loopback), frontend `tsc && vite build` = clean. Every task strict-TDD via subagent-driven-development. **3 review passes total:** an integration review found one Critical (cancel didn't stop the system recorder → bricked System/Call after one cancel; fixed `af30355`), then two deep independent passes (concurrency + contract/regression) found **no Critical and no regression** (clippy clean), with the small fixes above landing in `dfb14d7`. A signed test installer was built (`src-tauri/target/release/bundle/nsis/הכתבה בעברית_2.11.0_x64-setup.exe`). Behavioral E2E (loopback + Call with a real 2nd audio source + Deepgram key) still pending — Henry tests it himself when he next has a video call.

**⏳ Still open — MANUAL-VERIFY on a real Windows machine (cannot be automated here):**
> 1. **Loopback capture** — `cargo test ... loopback_captures_playing_audio -- --ignored` while a video/song plays on the default render device. Expect `... ok` (captured >1s).
> 2. **Call E2E** — `npm run tauri dev`, batch view: select **שיחה**, speak while system audio plays, stop → confirm the transcript separates **אני:/הצד השני:** and the SRT export carries those labels (needs Henry's Deepgram key). Plus the Mic/System regression + UI (three Windows-gated cards) checks in Tasks 17-19 Step 4.

**Wasapi 0.23 API note (Task 9):** the plan's original snippet used `wasapi::get_default_device(&Direction::Render)` as a free fn; in wasapi 0.23.0 it is a **`DeviceEnumerator` method**. The shipped code (correct) uses `DeviceEnumerator::new().and_then(|e| e.get_default_device(&Direction::Render))` — see the corrected Task 9 Step 3 snippet below.

---

## File Structure

| File | Created/Modified | Single responsibility |
|---|---|---|
| `src-tauri/src/audio.rs` | Modified | Add pure `interleave_stereo(mic, system) -> Vec<f32>`; promote existing `to_mono` + `resample` to `pub(crate)` for reuse. Mic capture path unchanged. |
| `src-tauri/src/api_transcribe.rs` | Modified | Add the Call multichannel path: `samples_to_wav_stereo` (2-channel WAV encoder), `multichannel_url` (URL builder), `build_multichannel_result` (per-channel parse + channel-index speaker stamp + chronological merge + labeled text), `transcribe_deepgram_multichannel` (async POST wrapper). Existing mono `samples_to_wav` + all mono callers untouched. |
| `src-tauri/src/srt.rs` | Modified | Add `SpeakerLabelStyle { Diarization, Call }`; `render_srt` takes the style — `Diarization` preserves the exact ≥2-speaker "דובר N:" behavior, `Call` always labels "אני:"/"הצד השני:". |
| `src-tauri/src/system_audio.rs` | **Created** | Windows-only WASAPI loopback recorder `SystemAudioRecorder` (parallel to `AudioRecorder`: `new`/`is_recording`/`start_recording`/`stop_recording`) + pure `resample_to_16k_mono` helper. |
| `src-tauri/src/batch.rs` | Modified | Add pure, unit-tested decision types: `RecordingSource { Mic, System, Call }` (the source axis, distinct from `BatchOpts.mode`), `ensure_call_deepgram_available` guard, `recorders_for_source` routing table. |
| `src-tauri/src/lib.rs` | Modified | Orchestration: `mod system_audio`; `AppState.system_recorder`; source-aware `start_batch_recording` + `stop_batch_recording_to_file`; `call_stereo_wav_or_silent` (interleaved silence guard + stereo WAV); `stop_call_recording` command + handler registration; update the sole `render_srt` caller to the 2-arg form. |
| `src-tauri/Cargo.toml` | Modified | Windows-only `wasapi = "0.23"` target dependency. |
| `src/App.tsx` | Modified | `RecordingSource` type, `IS_WINDOWS` const, `recordSource` state, the Windows-gated Mic/System/Call selector, threading `source` into the start/stop invokes, and the Call stop branch. |

---

## Cross-Block Seam Reconciliations (read before executing)

The chunks were authored as independent components; four interface mismatches were reconciled so the whole crate + frontend compile and run coherently. The reconciled forms below are already baked into the task code — **do not revert to the pre-reconciliation shapes.**

1. **`SystemAudioRecorder` method names + `stop` return type.** Chunk 4 is authoritative and names the methods to parallel `AudioRecorder`: **`start_recording(&mut self) -> Result<(), String>`** and **`stop_recording(&mut self) -> Result<Vec<f32>, String>`** (stop returns a `Result`, not a bare `Vec`). The Chunk 5 orchestration therefore calls `sys.start_recording()` / `sys.stop_recording()` (with `?` on the `Result`), **not** the `start()`/`stop()` shorthand from the spec §4.1 prose.
2. **Call stop command name.** The single canonical Tauri command is **`stop_call_recording`** (defined in Chunk 5). The frontend (Chunk 6) invokes exactly that name — the earlier `stop_call_recording_and_transcribe` spelling is retired.
3. **Call stop invoke passes `opts`.** `stop_call_recording(opts: BatchOpts)` requires an `opts` payload, so the frontend Call invoke sends `{ opts: { mode: batchMode, language: "he", inject: false } }`.
4. **`stop_batch_recording_to_file` receives `source`.** The backend re-derives which recorder to drain from `source` (it does not persist the source chosen at start), so the frontend Mic/System stop invoke sends `{ source: recordSource }`.

**Dependency order (why the chunks are ordered this way):** Chunk 5 (orchestration) calls into `audio::interleave_stereo` + `api_transcribe::samples_to_wav_stereo` (Chunk 1), `api_transcribe::transcribe_deepgram_multichannel` (Chunk 2), and `system_audio::SystemAudioRecorder` (Chunk 4); it also relies on the `render_srt` 2-arg signature + its `lib.rs:1201` caller fix landing together in Chunk 3. `cargo build`/`cargo test <filter>` compile the **whole crate**, so a Chunk-5 task cannot go green until Chunks 1–4 are merged. Chunks 1–4 are each self-contained and compile/pass on their own (unused-until-Chunk-5 helpers emit at most a harmless `dead_code` warning on plain `cargo build` — the crate has no `deny(warnings)`, and `cargo test` references them so the warning is silenced there).

**Tests run with:** `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml <filter>` (Windows host). Frontend tasks have no JS/Rust test runner; their safety net is `npx tsc --noEmit` + a manual `npm run tauri dev` preview, with a `git grep` "landed-marker" gate (mechanical presence check, not a behavioral test).

---

## Chunk 1: Pure stereo helpers

Two pure, side-effect-free encoders that the Call path is built on. Both are unit-tested end-to-end with no audio hardware. Nothing else depends on Chunk 1 yet, so it goes green immediately.

### Task 1: `interleave_stereo` pure helper in `audio.rs`
**Files:** Modify `src-tauri/src/audio.rs` (append `interleave_stereo` immediately after `resample`, ~line 634; add a new `#[cfg(test)] mod interleave_stereo_tests` as the file's **final** item so no test mod sits mid-file — avoids clippy `items_after_test_module`).

- [ ] Step 1: Write the failing test — append this as the **last item** in `audio.rs` (after `resample`):

```rust
#[cfg(test)]
mod interleave_stereo_tests {
    use super::*;

    #[test]
    fn equal_lengths_interleave_l_mic_r_system() {
        // L = mic, R = system, frame-interleaved: [L0, R0, L1, R1].
        let mic = [0.1f32, 0.2];
        let system = [0.3f32, 0.4];
        assert_eq!(
            interleave_stereo(&mic, &system),
            vec![0.1f32, 0.3, 0.2, 0.4]
        );
    }

    #[test]
    fn mic_longer_pads_system_with_silence() {
        // system is shorter → its missing R samples are silence (0.0).
        let mic = [0.1f32, 0.2, 0.5];
        let system = [0.3f32];
        assert_eq!(
            interleave_stereo(&mic, &system),
            vec![0.1f32, 0.3, 0.2, 0.0, 0.5, 0.0]
        );
    }

    #[test]
    fn system_longer_pads_mic_with_silence() {
        // mic is shorter → its missing L samples are silence (0.0).
        let mic = [0.1f32];
        let system = [0.3f32, 0.4];
        assert_eq!(
            interleave_stereo(&mic, &system),
            vec![0.1f32, 0.3, 0.0, 0.4]
        );
    }

    #[test]
    fn empty_inputs_yield_empty() {
        assert!(interleave_stereo(&[], &[]).is_empty());
        // One side empty still pads the other to a full stereo frame.
        assert_eq!(interleave_stereo(&[], &[0.5f32]), vec![0.0f32, 0.5]);
    }
}
```

- [ ] Step 2: Run test to verify it fails — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml interleave_stereo_tests`. Expected: fails to compile — `error[E0425]: cannot find function \`interleave_stereo\` in this scope`. This is the expected red.

- [ ] Step 3: Write minimal implementation — insert this fn **immediately after `resample` and before the `mod interleave_stereo_tests` added in Step 1**, keeping the test mod as the file's final item:

```rust
/// Interleave a mono mic buffer and a mono system buffer into a stereo (2-channel)
/// buffer: L = mic, R = system, laid out per-frame as [L0, R0, L1, R1, …]. The
/// shorter side is padded with silence (0.0) to the longer length, so the result is
/// always `2 * max(mic.len(), system.len())` samples. Used by Call mode to keep
/// channel 0 ("me") and channel 1 ("them") separated for Deepgram multichannel.
pub fn interleave_stereo(mic: &[f32], system: &[f32]) -> Vec<f32> {
    let max_len = mic.len().max(system.len());
    let mut out = Vec::with_capacity(max_len * 2);
    for i in 0..max_len {
        out.push(mic.get(i).copied().unwrap_or(0.0)); // L = mic
        out.push(system.get(i).copied().unwrap_or(0.0)); // R = system
    }
    out
}
```

- [ ] Step 4: Run test to verify it passes — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml interleave_stereo_tests`. Expected: `test result: ok. 4 passed; 0 failed`.

- [ ] Step 5: Commit —
`git add "C:/Users/אורח/claude-dev/AI-Tools/MCP-Dev/hebrew-dictation/src-tauri/src/audio.rs"`
`git commit -m "feat(audio): interleave mic+system into stereo for Call mode" -m "Pad the shorter buffer with silence; L=mic, R=system. Pure helper for the Call-mode multichannel path (spec 4.2)." -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"`

---

### Task 2: `samples_to_wav_stereo` in `api_transcribe.rs`
**Files:** Modify `src-tauri/src/api_transcribe.rs` (add fn immediately after `samples_to_wav`, i.e. after line 44; add tests inside the existing `#[cfg(test)] mod tests` at the file END, ~lines 405-443, before its closing `}`).

- [ ] Step 1: Write the failing test — add these two test fns **inside the existing `mod tests`** (before its closing brace), keeping all tests in the file's END test module:

```rust
    #[test]
    fn samples_to_wav_stereo_header_is_two_channel() {
        // One stereo frame (L=0, R=0) → 2 samples → 4 data bytes, 48-byte file.
        let wav = samples_to_wav_stereo(&[0.0f32, 0.0], 16000);
        assert_eq!(wav.len(), 48);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        // audio format = PCM (1)
        assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 1);
        // num_channels = 2 (the whole point of the stereo variant)
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2);
        // sample_rate = 16000
        assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), 16000);
        // byte_rate = 16000 * 2ch * 2bytes = 64000
        assert_eq!(u32::from_le_bytes([wav[28], wav[29], wav[30], wav[31]]), 64000);
        // block_align = 2ch * 2bytes = 4
        assert_eq!(u16::from_le_bytes([wav[32], wav[33]]), 4);
        // bits per sample = 16
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16);
        assert_eq!(&wav[36..40], b"data");
        // data_size = 4 bytes
        assert_eq!(u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]), 4);
    }

    #[test]
    fn samples_to_wav_stereo_encodes_and_clamps_samples() {
        // Full-scale L=+1.0 clamps to i16 32767; R=-1.0 maps to -32768.
        let wav = samples_to_wav_stereo(&[1.0f32, -1.0], 16000);
        assert_eq!(wav.len(), 48);
        // First sample (L): 32767 = 0x7FFF little-endian.
        assert_eq!(&wav[44..46], &32767i16.to_le_bytes());
        // Second sample (R): -32768 = 0x8000 little-endian.
        assert_eq!(&wav[46..48], &(-32768i16).to_le_bytes());
    }
```

- [ ] Step 2: Run test to verify it fails — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml samples_to_wav_stereo`. Expected: fails to compile — `error[E0425]: cannot find function \`samples_to_wav_stereo\` in this scope`. This is the expected red.

- [ ] Step 3: Write minimal implementation — insert this fn **immediately after `samples_to_wav` (after line 44)**. It is a SEPARATE fn (not a generalization of `samples_to_wav`) so no mono caller is touched:

```rust
/// Convert an **interleaved stereo** f32 buffer (L,R,L,R… at `sample_rate`) to a
/// 2-channel PCM16 WAV byte buffer. Kept deliberately SEPARATE from `samples_to_wav`
/// (which hardcodes `num_channels = 1`) so the Call-mode multichannel body can be
/// 2-channel without changing any existing mono caller (groq/deepgram single+batch).
pub(crate) fn samples_to_wav_stereo(interleaved: &[f32], sample_rate: u32) -> Vec<u8> {
    let num_samples = interleaved.len(); // total across both channels
    let bytes_per_sample: u16 = 2;
    let num_channels: u16 = 2;
    let data_size = (num_samples * bytes_per_sample as usize) as u32;
    // RIFF ChunkSize = 4("WAVE") + 24(fmt chunk) + 8(data header) + data_size
    let file_size = 36 + data_size;

    let mut buf = Vec::with_capacity(44 + data_size as usize);

    // RIFF header
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&file_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    // fmt chunk
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    buf.extend_from_slice(&num_channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * num_channels as u32 * bytes_per_sample as u32;
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    let block_align = num_channels * bytes_per_sample;
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&(bytes_per_sample * 8).to_le_bytes());

    // data chunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());

    // interleaved is already L,R,L,R… — write straight through.
    for &sample in interleaved {
        let clamped = (sample * 32768.0).clamp(-32768.0, 32767.0) as i16;
        buf.extend_from_slice(&clamped.to_le_bytes());
    }

    buf
}
```

- [ ] Step 4: Run test to verify it passes — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml samples_to_wav_stereo`. Expected: `test result: ok. 2 passed; 0 failed`.

- [ ] Step 5: Commit —
`git add "C:/Users/אורח/claude-dev/AI-Tools/MCP-Dev/hebrew-dictation/src-tauri/src/api_transcribe.rs"`
`git commit -m "feat(api): add samples_to_wav_stereo for Call multichannel body" -m "Separate 2-channel PCM16 WAV encoder for the Call stereo path; mono samples_to_wav callers untouched (spec 4.3)." -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"`

---

## Chunk 2: Deepgram multichannel transcribe

The Call transcription core, built on the existing `parse_deepgram_words` (`api_transcribe.rs:316`) and `chunk_words_to_cues`. All three functions land in `api_transcribe.rs`, inserted after `parse_deepgram_words` (ends ~line 340) and before the `#[cfg(test)] mod tests` block (~line 405). Reuses `crate::srt::{TimedSegment, TimedWord, chunk_words_to_cues, SRT_MAX_WORDS_PER_CUE, SRT_MAX_MS_PER_CUE}` and `ApiError`/`classify_request_error`/`classify_status`, all already in scope.

### Task 3: `multichannel_url` — Call-mode URL builder (multichannel on, diarize/paragraphs off)
**Files:** Modify `src-tauri/src/api_transcribe.rs` (insert after `parse_deepgram_words`, before the `// ── Unified entry point ──` section; add test inside `mod tests` after `parse_words_absent_speaker_is_none`, ~line 442).

- [ ] Step 1: Write the failing test — add inside `mod tests`:

```rust
#[test]
fn multichannel_url_has_multichannel_but_no_diarize_or_paragraphs() {
    // Same nova-3 base as the batch route, "auto" resolved to Hebrew…
    let url = multichannel_url("auto");
    assert!(url.contains("model=nova-3"));
    assert!(url.contains("language=he"));
    assert!(url.contains("smart_format=true"));
    assert!(url.contains("punctuate=true"));
    // …but Call mode is per-channel: multichannel ON, and NO diarize/paragraphs
    // (the labeled text is built from segments, not the flat transcript).
    assert!(url.contains("multichannel=true"));
    assert!(!url.contains("diarize"));
    assert!(!url.contains("paragraphs"));
    // An explicit language passes through unchanged.
    assert_eq!(
        multichannel_url("he"),
        "https://api.deepgram.com/v1/listen?model=nova-3&language=he&smart_format=true&punctuate=true&multichannel=true"
    );
}
```

- [ ] Step 2: Run test to verify it fails — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml multichannel_url_has_multichannel_but_no_diarize_or_paragraphs`. Expected: compile error `E0425: cannot find function 'multichannel_url' in this scope` (crate fails to build → red).

- [ ] Step 3: Write minimal implementation — insert after `parse_deepgram_words`:

```rust
/// Build the Deepgram `/listen` URL for Call-mode multichannel transcription:
/// the same nova-3 base as `transcribe_deepgram_batch` but WITH
/// `multichannel=true` and deliberately WITHOUT `diarize`/`paragraphs`. Each
/// channel is transcribed independently and the labeled text is assembled from
/// the merged segments (`build_multichannel_result`), never from Deepgram's
/// per-channel flat transcript. `auto` maps to Hebrew, matching the batch route.
fn multichannel_url(language: &str) -> String {
    let lang = if language == "auto" { "he" } else { language };
    format!(
        "https://api.deepgram.com/v1/listen?model=nova-3&language={}&smart_format=true&punctuate=true&multichannel=true",
        lang
    )
}
```

- [ ] Step 4: Run test to verify it passes — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml multichannel_url_has_multichannel_but_no_diarize_or_paragraphs`. Expected: `... ok` (1 passed).

- [ ] Step 5: Commit —
`git -C C:/Users/אורח/claude-dev/AI-Tools/MCP-Dev/hebrew-dictation add src-tauri/src/api_transcribe.rs`
`git -C C:/Users/אורח/claude-dev/AI-Tools/MCP-Dev/hebrew-dictation commit -m "feat(transcribe): add multichannel_url builder for Call mode" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"`

---

### Task 4: `build_multichannel_result` — parse both channels, stamp channel speaker, merge, build labeled text
**Files:** Modify `src-tauri/src/api_transcribe.rs` (insert after `multichannel_url`, still before `mod tests`; add test inside `mod tests` after Task 3's test).

- [ ] Step 1: Write the failing test — add inside `mod tests`:

```rust
#[test]
fn multichannel_stamps_channel_speaker_and_labels_text() {
    // Deepgram multichannel response with diarize OFF: words carry NO `speaker`
    // field, so the parser returns None and the channel index must be stamped
    // explicitly. channels[0] = mic ("me"), channels[1] = system ("them").
    // "them" speaks first in time, so the chronological merge must reorder it
    // ahead of "me" — proving the merge is by start_ms, not append order.
    let body = serde_json::json!({
        "results": {
            "channels": [
                { "alternatives": [{ "words": [
                    { "word": "הכול", "punctuated_word": "הכול", "start": 1.0, "end": 1.3 },
                    { "word": "טוב", "punctuated_word": "טוב", "start": 1.3, "end": 1.6 }
                ]}]},
                { "alternatives": [{ "words": [
                    { "word": "מה", "punctuated_word": "מה", "start": 0.0, "end": 0.3 },
                    { "word": "נשמע", "punctuated_word": "נשמע", "start": 0.3, "end": 0.6 }
                ]}]}
            ]
        }
    });

    let (text, segments) = build_multichannel_result(&body);

    // Channel index stamped explicitly (parser returned None — diarize off).
    assert_eq!(segments.len(), 2);
    // Merged chronologically: channel 1 ("them", start 0) before channel 0
    // ("me", start 1000).
    assert_eq!(segments[0].speaker, Some(1));
    assert_eq!(segments[0].text, "מה נשמע");
    assert_eq!(segments[1].speaker, Some(0));
    assert_eq!(segments[1].text, "הכול טוב");

    // Labeled text in chronological order, with BOTH sides present.
    assert_eq!(text, "הצד השני: מה נשמע\nאני: הכול טוב");
    assert!(text.contains("אני:"));
    assert!(text.contains("הצד השני:"));
}
```

- [ ] Step 2: Run test to verify it fails — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml multichannel_stamps_channel_speaker_and_labels_text`. Expected: compile error `E0425: cannot find function 'build_multichannel_result' in this scope` (red).

- [ ] Step 3: Write minimal implementation — **first** add the shared side-label helper to `src-tauri/src/srt.rs` (above `render_srt`), so this task's `text` builder and Chunk 3's `render_srt` Call arm use the *same* strings and can never drift:

```rust
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
```

  Then insert after `multichannel_url` in `src-tauri/src/api_transcribe.rs`:

```rust
/// Turn a Deepgram *multichannel* response into `(labeled_text, merged_segments)`,
/// mirroring `transcribe_deepgram_batch`'s return shape. `diarize` is off for
/// multichannel, so `parse_deepgram_words` yields `speaker: None`; we therefore
/// stamp each channel's index as the speaker (channel 0 = "me", channel 1 =
/// "them") on every word BEFORE chunking. Both channels' cues are merged by
/// `start_ms` (they share the one stereo-file clock) and the `text` is built
/// from those merged cues — never from Deepgram's per-channel flat transcript,
/// which defaults to channel 0 and would silently drop "them".
fn build_multichannel_result(
    body: &serde_json::Value,
) -> (String, Vec<crate::srt::TimedSegment>) {
    let mut segments: Vec<crate::srt::TimedSegment> = Vec::new();
    for channel_idx in 0u32..2 {
        let alt = &body["results"]["channels"][channel_idx as usize]["alternatives"][0];
        let mut words = parse_deepgram_words(alt);
        // diarize is off → the parser returns None; stamp the channel index
        // explicitly on each word before chunking (spec §4.4).
        for w in &mut words {
            w.speaker = Some(channel_idx);
        }
        segments.extend(crate::srt::chunk_words_to_cues(
            &words,
            crate::srt::SRT_MAX_WORDS_PER_CUE,
            crate::srt::SRT_MAX_MS_PER_CUE,
        ));
    }
    // Both channels share the single stereo-file clock, so start_ms merges them
    // chronologically. Stable sort keeps channel 0 before channel 1 on ties.
    segments.sort_by_key(|s| s.start_ms);

    let text = segments
        .iter()
        .map(|seg| {
            // Single source of truth (srt::call_side_label), shared with
            // render_srt's Call arm so the injected text and the SRT prefix
            // can never drift apart.
            let label = crate::srt::call_side_label(seg.speaker.unwrap_or(1));
            format!("{}: {}", label, seg.text)
        })
        .collect::<Vec<_>>()
        .join("\n");

    (text, segments)
}
```

- [ ] Step 4: Run test to verify it passes — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml multichannel_stamps_channel_speaker_and_labels_text`. Expected: `... ok` (1 passed).

- [ ] Step 5: Commit —
`git -C C:/Users/אורח/claude-dev/AI-Tools/MCP-Dev/hebrew-dictation add src-tauri/src/api_transcribe.rs src-tauri/src/srt.rs`
`git -C C:/Users/אורח/claude-dev/AI-Tools/MCP-Dev/hebrew-dictation commit -m "feat(transcribe): stamp channel speaker and build labeled text for multichannel" -m "Adds srt::call_side_label as the single source of truth for the side labels." -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"`

---

### Task 5: `transcribe_deepgram_multichannel` — async POST wrapper composing url + result builder
**Files:** Modify `src-tauri/src/api_transcribe.rs` (insert after `build_multichannel_result`, before `mod tests`; add test inside `mod tests` after Task 4's test).

- [ ] Step 1: Write the failing test — add this compile-time signature guard inside `mod tests` (there is no network mock harness in this crate — as with `transcribe_deepgram_batch`, only the pure pieces are behaviorally tested; this guards the exact `(&Client, Vec<u8>, &str, &str) -> Result<(String, Vec<TimedSegment>), ApiError>` shape the Call orchestration depends on, without ever building/awaiting the future):

```rust
#[test]
fn multichannel_wrapper_has_expected_signature() {
    // Compile-time signature guard only — never polls the future, never hits
    // the network. `_reference` compiles ONLY if transcribe_deepgram_multichannel
    // takes (&reqwest::Client, Vec<u8>, &str, &str) and returns a future whose
    // Output is Result<(String, Vec<TimedSegment>), ApiError>.
    #[allow(dead_code)]
    async fn _reference(
        client: &reqwest::Client,
        bytes: Vec<u8>,
        key: &str,
        lang: &str,
    ) -> Result<(String, Vec<crate::srt::TimedSegment>), ApiError> {
        transcribe_deepgram_multichannel(client, bytes, key, lang).await
    }
    let _ = _reference; // silence unused warning; body is never executed
}
```

- [ ] Step 2: Run test to verify it fails — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml multichannel_wrapper_has_expected_signature`. Expected: compile error `E0425: cannot find function 'transcribe_deepgram_multichannel' in this scope` (red).

- [ ] Step 3: Write minimal implementation — insert after `build_multichannel_result`:

```rust
/// Multichannel (stereo) Deepgram batch request — the Call-mode core. `stereo_wav_bytes`
/// is a 2-channel / 16kHz WAV body (channel 0 = mic → "me", channel 1 = system
/// → "them"), built by `samples_to_wav_stereo` upstream. Sends `&multichannel=true`
/// and — unlike `transcribe_deepgram_batch` — NO `diarize`/`paragraphs`: each
/// channel is transcribed on its own, the channel index is stamped as the
/// speaker, and the labeled `text` is built from the merged segments (see
/// `build_multichannel_result`). The caller supplies a client with a long
/// timeout; this fn sets none of its own (parity with `transcribe_deepgram_batch`).
pub(crate) async fn transcribe_deepgram_multichannel(
    client: &reqwest::Client,
    stereo_wav_bytes: Vec<u8>,
    api_key: &str,
    language: &str,
) -> Result<(String, Vec<crate::srt::TimedSegment>), ApiError> {
    let response = client
        .post(multichannel_url(language))
        .header("Authorization", format!("Token {}", api_key))
        .header("Content-Type", "audio/wav")
        .body(stereo_wav_bytes)
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

    Ok(build_multichannel_result(&body))
}
```

- [ ] Step 4: Run test to verify it passes — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml multichannel_wrapper_has_expected_signature`. Expected: `... ok` (1 passed; crate compiles clean).

- [ ] Step 5: Commit —
`git -C C:/Users/אורח/claude-dev/AI-Tools/MCP-Dev/hebrew-dictation add src-tauri/src/api_transcribe.rs`
`git -C C:/Users/אורח/claude-dev/AI-Tools/MCP-Dev/hebrew-dictation commit -m "feat(transcribe): add transcribe_deepgram_multichannel POST wrapper for Call mode" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"`

---

## Chunk 3: `render_srt` `SpeakerLabelStyle`

The signature change and its **only** production caller (`lib.rs:1201`, inside `export_srt`) land together in this one chunk so the crate never has a mismatched call site. `Diarization` reproduces the current ≥2-speaker "דובר N:" behavior byte-for-byte; `Call` always labels "אני:"/"הצד השני:".

### Task 6: `render_srt` takes `SpeakerLabelStyle` (Diarization = exact current behavior; Call always labels) + caller update
**Files:**
- Modify `src-tauri/src/srt.rs` (add `SpeakerLabelStyle` enum above `render_srt`'s doc comment, and replace the doc comment + fn together at lines 92-133 — the new block carries an updated `render_srt(&[cues], styles)` doc so the stale one-arg doc is swapped out, not duplicated). `call_side_label` already exists from Task 4.
- Modify `src-tauri/src/lib.rs` `export_srt` (~1160-1206): add a `styles: Option<Vec<srt::SpeakerLabelStyle>>` command param and forward it to `render_srt`. Omitting it (today's frontend) yields `Diarization` for every file, so existing exports stay byte-for-byte identical until Chunk 6 fills the array.
- Test in `src-tauri/src/srt.rs` `#[cfg(test)] mod tests` (~lines 219-269).

- [ ] Step 1: Write the failing tests — migrate the four existing `render_*` tests to the two-arg signature (asserting byte-identical Diarization output) and add two new `Call` tests. Replace the four existing render tests (`render_single_file_zero_offset`, `render_combines_files_with_cumulative_offset`, `render_labels_speakers_when_multiple`, `render_single_speaker_has_no_labels`) with these, and append the two `Call` tests inside the same `mod tests`:

```rust
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
```

- [ ] Step 2: Run tests to verify they fail — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml srt::tests`. Expected: build fails to compile — `error[E0433]: failed to resolve: use of undeclared type 'SpeakerLabelStyle'` (and typically `error[E0061]: this function takes 1 argument but 2 arguments were supplied`, though rustc may suppress the arity error where the arg expression itself fails to resolve). 0 tests run.

- [ ] Step 3: Write minimal implementation — replace `render_srt`'s doc comment **and** function together (lines 92-133) with the block below: the `SpeakerLabelStyle` enum, a refreshed doc comment (single-file example now `render_srt(&[cues], style)`), then the two-arg fn:

```rust
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
```

  Then **wire the style through the sole production caller** — `export_srt` in `lib.rs` (~1160-1206). Without this, `SpeakerLabelStyle::Call` is production-dead code and a Call recording would still export `דובר 1:/דובר 2:`. Add the command param (Tauri passes `None` when the frontend omits it, so today's exports are unchanged):

```rust
#[tauri::command]
async fn export_srt(
    app: AppHandle,
    state: State<'_, AppState>,
    items: Vec<Vec<srt::TimedSegment>>,
    // Per-item label style, parallel to `items`. Omitted or short → Diarization,
    // preserving the historical export byte-for-byte. Chunk 6 (Task 20) fills it
    // so a Call recording exports "אני:"/"הצד השני:" instead of "דובר N:".
    styles: Option<Vec<srt::SpeakerLabelStyle>>,
    suggested_name: Option<String>,
) -> Result<String, String> {
```

  and replace `let content = srt::render_srt(&items);` with:

```rust
    let styles = styles.unwrap_or_default();
    let content = srt::render_srt(&items, &styles);
```

- [ ] Step 4: Run tests to verify they pass — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml srt::tests`. Expected: `test result: ok. 16 passed; 0 failed` — the four migrated Diarization tests produce byte-identical output, the two `render_call_*` tests pass, and the two new tests (`render_defaults_to_diarization_when_styles_missing`, `render_applies_style_per_file`) pass. Also run the full suite once to confirm the `export_srt` caller compiles: `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml`.

- [ ] Step 5: Commit —
```
git -C C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation add src-tauri/src/srt.rs src-tauri/src/lib.rs
git -C C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation commit -m "feat(srt): per-file SpeakerLabelStyle; Call always labels אני/הצד השני

Diarization preserves the exact >=2-speaker gate and דובר N: output; Call
bypasses the gate for meeting exports. Style is per file so a combined export
can mix a call with dictations. export_srt gains an optional styles param and
forwards it; omitting it keeps today's output byte-for-byte.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Chunk 4: `SystemAudioRecorder` (WASAPI loopback)

> **Scope (spec §4.1):** Windows-only (`#[cfg(target_os = "windows")]`). The test host **is** Windows, so the module + the pure `resample_to_16k_mono` helper compile and unit-test normally. The actual loopback capture needs a live default render device with audio playing and **cannot be auto-tested** — that step is **MANUAL-VERIFY** (`#[ignore]`d, compiled but skipped by `cargo test`). The wasapi code targets the **pinned crate `0.23.0`** (docs.rs, 2026-07-05): three-arg `initialize_client(&format, &Direction, &StreamMode)`; loopback = default **Render** device acquired via `get_default_device(&Direction::Render)` then initialized for **Capture**; deprecated `get_periods` replaced by `get_device_period`. **Method naming (reconciled — see Seam #1):** methods are `start_recording()` / `stop_recording() -> Result<Vec<f32>, String>`, parallel to `AudioRecorder`, so the Chunk 5 orchestration shares one signature. Resampling **reuses the mic's linear-interpolation `resample`** (`audio.rs:611`) + `to_mono` (`audio.rs:538`) per the spec's "choose one, not rubato" directive.

### Task 7: Scaffold the Windows-only `SystemAudioRecorder` module
**Files:** Create `src-tauri/src/system_audio.rs`; Modify `src-tauri/src/lib.rs:13` (register module); Test in `src-tauri/src/system_audio.rs`.

- [ ] Step 1: Write the failing test. In `lib.rs`, immediately after `mod streaming;` (line 13) add:
```rust
#[cfg(target_os = "windows")]
mod system_audio;
```
  Create `system_audio.rs` with just:
```rust
//! System-audio (WASAPI loopback) recorder — Windows only. Mirrors `AudioRecorder`:
//! captures the default *render* device via loopback, resamples the native rate
//! (48k/44.1k) down to 16kHz mono, and exposes start/stop. Spec §4.1.

#[cfg(test)]
mod system_audio_tests {
    use super::*;

    #[test]
    fn new_recorder_is_idle() {
        let rec = SystemAudioRecorder::new();
        assert!(!rec.is_recording());
    }
}
```
- [ ] Step 2: Run test to verify it fails — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml new_recorder_is_idle`. Expected: compile error `cannot find type/function \`SystemAudioRecorder\`` (E0433/E0425).
- [ ] Step 3: Write minimal implementation — in `system_audio.rs`, **above** the `#[cfg(test)]` mod, add:
```rust
use std::sync::{Arc, Mutex};

/// WASAPI loopback recorder for system output audio. Field set is intentionally
/// minimal here; the capture buffer / native-format / thread-handle fields are
/// added when start/stop land. Re-entrancy is guarded per-recorder, independent
/// of the mic's `AudioRecorder` (spec §4.1: separate `AppState.system_recorder`).
pub struct SystemAudioRecorder {
    is_recording: Arc<Mutex<bool>>,
}

impl SystemAudioRecorder {
    pub fn new() -> Self {
        Self {
            is_recording: Arc::new(Mutex::new(false)),
        }
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.lock().map(|r| *r).unwrap_or(false)
    }
}
```
- [ ] Step 4: Run test to verify it passes — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml new_recorder_is_idle`. Expected: `test system_audio_tests::new_recorder_is_idle ... ok`.
- [ ] Step 5: Commit —
```
git -C C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation add src-tauri/src/system_audio.rs src-tauri/src/lib.rs
git -C C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation commit -m "feat(system-audio): scaffold Windows-only SystemAudioRecorder module" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: Pure `resample_to_16k_mono` helper (reuse the mic's resample)
**Files:** Modify `src-tauri/src/system_audio.rs`; Modify `src-tauri/src/audio.rs:538,611` (promote helpers to `pub(crate)`); Test in `src-tauri/src/system_audio.rs`.

- [ ] Step 1: Write the failing test — add inside `system_audio_tests`:
```rust
#[test]
fn resample_to_16k_mono_downmixes_and_downsamples() {
    // Stereo (2ch) @32kHz, L=+1.0 / R=-1.0 → per-frame average 0.0.
    // 8 interleaved samples = 4 frames → 4 mono @32k → 2 samples @16k (ratio 2.0).
    let stereo_32k = vec![1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
    let out = resample_to_16k_mono(&stereo_32k, 32000, 2);
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|s| s.abs() < 1e-6));
}

#[test]
fn resample_to_16k_mono_passthrough_when_already_16k_mono() {
    let mono_16k = vec![0.1, 0.2, 0.3, 0.4];
    let out = resample_to_16k_mono(&mono_16k, 16000, 1);
    assert_eq!(out, mono_16k);
}
```
- [ ] Step 2: Run test to verify it fails — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml resample_to_16k_mono`. Expected: compile error `cannot find function \`resample_to_16k_mono\`` (E0425).
- [ ] Step 3: Write minimal implementation. First expose the mic's helpers to the crate: in `audio.rs` change line 538 `fn to_mono(` → `pub(crate) fn to_mono(` and line 611 `fn resample(` → `pub(crate) fn resample(`. Then in `system_audio.rs`, **above** the `#[cfg(test)]` mod, add:
```rust
/// Down-mix to mono and resample to 16kHz — identical to the mic's stop-path tail
/// (audio.rs:496-503), reusing the SAME linear-interpolation `resample` the mic
/// uses (spec §4.1: "choose one; NOT rubato"). Pure — unit-testable without audio.
pub(crate) fn resample_to_16k_mono(raw: &[f32], native_rate: u32, native_channels: u16) -> Vec<f32> {
    let mono = crate::audio::to_mono(raw, native_channels);
    if native_rate == 16000 {
        mono
    } else {
        crate::audio::resample(&mono, native_rate, 16000)
    }
}
```
- [ ] Step 4: Run test to verify it passes — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml resample_to_16k_mono`. Expected: both tests `... ok`.
- [ ] Step 5: Commit —
```
git -C C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation add src-tauri/src/system_audio.rs src-tauri/src/audio.rs
git -C C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation commit -m "feat(system-audio): add pure resample_to_16k_mono helper reusing mic resample" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 9: WASAPI loopback capture — `start_recording` / `stop_recording` (MANUAL-VERIFY)
**Files:** Modify `src-tauri/Cargo.toml:53` (after `tiny_http`); Modify `src-tauri/src/system_audio.rs`; Test in `src-tauri/src/system_audio.rs`.

> **MANUAL-VERIFY:** the capture body binds a real WASAPI render device; it can only run on a Windows machine with audio playing. The test is `#[ignore]`d so `cargo test` **compiles** it (proving the wasapi 0.23.0 code builds) but does not run it. No API reconciliation is deferred — every symbol matches wasapi 0.23.0's published signatures.

- [ ] Step 1: Write the failing test — add to `system_audio_tests`:
```rust
#[test]
#[ignore = "MANUAL-VERIFY on Windows: play audio on the default render device, then run with --ignored"]
fn loopback_captures_playing_audio() {
    let mut rec = SystemAudioRecorder::new();
    rec.start_recording()
        .expect("loopback should bind the default render device");
    std::thread::sleep(std::time::Duration::from_secs(3));
    let samples = rec
        .stop_recording()
        .expect("stop should return 16kHz mono samples");
    // ~3s of 16kHz mono ≈ 48000 samples; assert a non-trivial (>1s) capture.
    assert!(samples.len() > 16000, "expected >1s of audio, got {}", samples.len());
    assert!(!rec.is_recording());
}
```
- [ ] Step 2: Run test to verify it fails — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml loopback_captures_playing_audio`. Expected: compile error `no method named \`start_recording\`/\`stop_recording\` found` (E0599).
- [ ] Step 3: Write minimal implementation. Add the wasapi dependency to `Cargo.toml` (after the `tiny_http` line 53):
```toml
# WASAPI loopback capture of the default render device (system audio). Windows only.
# Pinned to 0.23.x; the capture body below targets 0.23.0's StreamMode-based API
# (three-arg initialize_client; loopback = default Render device + Direction::Capture).
[target.'cfg(windows)'.dependencies]
wasapi = "0.23"
```
  In `system_audio.rs`, replace the current `use` line + struct/impl (from Tasks 7-8) with the full capture implementation **above** the `#[cfg(test)]` mod (`resample_to_16k_mono` from Task 8 stays as-is):
```rust
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
// NOTE (wasapi 0.23 fix): get_default_device is a DeviceEnumerator METHOD, not a free fn.
use wasapi::{initialize_mta, DeviceEnumerator, Direction, StreamMode};

pub struct SystemAudioRecorder {
    samples: Arc<Mutex<Vec<f32>>>,
    is_recording: Arc<Mutex<bool>>,
    /// Native capture format, published by the capture thread; read by stop().
    native_rate: Arc<Mutex<u32>>,
    native_channels: Arc<Mutex<u16>>,
    /// Owns the WASAPI objects (not `Send`) — joined on stop.
    capture_thread: Option<std::thread::JoinHandle<()>>,
}

impl SystemAudioRecorder {
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            is_recording: Arc::new(Mutex::new(false)),
            native_rate: Arc::new(Mutex::new(48000)),
            native_channels: Arc::new(Mutex::new(2)),
            capture_thread: None,
        }
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.lock().map(|r| *r).unwrap_or(false)
    }

    /// Start WASAPI loopback capture of the default render device. Returns an error
    /// (mode unavailable — spec §6) if the device can't be bound; the mic is unaffected.
    /// Method name mirrors `AudioRecorder::start_recording` (spec §4.1 shorthand `start()`).
    pub fn start_recording(&mut self) -> Result<(), String> {
        // Re-entrancy backstop — mirror AudioRecorder (audio.rs:135). Per-recorder.
        if self.is_recording() {
            return Err("הקלטה כבר פעילה — עצור אותה לפני התחלת הקלטה חדשה".to_string());
        }

        {
            let mut buf = self.samples.lock().map_err(|e| e.to_string())?;
            buf.clear();
        }
        {
            let mut rec = self.is_recording.lock().map_err(|e| e.to_string())?;
            *rec = true;
        }

        let samples = self.samples.clone();
        let is_recording = self.is_recording.clone();
        let native_rate = self.native_rate.clone();
        let native_channels = self.native_channels.clone();
        // Surface loopback bind failures back to the caller before returning.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        let handle = std::thread::spawn(move || {
            // WASAPI/COM objects are not Send — create and own them entirely here.
            // initialize_mta() returns an HRESULT; `.ok()` → windows Result, then is_err().
            if initialize_mta().ok().is_err() {
                let _ = ready_tx.send(Err("COM init (MTA) failed".to_string()));
                return;
            }
            // Loopback source = the default RENDER (playback) device. wasapi 0.23:
            // get_default_device is a DeviceEnumerator method, not a free function.
            let device = match DeviceEnumerator::new().and_then(|e| e.get_default_device(&Direction::Render)) {
                Ok(d) => d,
                Err(e) => { let _ = ready_tx.send(Err(format!("No default render device: {e}"))); return; }
            };
            let mut audio_client = match device.get_iaudioclient() {
                Ok(c) => c,
                Err(e) => { let _ = ready_tx.send(Err(format!("get_iaudioclient failed: {e}"))); return; }
            };
            // Native shared-mode mix format — 32-bit float, 48k/44.1k, usually stereo.
            let format = match audio_client.get_mixformat() {
                Ok(f) => f,
                Err(e) => { let _ = ready_tx.send(Err(format!("get_mixformat failed: {e}"))); return; }
            };
            let rate = format.get_samplespersec();
            let channels = format.get_nchannels();
            // get_device_period replaces the deprecated get_periods; (default, min) in hns.
            let (default_period, _min_period) = match audio_client.get_device_period() {
                Ok(p) => p,
                Err(e) => { let _ = ready_tx.send(Err(format!("get_device_period failed: {e}"))); return; }
            };
            // ...initialized for CAPTURE. In wasapi 0.23 the Render-device + Direction::Capture
            // mismatch IS the loopback selector — no ShareMode/period/bool args anymore.
            // Shared, event-driven, autoconvert:true so `format` is honored as-is.
            let mode = StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: default_period,
            };
            if let Err(e) = audio_client.initialize_client(&format, &Direction::Capture, &mode) {
                let _ = ready_tx.send(Err(format!("loopback initialize_client failed: {e}")));
                return;
            }
            let h_event = match audio_client.set_get_eventhandle() {
                Ok(h) => h,
                Err(e) => { let _ = ready_tx.send(Err(format!("set_get_eventhandle failed: {e}"))); return; }
            };
            // `read_from_device_to_deque` takes `&self` → non-mut binding is fine.
            let capture_client = match audio_client.get_audiocaptureclient() {
                Ok(c) => c,
                Err(e) => { let _ = ready_tx.send(Err(format!("get_audiocaptureclient failed: {e}"))); return; }
            };

            // Publish the native format so stop() resamples correctly.
            if let Ok(mut r) = native_rate.lock() { *r = rate; }
            if let Ok(mut c) = native_channels.lock() { *c = channels; }

            if let Err(e) = audio_client.start_stream() {
                let _ = ready_tx.send(Err(format!("start_stream failed: {e}")));
                return;
            }
            // Bind succeeded — unblock the caller.
            let _ = ready_tx.send(Ok(()));

            let mut raw: VecDeque<u8> = VecDeque::new();
            loop {
                if is_recording.lock().map(|r| !*r).unwrap_or(true) {
                    break;
                }
                // Wake on the next buffer or re-check the stop flag within ~100ms.
                let _ = h_event.wait_for_event(100);
                // read_from_device_to_deque -> Result<BufferInfo, _>; .is_err() still applies.
                if capture_client.read_from_device_to_deque(&mut raw).is_err() {
                    break;
                }
                if !raw.is_empty() {
                    let bytes: Vec<u8> = raw.drain(..).collect();
                    if let Ok(mut buf) = samples.lock() {
                        // Shared-mode mix format is 32-bit IEEE float, interleaved.
                        for frame in bytes.chunks_exact(4) {
                            buf.push(f32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]));
                        }
                    }
                }
            }
            let _ = audio_client.stop_stream();
        });

        // Wait for the capture thread to confirm the loopback bind succeeded.
        let ready = ready_rx
            .recv()
            .unwrap_or_else(|_| Err("Capture thread exited before signaling".to_string()));
        if let Err(e) = ready {
            // Loopback bind failed (spec §6): reset is_recording so the recorder stays
            // RECOVERABLE — otherwise the re-entrancy guard rejects every retry until an
            // unpaired stop. The mic (separate AudioRecorder) is unaffected.
            if let Ok(mut rec) = self.is_recording.lock() {
                *rec = false;
            }
            let _ = handle.join();
            return Err(e);
        }

        self.capture_thread = Some(handle);
        Ok(())
    }

    /// Stop capture and return the buffer as 16kHz mono f32 — mirrors
    /// `AudioRecorder::stop_recording` (audio.rs:470-504); spec §4.1 shorthand `stop()`.
    pub fn stop_recording(&mut self) -> Result<Vec<f32>, String> {
        {
            let mut rec = self.is_recording.lock().map_err(|e| e.to_string())?;
            *rec = false;
        }
        if let Some(handle) = self.capture_thread.take() {
            let _ = handle.join();
        }
        let raw = self.samples.lock().map_err(|e| e.to_string())?.clone();
        let rate = *self.native_rate.lock().map_err(|e| e.to_string())?;
        let channels = *self.native_channels.lock().map_err(|e| e.to_string())?;
        Ok(resample_to_16k_mono(&raw, rate, channels))
    }
}
```
- [ ] Step 4: Run test to verify it passes. First confirm the whole crate + wasapi 0.23.0 code **compiles** and the manual test is compiled-but-skipped: `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml loopback_captures_playing_audio`. Expected: builds cleanly; `test system_audio_tests::loopback_captures_playing_audio ... ignored`. **MANUAL-VERIFY (real device, audio playing):** `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml loopback_captures_playing_audio -- --ignored`. Expected while a video/song plays: `... ok` (captured `samples.len() > 16000`).
- [ ] Step 5: Commit —
```
git -C C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/system_audio.rs
git -C C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation commit -m "feat(system-audio): wasapi 0.23 loopback capture start/stop (manual-verify)" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Chunk 5: `lib.rs` orchestration + `source` param

Wires the source axis through the backend. Tasks 10-13 are pure, strictly TDD'd `batch.rs`/`lib.rs` helpers (the routing decision + guards). Tasks 14-16 are the `#[tauri::command]`/`AppState` wiring — verified by a green full build (matching the repo convention that `start_batch_recording`/`stop_batch_recording_to_file` carry no unit tests, and spec §7, which leaves capture + orchestration to manual Windows testing) with the genuinely-new decision logic pulled down into the tested Task 12 helper.

> **Chunk 5 depends on Chunks 1, 2, 4 being merged** (their APIs are called here) **and Chunk 3** (the `render_srt` 2-arg caller at `lib.rs:1201` must already read the 2-arg form). Reconciled `SystemAudioRecorder` surface consumed here: `new()`, `is_recording()`, `start_recording(&mut self) -> Result<(), String>`, `stop_recording(&mut self) -> Result<Vec<f32>, String>` (Seam #1).

### Task 10: `RecordingSource` enum (source axis, distinct from `BatchOpts.mode`)
**Files:** Modify `src-tauri/src/batch.rs` (add enum after `BatchOpts`, ~line 17); Test in `src-tauri/src/batch.rs` (append to `mod tests`, lines 42-53).

- [ ] Step 1: Write the failing test — append inside the existing `#[cfg(test)] mod tests` (before its closing brace):
```rust
    #[test]
    fn recording_source_deserializes_and_defaults_to_mic() {
        use serde_json::from_str;
        // Frontend sends lowercase strings for the source toggle.
        assert_eq!(from_str::<RecordingSource>("\"mic\"").unwrap(), RecordingSource::Mic);
        assert_eq!(from_str::<RecordingSource>("\"system\"").unwrap(), RecordingSource::System);
        assert_eq!(from_str::<RecordingSource>("\"call\"").unwrap(), RecordingSource::Call);
        // Zero-regression default: an absent/legacy `source` must fall back to Mic.
        assert_eq!(RecordingSource::default(), RecordingSource::Mic);
    }
```
- [ ] Step 2: Run test to verify it fails — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml recording_source_deserializes_and_defaults_to_mic`. Expected: `E0433: cannot find type RecordingSource`.
- [ ] Step 3: Write minimal implementation — add after the `BatchOpts` struct (~line 17). `use serde::Deserialize;` is already imported (batch.rs:4):
```rust
/// Which audio source a batch recording captures. This is a DIFFERENT axis from
/// `BatchOpts.mode` (cloud/local): `mode` picks the transcription engine, `source`
/// picks what is recorded. Named `source` precisely so it never collides with
/// `mode`. Defaults to `Mic` (existing behavior, zero regression when the frontend
/// omits it). Spec §3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingSource {
    /// Existing cpal microphone path (mono).
    #[default]
    Mic,
    /// WASAPI loopback of the default render device (Windows-only).
    System,
    /// Mic + system captured together, interleaved to stereo for multichannel.
    Call,
}
```
- [ ] Step 4: Run test to verify it passes — same filter. Expected: `test result: ok. 1 passed`.
- [ ] Step 5: Commit — `git commit -am "feat(batch): add RecordingSource enum (Mic/System/Call) distinct from BatchOpts.mode" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"`

---

### Task 11: Call/Deepgram availability guard (fail before recording if no key)
**Files:** Modify `src-tauri/src/batch.rs` (add fn after `pick_batch_route`, ~line 40); Test in `src-tauri/src/batch.rs`.

- [ ] Step 1: Write the failing test — append inside `#[cfg(test)] mod tests`:
```rust
    #[test]
    fn call_requires_a_deepgram_key() {
        // Key present → Call proceeds (via cloud) EVEN when BatchOpts.mode="local".
        assert!(ensure_call_deepgram_available(true).is_ok());
        // No key at all → a guiding Hebrew error, raised BEFORE recording starts.
        let err = ensure_call_deepgram_available(false).unwrap_err();
        assert!(err.contains("Deepgram"));
        assert!(err.contains("שיחה"));
    }
```
- [ ] Step 2: Run test to verify it fails — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml call_requires_a_deepgram_key`. Expected: `E0425: cannot find function ensure_call_deepgram_available`.
- [ ] Step 3: Write minimal implementation — add after `pick_batch_route` (~line 40):
```rust
/// Call mode is Deepgram-only (multichannel), so it needs a Deepgram key even when
/// `BatchOpts.mode="local"`: with a key present Call transparently forces cloud;
/// with no key at all we fail fast — BEFORE recording — with a guiding message,
/// rather than capturing audio that can't be transcribed. Spec §6.
pub fn ensure_call_deepgram_available(has_deepgram_key: bool) -> Result<(), String> {
    if has_deepgram_key {
        Ok(())
    } else {
        Err("מצב שיחה דורש מפתח Deepgram. הוסף אותו בהגדרות.".to_string())
    }
}
```
- [ ] Step 4: Run test to verify it passes — same filter. Expected: `test result: ok. 1 passed`.
- [ ] Step 5: Commit — `git commit -am "feat(batch): add Call/Deepgram availability guard" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"`

---

### Task 12: `recorders_for_source` — pure Mic/System/Call routing table
**Files:** Modify `src-tauri/src/batch.rs` (add fn after `ensure_call_deepgram_available`); Test in `src-tauri/src/batch.rs`.

Rationale: the genuinely-new "which recorder(s) does each source start/stop" branching is otherwise buried in un-unit-tested command code. Extracting the decision into a pure table gives it a real red→green test; `start_recorders_for_source` (Task 15) consults it so the test is load-bearing — a Mic/System/Call mis-route fails **here**, not only in manual Windows testing.

- [ ] Step 1: Write the failing test — append inside `#[cfg(test)] mod tests`:
```rust
    #[test]
    fn recorders_for_source_maps_each_variant() {
        // (uses_mic, uses_system) — the routing table the lib.rs start/stop wiring
        // keys off. Locked down so a Mic/System/Call mis-route fails HERE.
        assert_eq!(recorders_for_source(RecordingSource::Mic), (true, false));
        assert_eq!(recorders_for_source(RecordingSource::System), (false, true));
        // Call is the ONLY source that drives BOTH recorders → stereo/multichannel.
        assert_eq!(recorders_for_source(RecordingSource::Call), (true, true));
    }
```
- [ ] Step 2: Run test to verify it fails — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml recorders_for_source_maps_each_variant`. Expected: `E0425: cannot find function recorders_for_source`.
- [ ] Step 3: Write minimal implementation — add after `ensure_call_deepgram_available`:
```rust
/// Which physical recorders a batch `source` drives, as `(uses_mic, uses_system)`.
/// Pure decision table (spec §3, §4.6) so the Mic/System/Call routing is unit-tested
/// and can't silently regress — `start_recorders_for_source` in lib.rs keys off it,
/// making this the single source of truth for "what does each source capture".
pub fn recorders_for_source(source: RecordingSource) -> (bool, bool) {
    match source {
        RecordingSource::Mic => (true, false),
        RecordingSource::System => (false, true),
        RecordingSource::Call => (true, true),
    }
}
```
- [ ] Step 4: Run test to verify it passes — same filter. Expected: `test result: ok. 1 passed`.
- [ ] Step 5: Commit — `git commit -am "feat(batch): add recorders_for_source routing table (Mic/System/Call)" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"`

---

### Task 13: `call_stereo_wav_or_silent` — silence guard on the INTERLEAVED buffer + stereo WAV
**Files:** Modify `src-tauri/src/lib.rs` (add fn after `run_transcribe_file`, before `write_wav_16k_mono`, ~line 505); Test in `src-tauri/src/lib.rs` (append to `mod tests`, ~lines 1839-1859).
**Depends on:** `api_transcribe::samples_to_wav_stereo` (Task 2). Windows-only (Call is Windows-only).
**Dead-code note:** this fn's first non-test caller arrives in Task 16. Under `cargo test` (Step 4) the Windows test is a caller, so no warning. Under a plain `cargo build` (Tasks 14-15 build steps) it emits **one harmless `dead_code` warning** on Windows until Task 16 wires it — no `deny(warnings)`, so not an error.

- [ ] Step 1: Write the failing test — append inside `lib.rs`'s existing `#[cfg(test)] mod tests` (before its closing brace):
```rust
    #[cfg(target_os = "windows")]
    #[test]
    fn call_stereo_wav_or_silent_blocks_silence_and_wraps_audio() {
        // Call bypasses the stop_batch/transcribe silence guards, so this helper
        // must block silence itself — on the COMBINED buffer, before any network call.
        assert!(call_stereo_wav_or_silent(&[]).is_err());
        assert!(call_stereo_wav_or_silent(&vec![0.0f32; 16000]).is_err());

        // Non-silent interleaved buffer → a 2-channel WAV body for multichannel.
        let loud = vec![0.5f32; 16000];
        let wav = call_stereo_wav_or_silent(&loud).expect("loud audio must pass the guard");
        assert_eq!(&wav[0..4], b"RIFF");
        // Byte 22 (u16 LE) is the WAV channel count — Call MUST send stereo so
        // Deepgram can separate "אני"/"הצד השני".
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2);
    }
```
- [ ] Step 2: Run test to verify it fails — `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml call_stereo_wav_or_silent_blocks_silence_and_wraps_audio`. Expected: `E0425: cannot find function call_stereo_wav_or_silent`.
- [ ] Step 3: Write minimal implementation — add after `run_transcribe_file` (before `write_wav_16k_mono`, ~line 505):
```rust
/// Call bypasses `stop_batch_recording_to_file` / `run_transcribe_file`, so the
/// near-silence guard that normally lives there (lib.rs:595, 423) must run HERE, on
/// the INTERLEAVED buffer — never per-channel: a call where only one side spoke
/// still has content in the combined buffer and must NOT be blocked (spec §6). On
/// pass, encodes the two-channel WAV body posted to Deepgram `multichannel=true`.
/// Reuses the 0.005 threshold from the mono stop-recording guard it replaces.
#[cfg(target_os = "windows")]
fn call_stereo_wav_or_silent(interleaved: &[f32]) -> Result<Vec<u8>, String> {
    if interleaved.is_empty() || audio::is_effectively_silent(interleaved, 0.005) {
        return Err("לא נקלט אודיו בשיחה — ודאו שהמיקרופון ולכידת-המערכת פעילים.".to_string());
    }
    Ok(api_transcribe::samples_to_wav_stereo(interleaved, 16000))
}
```
- [ ] Step 4: Run test to verify it passes — same filter. Expected: `test result: ok. 1 passed` (no dead_code warning — the test references the fn).
- [ ] Step 5: Commit — `git commit -am "feat(batch): add call_stereo_wav_or_silent (interleaved silence guard + stereo WAV)" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"`

---

### Task 14: Wire `system_recorder` into `AppState` (module + field + init)
**Files:** Modify `src-tauri/src/lib.rs` (module decl ~line 13 — already added in Task 7; `AppState` field ~line 31; `.manage` init ~line 1623).
**Depends on:** `system_audio::SystemAudioRecorder::new` (Task 7).
**Why no red step:** pure compile-gated wiring, not TDD. `mod system_audio;` already resolves (Task 7); Step 1 adds no `system_recorder` *reference* on its own, so neither `E0583` nor `E0609` can occur — the crate would build green. Do the two remaining edits together and verify one green build. (AppState construction is untested repo-wide; the field is exercised by Tasks 15-16.)

- [ ] Step 1: Apply both edits in one pass.
  (a) Field in `AppState` right after `recorder: Mutex<AudioRecorder>,` (line 31):
```rust
    /// System-audio (WASAPI loopback) recorder for `System`/`Call` sources.
    /// Windows-only (spec §4.1, §6) and independent of `recorder` (the mic) —
    /// the "already recording" guard is per-recorder, so both can run at once.
    #[cfg(target_os = "windows")]
    system_recorder: Mutex<system_audio::SystemAudioRecorder>,
```
  (b) Initializer in the `.manage({ ... AppState { ... } })` literal right after `recorder: Mutex::new(AudioRecorder::new()),` (line 1623):
```rust
                #[cfg(target_os = "windows")]
                system_recorder: Mutex::new(system_audio::SystemAudioRecorder::new()),
```
  (The `#[cfg(target_os = "windows")] mod system_audio;` module decl at line 13 was added in Task 7.)
- [ ] Step 2: Verify one green build — `cargo build --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml`. Expected: compiles. On Windows a **single expected `dead_code` warning** for `call_stereo_wav_or_silent` (Task 13, wired in Task 16) is present and harmless. Then `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml` — all existing tests plus Tasks 10-13 pass (no regression; the Task-13 test references the fn so `cargo test` silences the warning).
- [ ] Step 3: Commit — `git commit -am "feat(batch): add system_recorder (WASAPI loopback) to AppState" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"`

---

### Task 15: Make `start_batch_recording` source-aware (Mic unchanged, System, Call + pre-record guard)
**Files:** Modify `src-tauri/src/lib.rs` (`start_batch_recording` ~line 550; add two helpers below it).
**Depends on:** Tasks 10, 11, 12, 14; `system_audio::SystemAudioRecorder::start_recording` (Task 9). Compile-gated wiring; the routing it performs is covered by Task 12's test via `recorders_for_source`.

- [ ] Step 1: Establish the failing signal — replace the body of `start_batch_recording` so it takes `source` and branches. Before the two helper fns exist this is a real red (`E0425 cannot find function start_recorders_for_source`):
```rust
#[tauri::command]
fn start_batch_recording(
    state: State<AppState>,
    source: Option<batch::RecordingSource>,
) -> Result<(), String> {
    let source = source.unwrap_or_default();

    if state.batch_in_progress.load(Ordering::SeqCst) {
        return Err("תמלול בתהליך — המתן לסיומו לפני הקלטה".to_string());
    }
    // Symmetric to C1: a live streaming session also owns the mic recorder. `try_lock`
    // keeps this command sync — a held lock means streaming is mid setup/teardown.
    match state.streaming.try_lock() {
        Ok(guard) if guard.is_some() => {
            return Err("חיבור streaming פעיל — עצור אותו לפני הקלטת ישיבה".to_string());
        }
        Err(_) => {
            return Err("מצב הקלטה בלתי-יציב כרגע — נסה שוב בעוד רגע".to_string());
        }
        Ok(_) => {}
    }

    // Call needs Deepgram (multichannel) — fail BEFORE recording if no key exists,
    // so the user isn't left with an un-transcribable capture (spec §6).
    if matches!(source, batch::RecordingSource::Call) {
        let has_key = {
            let s = state.settings.lock().map_err(|e| e.to_string())?;
            s.deepgram_api_key.as_ref().is_some_and(|k| !k.is_empty())
        };
        batch::ensure_call_deepgram_available(has_key)?;
    }

    if state.batch_recording_in_progress.swap(true, Ordering::SeqCst) {
        return Err("הקלטה כבר בתהליך".to_string());
    }

    let result = start_recorders_for_source(&state, source);
    if result.is_err() {
        // Never leave the guard set on a failed start, or short dictation stays blocked.
        state.batch_recording_in_progress.store(false, Ordering::SeqCst);
    }
    result
}
```
- [ ] Step 2: Run build to confirm red — `cargo build --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml`. Expected: fails (`start_recorders_for_source` / `start_mic_batch` undefined).
- [ ] Step 3: Complete the implementation — add both helpers immediately after `start_batch_recording`. `start_recorders_for_source` consults the tested `batch::recorders_for_source` table (Task 12) so the routing is load-bearing. **Seam #1 reconciliation is baked in: `sys.start_recording()`, not `sys.start()`.**
```rust
/// Mic batch-recording setup, factored out so Mic and Call share exactly one code
/// path (VAD off — user stops manually; 1-hour hard ceiling in AudioRecorder).
fn start_mic_batch(state: &State<AppState>) -> Result<(), String> {
    let mut recorder = state.recorder.lock().map_err(|e| e.to_string())?;
    recorder.set_vad_enabled(false);
    recorder.set_max_recording_secs(3600.0);
    recorder.start_recording()
}

/// Start the recorder(s) a batch `source` needs, per the tested `recorders_for_source`
/// table (Task 12): Mic = existing cpal path; System = loopback only; Call = BOTH, so
/// the two sides can be interleaved at stop. System/Call are Windows-only (spec §6);
/// Call rolls the mic back if loopback fails so no half-open capture is left behind.
fn start_recorders_for_source(
    state: &State<AppState>,
    source: batch::RecordingSource,
) -> Result<(), String> {
    let (uses_mic, uses_system) = batch::recorders_for_source(source);

    // Reject System/Call up front on non-Windows — BEFORE starting the mic — so an
    // unsupported platform never leaves a half-open mic capture running.
    #[cfg(not(target_os = "windows"))]
    if uses_system {
        return Err("לכידת אודיו-מערכת נתמכת רק ב-Windows".to_string());
    }

    if uses_mic {
        start_mic_batch(state)?;
    }

    #[cfg(target_os = "windows")]
    if uses_system {
        let mut sys = state.system_recorder.lock().map_err(|e| e.to_string())?;
        if let Err(e) = sys.start_recording() {
            drop(sys);
            // Roll back a mic we started for Call — no half-open capture behind us.
            if uses_mic {
                if let Ok(mut rec) = state.recorder.lock() {
                    let _ = rec.stop_recording();
                }
            }
            return Err(e);
        }
    }
    Ok(())
}
```
- [ ] Step 4: Run build + suite to verify green — `cargo build --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml` (compiles; on Windows the same expected `dead_code` warning for `call_stereo_wav_or_silent` persists until Task 16) then `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml` (all pass; Mic path behavior byte-for-byte unchanged).
- [ ] Step 5: Commit — `git commit -am "feat(batch): source-aware start_batch_recording (Mic/System/Call + Call key guard)" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"`

---

### Task 16: Source-aware stop + `stop_call_recording` command + handler registration
**Files:** Modify `src-tauri/src/lib.rs` (`stop_batch_recording_to_file` ~line 587; add helper + new commands after it; register in `generate_handler!` — the handler list opens at ~line 1781, after the existing `stop_batch_recording_to_file,` entry).
**Depends on:** Tasks 10, 12, 13, 14, 15; `api_transcribe::transcribe_deepgram_multichannel` (Task 5), `audio::interleave_stereo` (Task 1), `system_audio::SystemAudioRecorder::stop_recording` (Task 9). Compile-gated wiring. This task supplies `call_stereo_wav_or_silent`'s first non-test caller, so its intermediate `dead_code` warning clears here.

> **Seam #1 baked in:** `stop_recorder_for_source` returns `sys.stop_recording()` directly (already a `Result<Vec<f32>, String>`); `run_stop_call_recording` uses `sys.stop_recording()?`. No bare `sys.stop()` remains.

- [ ] Step 1: Establish the failing signal — replace `stop_batch_recording_to_file` to take `source` and reject Call; add the `stop_call_recording` reference in `generate_handler!` (after the existing `stop_batch_recording_to_file,` entry). Before the new fns exist this is red (`E0425 cannot find value stop_call_recording` in the handler macro). Replace the stop fn:
```rust
#[tauri::command]
async fn stop_batch_recording_to_file(
    state: State<'_, AppState>,
    source: Option<batch::RecordingSource>,
) -> Result<String, String> {
    let source = source.unwrap_or_default();
    // Call is NOT a mono file path — it interleaves two channels and transcribes
    // inline; the frontend must call `stop_call_recording` instead.
    if matches!(source, batch::RecordingSource::Call) {
        return Err("מצב שיחה נעצר דרך stop_call_recording ולא דרך מסלול-הקובץ".to_string());
    }

    let samples = stop_recorder_for_source(&state, source)?;
    state.batch_recording_in_progress.store(false, Ordering::SeqCst);
    restore_recorder_settings(&state);

    if samples.is_empty() || audio::is_effectively_silent(&samples, 0.005) {
        return Err("לא נקלט אודיו — ודא שהמיקרופון מחובר ופעיל.".to_string());
    }

    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let tmp_path = std::env::temp_dir().join(format!("hd-recording-{}.wav", epoch));
    write_wav_16k_mono(&tmp_path, &samples)?;
    Ok(tmp_path.to_string_lossy().to_string())
}
```
- [ ] Step 2: Run build to confirm red — `cargo build --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml`. Expected: fails (`stop_recorder_for_source` and `stop_call_recording` undefined).
- [ ] Step 3: Complete the implementation — add the helper + both command variants immediately after `stop_batch_recording_to_file`, and register the command in `generate_handler!`. Note the poisoned-lock guard fix in `run_stop_call_recording` (`batch_recording_in_progress` cleared FIRST, mirroring `cancel_batch_recording` at lib.rs:610 and `start_batch_recording`'s rollback) so a poisoned recorder lock can't leave short dictation blocked:
```rust
/// Stop and drain the recorder a non-Call `source` used, returning its mono samples.
fn stop_recorder_for_source(
    state: &State<AppState>,
    source: batch::RecordingSource,
) -> Result<Vec<f32>, String> {
    match source {
        batch::RecordingSource::Mic => {
            let mut recorder = state.recorder.lock().map_err(|e| e.to_string())?;
            recorder.stop_recording()
        }
        #[cfg(target_os = "windows")]
        batch::RecordingSource::System => {
            let mut sys = state.system_recorder.lock().map_err(|e| e.to_string())?;
            sys.stop_recording()
        }
        #[cfg(target_os = "windows")]
        batch::RecordingSource::Call => unreachable!("Call is handled before this call"),
        #[cfg(not(target_os = "windows"))]
        batch::RecordingSource::System | batch::RecordingSource::Call => {
            Err("לכידת אודיו-מערכת נתמכת רק ב-Windows".to_string())
        }
    }
}

/// Stop a `Call` recording: drain BOTH recorders, interleave to stereo, and
/// transcribe via Deepgram `multichannel=true` — bypassing the mono file path
/// (`transcribe_file` → `decode_file_to_16k_mono` would collapse the channels).
/// Returns a `TranscribeFileResult` so every existing consumer (inject/copy/TXT/
/// DOCX/SRT) works unchanged. `opts.mode` is ignored: Call always uses Deepgram
/// (spec §6). Mirrors `transcribe_file`'s one-batch-at-a-time guard + cancel.
#[cfg(target_os = "windows")]
#[tauri::command]
async fn stop_call_recording(
    app: AppHandle,
    state: State<'_, AppState>,
    opts: batch::BatchOpts,
) -> Result<TranscribeFileResult, String> {
    if state.batch_in_progress.swap(true, Ordering::SeqCst) {
        return Err("תמלול ארוך כבר רץ — המתן לסיומו או בטל אותו".to_string());
    }
    state.batch_cancel.store(false, Ordering::SeqCst);
    let result = run_stop_call_recording(&app, &state, opts).await;
    state.batch_in_progress.store(false, Ordering::SeqCst);
    result
}

#[cfg(target_os = "windows")]
async fn run_stop_call_recording(
    app: &AppHandle,
    state: &State<'_, AppState>,
    opts: batch::BatchOpts,
) -> Result<TranscribeFileResult, String> {
    // Clear the recording guard FIRST — mirrors cancel_batch_recording (lib.rs:610)
    // and start_batch_recording's rollback. If either recorder lock is poisoned
    // below, the `?` returns early; leaving this flag set would block short
    // dictation (Alt+D) until app restart. `batch_in_progress` is cleared by the
    // outer stop_call_recording, so without this only the recording guard leaks.
    state.batch_recording_in_progress.store(false, Ordering::SeqCst);

    // Stop both captures (mic first, matching start order); always drain system so
    // its buffer never leaks into a later session.
    let mic = {
        let mut recorder = state.recorder.lock().map_err(|e| e.to_string())?;
        recorder.stop_recording()?
    };
    let system = {
        let mut sys = state.system_recorder.lock().map_err(|e| e.to_string())?;
        sys.stop_recording()?
    };
    restore_recorder_settings(state);

    // Interleave L=mic / R=system; the near-silence guard runs on the COMBINED
    // buffer (§6) — a one-sided call is NOT blocked because the buffer has content.
    let interleaved = audio::interleave_stereo(&mic, &system);
    let stereo_wav = call_stereo_wav_or_silent(&interleaved)?;

    // Call forces Deepgram regardless of opts.mode; key was checked before recording,
    // re-check defensively here.
    let key = {
        let s = state.settings.lock().map_err(|e| e.to_string())?;
        s.deepgram_api_key.clone()
    };
    let key = key
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "מצב שיחה דורש מפתח Deepgram. הוסף אותו בהגדרות.".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(900))
        .build()
        .map_err(|e| format!("שגיאת לקוח רשת: {}", e))?;

    let _ = app.emit(
        "batch-progress",
        serde_json::json!({ "stage": "transcribing", "pct": 0 }),
    );

    let notify = state.batch_cancel_notify.clone();
    let fut =
        api_transcribe::transcribe_deepgram_multichannel(&client, stereo_wav, &key, &opts.language);
    let (text, segments) = tokio::select! {
        r = fut => r.map_err(|e| e.to_string())?,
        _ = notify.notified() => return Err(batch::CANCELLED.to_string()),
    };
    let _ = app.emit("batch-progress", serde_json::json!({ "stage": "done", "pct": 100 }));
    Ok(TranscribeFileResult { text, segments })
}

/// Non-Windows stub so the command is always registrable in `generate_handler!`.
#[cfg(not(target_os = "windows"))]
#[tauri::command]
async fn stop_call_recording(
    _app: AppHandle,
    _state: State<'_, AppState>,
    _opts: batch::BatchOpts,
) -> Result<TranscribeFileResult, String> {
    Err("מצב שיחה נתמך רק ב-Windows".to_string())
}
```
Then register it in the `invoke_handler(tauri::generate_handler![ ... ])` list, immediately after the existing `stop_batch_recording_to_file,` entry:
```rust
            stop_call_recording,
```
- [ ] Step 4: Run build + suite to verify green — `cargo build --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml` (compiles clean — `call_stereo_wav_or_silent` now has a non-test caller, so the earlier dead_code warning is gone) then `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml` (full suite green; Tasks 1-13 tests included).
- [ ] Step 5: Commit — `git commit -am "feat(batch): source-aware stop + stop_call_recording command (Call → multichannel Deepgram)" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"`

---

## Chunk 6: Frontend source selector

All edits are in `src/App.tsx` only, reusing the existing `batch-mode-card*` CSS (no `App.css` change). **Test-first mapping (override):** the frontend has no Rust/JS test runner, so the "test" per task is a mechanical `git grep` landed-marker (flips to pass the instant the literal is typed — not a behavioral test). The **real safety net is `npx tsc --noEmit`** (proves `RecordingSource` is threaded through the union/state/invoke payloads/dep-arrays at the type level) **plus a manual `npm run tauri dev` preview**. Verified anchors: `APP_LICENSE` line 11, `type ApiProvider` line 66, `batchRecording` state line 414, `handleStartBatchRecord` lines 1024-1036, `handleStopBatchRecord` lines 1038-1090 (`stop_batch_recording_to_file` at 1047, `transcribe_file` at 1069-1072), mode-selector `aria-label="מצב תמלול"` line 2361, anchors `</div>` (2382) and `{/* Actions … */}` (2384) both at 8-space indent.

### Task 17: Add Mic/System/Call recording-source selector to the batch view (Windows-gated)
**Files:** Modify `src/App.tsx` (types ~line 66, module const ~line 11, state ~line 414, JSX ~line 2382).

- [ ] Step 1: Write the landed-marker check. The machine anchor lives in a **code comment** (non-user-facing) so the a11y label stays clean Hebrew:
  ```
  git grep -n "batch-source-selector" -- src/App.tsx
  ```
  Expected once implemented: exactly one match. Before implementation: zero matches.
- [ ] Step 2: Run the check to verify it is absent. From the project dir:
  ```
  cd C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation
  git grep -n "batch-source-selector" -- src/App.tsx
  ```
  Expected ABSENT: no output (exit 1).
- [ ] Step 3: Write minimal implementation. Four edits, all in `src/App.tsx` (`noUnusedLocals` is on, so type + const + state + UI land together — each new symbol is consumed by the JSX):

  Edit A — add the source union type after line 66 (`type ApiProvider = ...`):
  ```tsx
  type ApiProvider = "deepgram" | "groq";
  /** Recording input source, chosen before a batch recording and threaded into
   * `start_batch_recording` (Task 18). Default "mic" = existing behavior (zero
   * regression). "system" (WASAPI loopback) and "call" (mic + system) are
   * Windows-only — see IS_WINDOWS gating in the batch view. Wire values are
   * lowercase to match the app's existing `mode`/`language` invoke payloads. */
  type RecordingSource = "mic" | "system" | "call";
  ```

  Edit B — add the platform constant at module scope after line 11 (`const APP_LICENSE = "MIT";`):
  ```tsx
  const APP_LICENSE = "MIT";
  // System-audio capture (WASAPI loopback) is Windows-only, so System/Call are
  // hidden off-Windows — non-Windows users only ever see Mic (zero regression).
  // WebView2 on Windows always reports "Windows" in navigator.userAgent.
  const IS_WINDOWS =
    typeof navigator !== "undefined" && navigator.userAgent.includes("Windows");
  ```

  Edit C — add the state after line 414 (`const [batchRecording, setBatchRecording] = useState(false);`):
  ```tsx
  const [batchRecording, setBatchRecording] = useState(false);
  // Recording source for the batch record button. Default "mic" = zero regression.
  const [recordSource, setRecordSource] = useState<RecordingSource>("mic");
  ```

  Edit D — insert the selector between the mode-selector's closing `</div>` (line 2382) and the `{/* Actions ... */}` comment (line 2384) — **both anchor lines sit at 8-space indent; match that.** Reuses the existing `batch-mode-cards`/`batch-mode-card` classes (no CSS change), keeps `aria-label` clean Hebrew, renders only while not recording:
  ```tsx
        </div>

        {/* batch-source-selector — recording source (Mic / System / Call), chosen
            before recording. System/Call are Windows-only (WASAPI loopback) →
            hidden off-Windows. Reuses batch-mode-card styles; no new CSS.
            (The machine anchor for git grep lives HERE in the comment, NOT in the
            aria-label, which stays clean Hebrew per the repo's UI convention.) */}
        {!batchRecording && (
          <div className="batch-mode-cards" role="group" aria-label="מקור הקלטה">
            <button
              className={`batch-mode-card ${recordSource === "mic" ? "active" : ""}`}
              onClick={() => !batchRunning && setRecordSource("mic")}
              disabled={batchRunning}
              aria-pressed={recordSource === "mic"}
            >
              <span className="batch-mode-icon" aria-hidden="true">🎙</span>
              <span className="batch-mode-name">מיקרופון</span>
              <span className="batch-mode-desc">הקול שלכם בלבד</span>
            </button>
            {IS_WINDOWS && (
              <>
                <button
                  className={`batch-mode-card ${recordSource === "system" ? "active" : ""}`}
                  onClick={() => !batchRunning && setRecordSource("system")}
                  disabled={batchRunning}
                  aria-pressed={recordSource === "system"}
                >
                  <span className="batch-mode-icon" aria-hidden="true">🔊</span>
                  <span className="batch-mode-name">אודיו מערכת</span>
                  <span className="batch-mode-desc">מה שמתנגן במחשב</span>
                </button>
                <button
                  className={`batch-mode-card ${recordSource === "call" ? "active" : ""}`}
                  onClick={() => !batchRunning && setRecordSource("call")}
                  disabled={batchRunning}
                  aria-pressed={recordSource === "call"}
                >
                  <span className="batch-mode-icon" aria-hidden="true">📞</span>
                  <span className="batch-mode-name">שיחה</span>
                  <span className="batch-mode-desc">אתם + הצד השני</span>
                </button>
              </>
            )}
          </div>
        )}

        {/* Actions — pinned near the top so a growing result list never pushes them off-screen */}
  ```
- [ ] Step 4: Verify it passes (the real safety net). Typecheck first (the meaningful gate), then the landed-marker, then eyeball it:
  ```
  cd C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation
  npx tsc --noEmit
  git grep -n "batch-source-selector" -- src/App.tsx
  ```
  Expected: `tsc` prints nothing (exit 0); `git grep` prints one match. Then `npm run tauri dev`, open the batch view ("תמלול קובץ"): on Windows three cards appear — מיקרופון / אודיו מערכת / שיחה — with מיקרופון active by default; clicking a card moves the `active` highlight; the cards disappear the moment recording starts. (Off-Windows only מיקרופון renders.)
- [ ] Step 5: Commit.
  ```
  cd C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation
  git add src/App.tsx
  git commit -m "feat(batch): add Mic/System/Call recording source selector (Windows-gated)"
  ```

---

### Task 18: Thread the chosen source into the `start_batch_recording` invoke
**Files:** Modify `src/App.tsx` (`handleStartBatchRecord`, lines 1024-1036).

- [ ] Step 1: Write the landed-marker check:
  ```
  git grep -n 'invoke("start_batch_recording", { source: recordSource })' -- src/App.tsx
  ```
  Expected once implemented: exactly one match. Before: zero (the bare `invoke("start_batch_recording")` at line 1027).
- [ ] Step 2: Run the check to verify it is absent:
  ```
  cd C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation
  git grep -n "start_batch_recording" -- src/App.tsx
  ```
  Expected ABSENT: the only match is `await invoke("start_batch_recording");` (no `source`).
- [ ] Step 3: Write minimal implementation — replace the whole `handleStartBatchRecord` callback (lines 1024-1036); thread `recordSource` into the invoke and add it to the dependency array:
  ```tsx
  const handleStartBatchRecord = useCallback(async () => {
    setBatchError("");
    try {
      // Thread the chosen source (Mic/System/Call) into the record command.
      // Default "mic" reproduces the previous no-arg behavior exactly.
      await invoke("start_batch_recording", { source: recordSource });
      setBatchRecording(true);
      setBatchRecordElapsed(0);
      batchRecordTimerRef.current = setInterval(() => {
        setBatchRecordElapsed((e) => e + 1);
      }, 1000);
    } catch (e) {
      setBatchError(String(e));
    }
  }, [recordSource]);
  ```
  **Cross-component contract (backend, Chunk 5):** `start_batch_recording` accepts `source: Option<RecordingSource>` defaulting to `Mic`, and `RecordingSource` deserializes the lowercase wire values (`#[serde(rename_all = "lowercase")]`). Per spec §6, the Call/cloud guard ("מצב שיחה דורש מפתח Deepgram" when no Deepgram key) fires *at start* inside this command, so a keyless Call rejects here and `setBatchError` surfaces it before any recording begins.
- [ ] Step 4: Verify it passes (real safety net):
  ```
  cd C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation
  npx tsc --noEmit
  git grep -n "source: recordSource" -- src/App.tsx
  ```
  Expected: `tsc` clean; `git grep` prints one match (this task) — note Task 19 will add a second `source: recordSource` occurrence for the stop invoke. Then `npm run tauri dev`, batch view with default מיקרופון, press "🎙 הקלט ותמלל", speak, then "⏹ עצור ותמלל" — recording starts, timer runs, transcript appears as before (Mic = zero regression); DevTools console shows no "argument missing"/deserialize error.
- [ ] Step 5: Commit.
  ```
  cd C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation
  git add src/App.tsx
  git commit -m "feat(batch): thread selected source into start_batch_recording invoke"
  ```

---

### Task 19: Make `handleStopBatchRecord` source-aware — branch the Call stop path off the mono file path
**Files:** Modify `src/App.tsx` (`handleStopBatchRecord`, lines 1038-1090).

> **Why (spec §4.3 + §4.6).** Today `handleStopBatchRecord` unconditionally calls `stop_batch_recording_to_file` then `transcribe_file`. Spec §4.3 🔴 forbids Call from that path — `transcribe_file → decode_file_to_16k_mono` merges to mono and destroys channel separation, losing "הצד השני". Call is backend-only, writes no WAV, returns `(text, segments)` directly. **Mic and System both keep the existing file path** — System's routing to `system_recorder` is *backend-handled inside `stop_batch_recording_to_file`* (which now receives `source`, Seam #4), so the frontend needs no System-specific branch beyond passing `source`.

> **Seams #2/#3/#4 baked in below:** Call invokes `stop_call_recording` (not `..._and_transcribe`) **with `opts`**; the Mic/System stop passes `{ source: recordSource }`.

- [ ] Step 1: Write the landed-marker check:
  ```
  git grep -n "stop_call_recording" -- src/App.tsx
  ```
  Expected once implemented: matches on the Call invoke line. Before: zero matches.
- [ ] Step 2: Run the check to verify it is absent:
  ```
  cd C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation
  git grep -n "stop_call_recording" -- src/App.tsx
  ```
  Expected ABSENT: no output (exit 1) — Call is still (wrongly) funnelled through the mono `stop_batch_recording_to_file`/`transcribe_file` path.
- [ ] Step 3: Write minimal implementation — replace the whole `handleStopBatchRecord` callback (lines 1038-1090). The Mic/System branch is the existing behavior (now passing `source`); only Call diverges to the backend-only command that returns the same `{ text, segments }` shape `transcribe_file` returns, so downstream result-wiring is reused untouched:
  ```tsx
  const handleStopBatchRecord = useCallback(async () => {
    if (batchRecordTimerRef.current) {
      clearInterval(batchRecordTimerRef.current);
      batchRecordTimerRef.current = null;
    }
    setBatchRecording(false);

    // Call is backend-only (spec §4.3/§4.6): it never writes a WAV and must NOT go
    // through transcribe_file → decode_file_to_16k_mono, which merges to mono and
    // destroys the channel separation ("הצד השני"). It stops both recorders and
    // returns (text, segments) directly. Mic/System keep the existing file path,
    // now passing `source` so the backend drains the right recorder (System routes
    // to system_recorder inside stop_batch_recording_to_file).
    const isCall = recordSource === "call";

    let filePath = "";
    if (!isCall) {
      try {
        filePath = await invoke<string>("stop_batch_recording_to_file", { source: recordSource });
      } catch (e) {
        setBatchError(`שמירת ההקלטה נכשלה: ${String(e)}`);
        return;
      }
    }

    const now = new Date();
    const timeStr = now.toLocaleTimeString("he-IL", { hour: "2-digit", minute: "2-digit" });
    const fileName = `הקלטה ${timeStr}.wav`;
    const newId = ++batchIdCounter;
    const newItem: BatchResult = { id: newId, fileName, filePath, transcript: "", status: "pending" };

    setBatchActiveResultId(newId);
    setBatchResults((prev) => [...prev, newItem]);
    setBatchRunning(true);
    setBatchPct(0);
    setBatchStage(isCall ? "transcribing" : "decoding");
    batchCancelledRef.current = false;

    setBatchResults((prev) => prev.map((r) => r.id === newId ? { ...r, status: "processing" } : r));

    try {
      // Call → dedicated backend command (stops both recorders, interleaves,
      // multichannel Deepgram, returns tagged "אני:"/"הצד השני:" text + merged
      // segments). Mic/System → existing mono file path, unchanged.
      const { text, segments } = isCall
        ? await invoke<{ text: string; segments: TimedSegment[] }>("stop_call_recording", {
            opts: { mode: batchMode, language: "he", inject: false },
          })
        : await invoke<{ text: string; segments: TimedSegment[] }>(
            "transcribe_file",
            { filePath, opts: { mode: batchMode, language: "he", inject: false } }
          );
      setBatchResults((prev) => prev.map((r) => r.id === newId ? { ...r, status: "done", transcript: text, segments } : r));
    } catch (e) {
      const msg = String(e);
      if (msg === "בוטל" || batchCancelledRef.current) {
        setBatchResults((prev) => prev.map((r) => r.id === newId ? { ...r, status: "cancelled" } : r));
      } else {
        setBatchResults((prev) => prev.map((r) => r.id === newId ? { ...r, status: "error", error: msg } : r));
      }
    } finally {
      // Only Mic/System write a temp WAV (≈110 MB/hour); Call is in-memory so there
      // is no file to delete. Guard on filePath so the Call branch skips cleanup.
      if (filePath) {
        try { await invoke("delete_temp_recording", { path: filePath }); } catch { /* ignore */ }
      }
    }

    setBatchRunning(false);
    setBatchStage("done");
    setBatchActiveResultId(null);
  }, [batchMode, recordSource]);
  ```
  **Cross-component contract (backend, Chunk 5 Task 16):** `stop_call_recording(opts: BatchOpts)` stops *both* the mic and `system_recorder`, runs `is_effectively_silent` on the **combined** interleaved buffer (§6 — the per-file guards are bypassed for Call), builds the stereo WAV (`samples_to_wav_stereo`), calls `transcribe_deepgram_multichannel`, and wraps `(text, segments)` into the same `TranscribeFileResult` `{ text, segments }` shape — no file written. Also: `stop_batch_recording_to_file(source)` drains `system_recorder` (not the mic) when `source = "system"`, so the frontend's unchanged Mic/System branch is correct for both.
- [ ] Step 4: Verify it passes (real safety net):
  ```
  cd C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation
  npx tsc --noEmit
  git grep -n "stop_call_recording" -- src/App.tsx
  ```
  Expected: `tsc` clean; `git grep` prints one match. Then `npm run tauri dev` on Windows: (a) **Mic/System regression** — with מיקרופון (or אודיו מערכת) selected, record→stop transcribes as before via the file path. (b) **Call routing** — select שיחה, record→stop, and with DevTools open confirm the stop handler invokes `stop_call_recording` and does **not** invoke `stop_batch_recording_to_file`/`transcribe_file`. Full end-to-end "אני / הצד השני" separation is verified in the backend + the spec §7 manual real-audio pass.
- [ ] Step 5: Commit.
  ```
  cd C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation
  git add src/App.tsx
  git commit -m "feat(batch): branch stop handler so Call skips the mono transcribe_file path"
  ```

---

### Task 20: SRT export sends the per-file `SpeakerLabelStyle`

> **Added by plan review.** Without this task `SpeakerLabelStyle::Call` is production-dead:
> `export_srt` would always render `Diarization`, so a Call recording's SRT would read
> `דובר 1:/דובר 2:` instead of the `אני:/הצד השני:` that spec §4.5 mandates. The labels
> live in the top-level `text` (inject/copy/TXT/DOCX) but would be missing from the SRT surface.

**Files:**
- Modify: `src/App.tsx` (mark Call-sourced results; send `styles` on both SRT export paths)

- [ ] Step 1: Mark Call results — add `isCall?: boolean` to the `BatchResult` interface, and set `isCall: true` on the result stored by the Call stop handler (Task 19). Mic/System results leave it undefined.

- [ ] Step 2: Send the style on the per-item export — in `exportSingleSrt`, accept the result's `isCall` and send a one-element `styles` array parallel to `items`:

```ts
await invoke<string>("export_srt", {
  items: [segments],
  styles: [isCall ? "Call" : "Diarization"],
  suggested_name: firstWordsName(transcriptForName),
});
```

- [ ] Step 3: Send the styles on the combined export — in `exportBatchSrt`, build the array parallel to `items` so each file keeps its own style:

```ts
const items = eligible.map((r) => r.segments!);
const styles = eligible.map((r) => (r.isCall ? "Call" : "Diarization"));
const path = await invoke<string>("export_srt", { items, styles, suggested_name });
```

- [ ] Step 4: Verify manually — record a short Call (speak, then play audio), export its SRT, and confirm the cues read `אני: ...` / `הצד השני: ...`. Then export a plain dictation's SRT and confirm it is unchanged. Finally do a combined export of both and confirm each file is labeled by its own source.

- [ ] Step 5: Commit —
```
git -C C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation add src/App.tsx
git -C C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation commit -m "feat(srt): export Call recordings with אני/הצד השני labels

The frontend now sends a per-file SpeakerLabelStyle, so export_srt renders Call
results with side labels instead of the diarization דובר N: prefix.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Follow-ups (out of scope for this plan)

- ~~**`cancel_batch_recording` only stops the mic.**~~ ✅ **DONE (`af30355`, promoted from follow-up to a Critical fix after final review).** Cancel now unconditionally drains `state.system_recorder` too — the final integration review showed the Cancel button renders for every source, so a cancelled System/Call recording was leaving the loopback thread running AND (via the per-recorder re-entrancy guard) bricking every future System/Call start until app restart. Not a "small hardening" — it was ship-blocking.
