# Recording Modes UX (4th mode + source-selector regroup) Implementation Plan

> **Agentic workers are REQUIRED to execute this plan with `superpowers:subagent-driven-development`** (independent tasks in the current session) **or `superpowers:executing-plans`** (separate session with review checkpoints). Do not free-hand the work. Every Rust task follows strict red→green TDD: write the failing test, run it to see it fail for the right reason, write the minimal implementation, run it green, commit. Frontend tasks have no Rust/JS test runner — their safety net is `npx tsc --noEmit` + a manual `npm run tauri dev` preview + a `git grep` landed-marker gate. Each step is a `- [ ]` checkbox — check it off only when done. Work the chunks in order; within a chunk, work the tasks in order.

**Goal:** Add a 4th recording mode — **"פגישה — פרטית במכשיר" (`CallLocal`)**: mic + system audio mixed into one mono transcript, transcribed **locally** (whisper), no speaker separation — and reorganize the batch source selector into two clear groups ("הקלטה רגילה" / "פגישות") with a context-dependent cloud/local selector, per Henry's explicit clarity priority.

**Architecture:** `CallLocal` reuses the existing **mono file path** (never the `CallCloud` inline stereo/multichannel path): it drives both recorders (like `CallCloud`), but at stop it **mixes** the two 16 kHz mono buffers to one mono buffer (`mix_to_mono`, averaging with silence-padding), writes the existing 16 kHz mono WAV, and the frontend transcribes it with **forced local** mode. A pre-record guard requires a downloaded whisper model (symmetric to `CallCloud`'s Deepgram-key guard). The `RecordingSource` enum is renamed `Call → CallCloud` and gains `CallLocal`. The frontend regroups the selector and renders the cloud/local cards only for the "regular" group.

**Tech Stack:** cpal (mic), wasapi 0.23 (Windows loopback, reused unchanged), whisper.cpp (existing local engine), Tauri v2 commands + `AppState`, React/TS frontend. **No new crate dependencies.**

**Spec:** `docs/superpowers/specs/2026-07-12-recording-modes-ux.md` (all 4 design questions closed with Henry; reviewer-approved with anchors verified).

---

## File Structure

| File | Created/Modified | Single responsibility for this feature |
|---|---|---|
| `src-tauri/src/audio.rs` | Modified | Add pure `mix_to_mono(mic, system) -> Vec<f32>` (average + silence-pad). `interleave_stereo` + mic path unchanged. |
| `src-tauri/src/batch.rs` | Modified | Rename `RecordingSource::Call → CallCloud`; add `CallLocal`; extend `recorders_for_source`; add pure `ensure_local_meeting_model_available` guard. |
| `src-tauri/src/lib.rs` | Modified | Rename all `Call` sites → `CallCloud`; add `CallLocal` model guard in `start_batch_recording`; add `CallLocal` drain-both+`mix_to_mono` arm in `stop_recorder_for_source`; narrow the `stop_batch_recording_to_file` rejection to `CallCloud` only; extend the non-Windows match arm. |
| `src/App.tsx` | Modified | Rename `"call" → "callcloud"` (+ `isCall → isCallCloud`); add `"calllocal"`; regroup the source selector into two labeled groups with two meeting cards; render the cloud/local cards only for `mic`/`system`; force `mode:"local"` for `calllocal`; delete the standalone transparency note. |

**No changes to:** `model.rs`, `settings.rs` (read-only use of `is_model_downloaded` / `preferred_model`), `system_audio.rs`, `api_transcribe.rs`, `srt.rs`, `Cargo.toml`, `stop_call_recording` logic (rename only).

---

## Cross-Task Compile Notes (read before executing)

- **`cargo test <filter>` compiles the WHOLE crate.** Adding the `CallLocal` variant (Task 4) makes the `match source` in `stop_recorder_for_source` and the non-Windows arm non-exhaustive, so the variant-add and its `lib.rs` arms must land in the **same task/commit** to keep the crate green. Task 4 is therefore end-to-end (batch.rs variant + all lib.rs arms) — this is the irreducible atomic increment for adding an enum variant to a crate with exhaustive matches.
- **The `Call → CallCloud` rename (Task 2) is full-stack in ONE commit** (batch.rs + lib.rs + App.tsx) so no interim commit leaves the app's call mode wired to a stale string. It is a pure mechanical rename — zero behavior change.
- **Tests run with:** `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml <filter>` (Windows host).
- **Frontend gate:** `npx tsc --noEmit` (from repo root) must be clean; then a `git grep` presence check for the landed markers; then manual `npm run tauri dev`. Rust changes need the dev process **restarted** (HMR covers only the frontend).
- **Commit style:** direct to `main`, no PR (`feedback_direct_commits`). Scope each commit to only the files in that task.

---

## Chunk 1: Pure `mix_to_mono` helper

Self-contained pure function the `CallLocal` stop path is built on. Unit-tested end-to-end with no audio hardware. Nothing depends on it yet, so it goes green immediately (an unused-until-Chunk-3 helper emits at most a harmless `dead_code` warning on plain `cargo build`; its own test references it so the warning is silenced under `cargo test`).

### Task 1: `mix_to_mono` pure helper in `audio.rs`

**Files:**
- Modify: `src-tauri/src/audio.rs` — add `mix_to_mono` immediately **after** `interleave_stereo` (~line 649-650, i.e. before the `#[cfg(test)] mod interleave_stereo_tests` block at ~651); append a new `#[cfg(test)] mod mix_to_mono_tests` as the file's **final** item (two consecutive test mods at EOF is fine — `items_after_test_module` only fires on a *non-test* item after a test mod).

- [ ] **Step 1: Write the failing test** — append as the file's last item:

```rust
#[cfg(test)]
mod mix_to_mono_tests {
    use super::*;

    #[test]
    fn equal_lengths_average_sample_wise() {
        // (m + s) * 0.5 per sample. Values chosen exact in f32.
        let mic = [0.5f32, 0.25];
        let system = [0.5f32, -0.25];
        assert_eq!(mix_to_mono(&mic, &system), vec![0.5f32, 0.0]);
    }

    #[test]
    fn mic_longer_pads_system_with_silence() {
        // system missing samples count as 0.0 → (m + 0) * 0.5.
        let mic = [0.5f32, 0.5];
        let system = [0.5f32];
        assert_eq!(mix_to_mono(&mic, &system), vec![0.5f32, 0.25]);
    }

    #[test]
    fn system_longer_pads_mic_with_silence() {
        let mic = [0.5f32];
        let system = [0.5f32, 1.0];
        assert_eq!(mix_to_mono(&mic, &system), vec![0.5f32, 0.5]);
    }

    #[test]
    fn empty_and_one_sided() {
        assert!(mix_to_mono(&[], &[]).is_empty());
        // One side empty → the other is halved (0 + 0.5) * 0.5.
        assert_eq!(mix_to_mono(&[], &[0.5f32]), vec![0.25f32]);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml mix_to_mono`
Expected: FAIL to compile — `cannot find function mix_to_mono in this scope`.

- [ ] **Step 3: Write the minimal implementation** — insert right after `interleave_stereo`:

```rust
/// Mix two 16 kHz mono buffers into one by averaging sample-wise, padding the
/// shorter side with silence (`0.0`). Output length = `max(mic.len, system.len)`.
/// Averaging (×0.5) — not summation — avoids clipping when both sides speak at once;
/// the cost is that a one-sided moment plays at half amplitude (whisper tolerates
/// level). Used by the `CallLocal` ("פגישה מקומית") stop path. Pairs with
/// `interleave_stereo` (which keeps the channels separate for `CallCloud`).
pub fn mix_to_mono(mic: &[f32], system: &[f32]) -> Vec<f32> {
    let n = mic.len().max(system.len());
    (0..n)
        .map(|i| {
            let m = mic.get(i).copied().unwrap_or(0.0);
            let s = system.get(i).copied().unwrap_or(0.0);
            (m + s) * 0.5
        })
        .collect()
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml mix_to_mono`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/audio.rs
git commit -m "feat(audio): add pure mix_to_mono for the local-meeting mode"
```

---

## Chunk 2: Full-stack `Call → CallCloud` rename

One atomic, behavior-preserving rename across Rust + TS. The "test" is: the whole crate still compiles, all existing tests still pass with the new name, `tsc --noEmit` is clean, and no `Call`/`"call"` source-token remains.

### Task 2: Rename `RecordingSource::Call → CallCloud` everywhere

**Files:**
- Modify: `src-tauri/src/batch.rs` — enum variant + doc + `recorders_for_source` arm + the 2 tests referencing `RecordingSource::Call` (`batch.rs:101` and `:123`) + the `"\"call\""` deserialize string (`batch.rs:101`).
- Modify: `src-tauri/src/lib.rs` — every `batch::RecordingSource::Call` site: `start_batch_recording` (~594), `stop_batch_recording_to_file` (~676), `stop_recorder_for_source` `unreachable!` arm (~725) + non-Windows arm (~727).
- Modify: `src/App.tsx` — `RecordingSource` type member `"call"→"callcloud"` (~85); `recordSource === "call"` (~1074); the "שיחה" card's `setRecordSource("call")`/`=== "call"` (~2459-2467); rename the `isCall` local + `BatchResult.isCall` field → `isCallCloud` (~73, ~1074-1113, ~1193-1216, ~2632).

- [ ] **Step 1: Update the existing Rust tests to the new name** (red) — in `batch.rs`, change the two `RecordingSource::Call` occurrences (one in `recording_source_deserializes_and_defaults_to_mic`, one in `recorders_for_source_maps_each_variant`) to `RecordingSource::CallCloud`, and the deserialize string `"\"call\""` to `"\"callcloud\""`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml recording_source`
Expected: FAIL to compile — `no variant named Call... did you mean CallCloud?` is not yet true; error is `no variant or associated item named CallCloud`.

- [ ] **Step 3: Rename in `batch.rs`** — variant `Call → CallCloud` (keep `#[serde(rename_all = "lowercase")]` → serde token becomes `"callcloud"`); update the variant doc comment; `recorders_for_source` arm `RecordingSource::Call => (true, true)` → `RecordingSource::CallCloud => (true, true)`. Leave `ensure_call_deepgram_available` name as-is (still "a call needs Deepgram").

- [ ] **Step 4: Rename all `lib.rs` sites** — replace every `batch::RecordingSource::Call` with `batch::RecordingSource::CallCloud`:
  - `start_batch_recording`: `if matches!(source, batch::RecordingSource::CallCloud) { ...deepgram guard... }`
  - `stop_batch_recording_to_file`: `if matches!(source, batch::RecordingSource::CallCloud) { return Err("מצב שיחה נעצר דרך stop_call_recording…") }`
  - `stop_recorder_for_source`: `#[cfg(windows)] batch::RecordingSource::CallCloud => unreachable!("CallCloud is handled before this call")`
  - non-Windows arm: `batch::RecordingSource::System | batch::RecordingSource::CallCloud => Err(...)`

- [ ] **Step 5: Rename in `App.tsx`**
  - Type: `type RecordingSource = "mic" | "system" | "callcloud";`
  - `const isCallCloud = recordSource === "callcloud";` (was `isCall`/`"call"`); update all downstream `isCall` → `isCallCloud` (the `!isCall` file branch, `newItem`, `setBatchStage`, the inline-Call branch, the finally guard).
  - `BatchResult.isCall?: boolean` → `isCallCloud?: boolean`; SRT export (`exportSingleSrt` param + `r.isCall` → `r.isCallCloud`, and the `styles` maps at ~1200/1214).
  - The existing "שיחה" card: `setRecordSource("callcloud")` + `recordSource === "callcloud"` (this card is replaced in Chunk 4; renaming keeps it working meanwhile).

- [ ] **Step 6: Verify Rust green + TS clean**

Run: `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml`
Expected: PASS, 0 failures (baseline 50 + the 4 `mix_to_mono` tests from Task 1 = 54 passed, 1 ignored; the rename adds no test).
Run (repo root): `npx tsc --noEmit`
Expected: no errors.
Run: `git grep -nE "RecordingSource::Call\b|\"call\"|isCall\b" -- src-tauri/src src/App.tsx`
Expected: **no matches** (only `CallCloud` / `"callcloud"` / `isCallCloud` remain).

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/batch.rs src-tauri/src/lib.rs src/App.tsx
git commit -m "refactor: rename RecordingSource::Call -> CallCloud (no behavior change)"
```

---

## Chunk 3: `CallLocal` backend

Adds the new mode's backend end-to-end. Task 3 is a pure, independently-green guard. Task 4 adds the variant + all forced match arms + the model guard + the mixing stop path in one atomic, crate-compiling commit.

### Task 3: `ensure_local_meeting_model_available` guard in `batch.rs`

**Files:**
- Modify: `src-tauri/src/batch.rs` — add the pure fn after `ensure_call_deepgram_available` (~line 69) and a test in the existing `mod tests`.

- [ ] **Step 1: Write the failing test** — add inside `batch.rs` `mod tests`:

```rust
#[test]
fn local_meeting_requires_a_downloaded_model() {
    // Model present → proceed.
    assert!(ensure_local_meeting_model_available(true).is_ok());
    // No local model → a guiding Hebrew error, raised BEFORE recording starts.
    let err = ensure_local_meeting_model_available(false).unwrap_err();
    assert!(err.contains("מודל מקומי"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml local_meeting_requires`
Expected: FAIL to compile — `cannot find function ensure_local_meeting_model_available`.

- [ ] **Step 3: Implement** — add after `ensure_call_deepgram_available`:

```rust
/// `CallLocal` ("פגישה מקומית") transcribes with the local whisper engine, so a
/// model must be downloaded. Fail fast — BEFORE recording — with a guiding message,
/// mirroring `ensure_call_deepgram_available`. NOTE: this checks the model *file* on
/// disk; the engine must also be loaded in memory (checked later at transcribe time),
/// so a narrow "downloaded but not loaded" window remains — the same pre-existing gap
/// as the Mic + local batch path, accepted for v1 (spec §4.6).
pub fn ensure_local_meeting_model_available(has_local_model: bool) -> Result<(), String> {
    if has_local_model {
        Ok(())
    } else {
        Err("פגישה מקומית דורשת מודל מקומי מורד. הורד אותו בהגדרות.".to_string())
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml local_meeting_requires`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/batch.rs
git commit -m "feat(batch): add ensure_local_meeting_model_available guard"
```

### Task 4: Add `CallLocal` variant + backend wiring (atomic, crate-compiling)

**Files:**
- Modify: `src-tauri/src/batch.rs` — add `CallLocal` variant + `recorders_for_source` arm + deserialize/routing tests.
- Modify: `src-tauri/src/lib.rs` — `start_batch_recording` model guard; `stop_batch_recording_to_file` (its **rejection** already narrowed to `CallCloud` in Task 2 — but its silence-message match at `lib.rs:691` DOES gain a `CallLocal` case, Step 4); `stop_recorder_for_source` new Windows arm (drain both + `mix_to_mono`) + extend non-Windows arm.

- [ ] **Step 1: Write the failing tests** — in `batch.rs`:
  - extend `recorders_for_source_maps_each_variant` with `assert_eq!(recorders_for_source(RecordingSource::CallLocal), (true, true));`
  - extend `recording_source_deserializes_and_defaults_to_mic` with `assert_eq!(from_str::<RecordingSource>("\"calllocal\"").unwrap(), RecordingSource::CallLocal);`

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml recorders_for_source`
Expected: FAIL to compile — `no variant or associated item named CallLocal`. (Adding the variant next will also surface non-exhaustive `match` errors in `lib.rs` — that is the signal for Step 4.)

- [ ] **Step 3: Add the variant + routing in `batch.rs`:**

```rust
// in enum RecordingSource, after CallCloud:
/// Mic + system captured together, MIXED to one mono buffer and transcribed
/// LOCALLY (whisper). No speaker separation. Privacy: audio never leaves the machine.
CallLocal,
```
```rust
// in recorders_for_source, add:
RecordingSource::CallLocal => (true, true),
```

- [ ] **Step 4: Add the `lib.rs` arms** (the crate will not compile until all land):
  - **Model guard** in `start_batch_recording`, right after the existing `CallCloud` Deepgram guard block, before `state.batch_recording_in_progress.swap(...)`:

```rust
// CallLocal transcribes with the local whisper engine — fail BEFORE recording if
// no model is downloaded, so the user isn't left with an un-transcribable capture.
if matches!(source, batch::RecordingSource::CallLocal) {
    let has_model = {
        let s = state.settings.lock().map_err(|e| e.to_string())?;
        model::is_model_downloaded(&s.preferred_model)
    };
    batch::ensure_local_meeting_model_available(has_model)?;
}
```

  - **Windows mixing arm** in `stop_recorder_for_source`, after the `System` arm:

```rust
#[cfg(target_os = "windows")]
batch::RecordingSource::CallLocal => {
    // Drain BOTH recorders and mix to mono. Compute both results BEFORE propagating
    // an error so a failing mic stop never skips joining the system (WASAPI) thread
    // (mirrors run_stop_call_recording, lib.rs:772-783). The mixed mono buffer then
    // flows through the caller's existing silence-guard + write_wav_16k_mono path.
    let mic_result = state
        .recorder
        .lock()
        .map_err(|e| e.to_string())
        .and_then(|mut r| r.stop_recording());
    let system_result = state
        .system_recorder
        .lock()
        .map_err(|e| e.to_string())
        .and_then(|mut s| s.stop_recording());
    let mic = mic_result?;
    let system = system_result?;
    Ok(audio::mix_to_mono(&mic, &system))
}
```

  - **Non-Windows arm** in `stop_recorder_for_source`: extend to
    `batch::RecordingSource::System | batch::RecordingSource::CallCloud | batch::RecordingSource::CallLocal => Err("לכידת אודיו-מערכת נתמכת רק ב-Windows".to_string())`
  - **`stop_batch_recording_to_file` silence message** (`lib.rs:691` `match source`): add a `CallLocal` arm with a **meeting-specific** message that names both sides (a meeting captures mic AND system): `batch::RecordingSource::CallLocal => "לא נקלט אודיו בפגישה — ודאו שהמיקרופון פעיל ושמתנגן קול במחשב."` (the `System` arm text is loopback-only; don't reuse it verbatim for a meeting).
  - Confirm `stop_batch_recording_to_file`'s early rejection is `matches!(source, batch::RecordingSource::CallCloud)` **only** (from Task 2) — `CallLocal` must fall through to `stop_recorder_for_source`.

- [ ] **Step 5: Run the full suite green**

Run: `cargo test --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml`
Expected: PASS, 0 failures (55 passed, 1 ignored — 54 from after Task 1's rename + Task 3's 1 new test fn; the two `CallLocal` asserts extend existing tests, adding no new fn). Exact totals matter less than: all green, 0 failures.
Run: `cargo build --manifest-path C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation\src-tauri\Cargo.toml`
Expected: 0 warnings (no `dead_code` — `mix_to_mono` is now used).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/batch.rs src-tauri/src/lib.rs
git commit -m "feat: add CallLocal recording mode (mic+system -> mono, local whisper)"
```

---

## Chunk 4: Frontend — 4th mode card + source-selector regroup

Regroups the selector into two labeled groups, adds the two meeting cards, makes the cloud/local selector context-dependent, forces local for `calllocal`, and deletes the now-redundant transparency note. No Rust/JS test runner — gates are `tsc --noEmit`, a `git grep` landed-marker, and manual preview.

### Task 5: Regroup the source selector into two labeled groups (+ `calllocal` type)

**Files:**
- Modify: `src/App.tsx` — `RecordingSource` type (~85); the source selector block (~2429-2471); the standalone note (~2476-2483, deleted here).

- [ ] **Step 1: Add `calllocal` to the type**

```ts
type RecordingSource = "mic" | "system" | "callcloud" | "calllocal";
```

- [ ] **Step 2: Replace the source-selector block** (the `{!batchRecording && ( <div className="batch-mode-cards" role="group" aria-label="מקור הקלטה"> ... </div> )}` at ~2434-2471) with two labeled groups. Group headers use **inline styles** (no new CSS). "אודיו מערכת" + the whole "פגישות" group are `IS_WINDOWS`-gated:

```tsx
{!batchRecording && (
  <>
    <div style={{ fontSize: "0.8rem", opacity: 0.7, margin: "8px 0 4px", textAlign: "center" }}>
      הקלטה רגילה
    </div>
    <div className="batch-mode-cards" role="group" aria-label="הקלטה רגילה">
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
      )}
    </div>

    {IS_WINDOWS && (
      <>
        <div style={{ fontSize: "0.8rem", opacity: 0.7, margin: "8px 0 4px", textAlign: "center" }}>
          פגישות
        </div>
        <div className="batch-mode-cards" role="group" aria-label="פגישות">
          <button
            className={`batch-mode-card ${recordSource === "callcloud" ? "active" : ""}`}
            onClick={() => !batchRunning && setRecordSource("callcloud")}
            disabled={batchRunning}
            aria-pressed={recordSource === "callcloud"}
          >
            <span className="batch-mode-icon" aria-hidden="true">📞</span>
            <span className="batch-mode-name">פגישה — עם זיהוי דוברים</span>
            <span className="batch-mode-desc">אתם + הצד השני, כל אחד בנפרד · מתומלל בענן</span>
          </button>
          <button
            className={`batch-mode-card ${recordSource === "calllocal" ? "active" : ""}`}
            onClick={() => !batchRunning && setRecordSource("calllocal")}
            disabled={batchRunning}
            aria-pressed={recordSource === "calllocal"}
          >
            <span className="batch-mode-icon" aria-hidden="true">🔒</span>
            <span className="batch-mode-name">פגישה — פרטית במכשיר</span>
            <span className="batch-mode-desc">אתם + הצד השני יחד · נשאר במחשב, בלי הפרדת דוברים</span>
          </button>
        </div>
      </>
    )}
  </>
)}
```

- [ ] **Step 3: Delete the standalone transparency note** (the `{!batchRecording && recordSource === "call" && ( <p role="note">🔒 שיחה מתומללת תמיד בענן…</p> )}` block, ~2476-2483) — the info now lives in the "עם זיהוי דוברים" card's desc line.

- [ ] **Step 4: Gate + landed-marker + preview**

Run (repo root): `npx tsc --noEmit`
Expected: no errors.
Run: `git grep -n "פרטית במכשיר" -- src/App.tsx`
Expected: one match (the new card).
Run: `git grep -n "שיחה מתומללת תמיד בענן" -- src/App.tsx`
Expected: **no matches** (note deleted).
Manual (Windows): `npm run tauri dev` → batch view shows two labeled groups; four cards on Windows (mic, system, two meetings); off-Windows only "מיקרופון" under "הקלטה רגילה".

- [ ] **Step 5: Commit**

```bash
git add src/App.tsx
git commit -m "feat(ui): regroup batch source selector into two groups + local-meeting card"
```

### Task 6: Context-dependent cloud/local + forced-local `calllocal` stop path

**Files:**
- Modify: `src/App.tsx` — the transcription-mode `batch-mode-cards` block (~2406-2427: make contextual + move below the source groups); `handleStopBatchRecord` (~1061-1132: `calllocal` file path + forced local mode).

> **Interim note (Task 5 → Task 6):** after Task 5's commit, `calllocal` is selectable and backend-supported (Task 4) but not yet forced-local, so it would transiently honor `batchMode` (possibly "cloud" → would hit the `CallLocal` backend which has no cloud path and error). Harmless within one session — the feature isn't released until Task 6 + manual verification — but do not ship mid-chunk; land Task 6 before any push.

- [ ] **Step 1: Make the cloud/local cards context-dependent** — wrap the existing transcription-mode `<div className="batch-mode-cards" role="group" aria-label="מצב תמלול"> ... </div>` (☁ מהיר—ענן / 🔒 פרטי—מכשיר) in a guard so it renders only for the regular group, and position it **after** the source groups (per spec §3.3 — source is the primary choice):

```tsx
{(recordSource === "mic" || recordSource === "system") && (
  <div className="batch-mode-cards" role="group" aria-label="מצב תמלול">
    {/* …existing ☁ מהיר—ענן / 🔒 פרטי—מכשיר cards, unchanged… */}
  </div>
)}
```
(For `callcloud`/`calllocal` the engine is fixed by the card, so no cloud/local selector shows.)

- [ ] **Step 2: Force local mode for `calllocal` in `handleStopBatchRecord`** — `calllocal` goes through the **file** path (`!isCallCloud`), but must transcribe with `mode:"local"` regardless of `batchMode` (which is hidden for meetings). Compute an effective mode and pass it to the `transcribe_file` invoke:

```ts
// calllocal is forced local (its card encodes the engine); mic/system honor batchMode.
const fileMode = recordSource === "calllocal" ? "local" : batchMode;
```
Then in the `transcribe_file` branch, send `opts: { mode: fileMode, language: "he", inject: false }` (instead of `mode: batchMode`). Leave the `stop_call_recording` (callcloud) branch untouched.

- [ ] **Step 3: Confirm `isCallCloud` stays callcloud-only** — `const isCallCloud = recordSource === "callcloud";` means `calllocal` takes the `!isCallCloud` file branch and its `BatchResult.isCallCloud` is `false` → SRT export uses `"Diarization"` (mono, no "אני/הצד השני" labels). No change needed beyond verifying.

- [ ] **Step 4: Gate + landed-marker + preview**

Run (repo root): `npx tsc --noEmit`
Expected: no errors.
Run: `git grep -n 'recordSource === "calllocal" ? "local"' -- src/App.tsx`
Expected: one match (forced-local wiring).
Manual (Windows, `npm run tauri dev`):
  - Select **מיקרופון** or **אודיו מערכת** → the cloud/local cards appear below.
  - Select either **פגישה** card → the cloud/local cards **disappear**.
  - With a whisper model downloaded, record **פגישה — פרטית במכשיר** while system audio plays, stop → one mono local transcript (no "אני/הצד השני").
  - With **no** model → the guard error fires *before* recording.
  - Cancel mid-**פרטית במכשיר**, then start a meeting again → works (system recorder drained; regression gate for the af30355 risk).

- [ ] **Step 5: Commit**

```bash
git add src/App.tsx
git commit -m "feat(ui): context-dependent cloud/local selector + forced-local local meeting"
```

---

## Definition of Done

- `cargo test <manifest>` green (baseline + new asserts), `cargo build` 0 warnings, `npx tsc --noEmit` clean.
- On Windows: four source cards in two groups; cloud/local cards show only for mic/system; `פרטית במכשיר` produces a local mono transcript; the no-model guard fires before recording; cancel-then-restart of a meeting works.
- Off Windows: only `מיקרופון` shows, cloud/local cards present, zero regression.
- **Manual-verify (Henry, real audio — cannot be automated):** the five preview checks in Task 6 Step 4. Do NOT mark the feature released until these pass.
- Direct commits to `main`, no PR. Push only after Henry approves (per the shipped-feature precedent).

## Out of Scope (v1) — do not build

Speaker separation for the local mode, cloud option for `calllocal`, a separate language selector for meetings, surfacing speakers in the UI transcript view, AGC/normalize before mixing. All listed in spec §2.
