# Hebrew Dictation — Session Handoff (2026-06-30)

> **Next session: read this + `memory/hebrew-dictation.md` to continue.**

## ✅ 2026-06-30 — Release blockers C1 + H1–H4 FIXED (compile + tests green)
All five known issues from the 2026-06-29 adversarial review are resolved.
**Verified:** `cargo check` ✅ (0 errors) · `cargo test` ✅ (14 passed, 0 failed) · `tsc --noEmit` ✅ (exit 0).
**NOT yet runtime-verified by Henry** — needs a `npm run tauri dev` pass (see START NEXT SESSION).

- **C1** — `start_streaming_transcription` (lib.rs) now bails at the top if `batch_recording_in_progress` is set → Alt+D can no longer wipe a meeting recording.
- **H1** — `start_batch_recording` now rejects when a streaming session is live (`state.streaming.try_lock()`, kept sync; a held lock = "busy, try again").
- **H2** — `cancel_batch_recording` clears `batch_recording_in_progress` FIRST, before the (fallible) recorder lock → a poisoned lock can't permanently block short dictation.
- **H3 (root cause)** — `AudioRecorder::start_recording` (audio.rs) now returns `Err` if `is_recording()` is already true → no buffer-clear, no duplicate VAD thread. Backstop for C1/H1.
- **H4** — new hardened `delete_temp_recording` command (temp-dir + `hd-recording-*.wav` pattern only); `handleStopBatchRecord` deletes the temp WAV in a `finally` after transcription resolves.
- **Bonus (medium)** — frontend hotkey handler bails (`batchRecordingRef`) when a batch recording is active, so Alt+D is a clean no-op on the batch screen instead of a hidden main-view error.

## Where we are

### Batch UX/UI Redesign + Multi-File + Recording + Smart Export Name — COMPILES CLEAN (2026-06-29)

**Compile verified:** `cargo check` ✅ (Finished dev profile, 0 errors) + `tsc --noEmit` ✅ (exit 0) — both green 2026-06-29 incl. smart-export-name change.
**Henry runtime-verified (visually):** batch UI works, recording works. Smart export name NOT yet runtime-verified (only compiled).

#### What's in the code:
- **"batch" AppView** — dedicated full-screen view (mirrors settings pattern)
- **Multi-file**: `pick_audio_files` → frontend loop per file, per-file result cards
- **Recording** (Phase 3 MVP):
  - `start_batch_recording` — disables VAD, 3600s max, starts mic
  - `stop_batch_recording_to_file` — stops mic → writes temp WAV (16-bit PCM 16kHz) → returns path → `transcribe_file` handles it like a picked file (no IPC transfer of Vec<f32>)
  - `cancel_batch_recording` — stops mic, discards buffer
  - `batch_recording_in_progress: AtomicBool` in AppState — guard added to `start_recording` only. ⚠️ **streaming mode (Alt+D) is NOT guarded — see Known Issues C1 below.**
  - Frontend: 🎙 button in empty state + action bar; MM:SS timer; red pulsing dot; "⏹ עצור ותמלל" / "בטל"
  - Recording card appears as "הקלטה HH:MM.wav" alongside file cards
- **UI fixes** (2026-06-29):
  - "חזור" — no arrow (was "← חזור", LTR arrow wrong in RTL)
  - Main header nav: text button "תמלול קבצים" (was 📁 emoji)
- **Smart export filename** (2026-06-29):
  - `generateExportName(results)` in App.tsx — takes first 6 words of the FIRST done transcript (combined exports of multiple files also use the first file's opening words)
  - `export_history` backend now accepts `suggested_name: Option<String>`
  - `sanitize_filename(name)` in lib.rs — strips Windows-forbidden chars (`\ / : * ? " < > |` + controls), caps at 80 chars
  - Save dialog default name = sanitized transcript opening words (user can still rename)
  - Fallback: timestamp-based name when no transcript available

#### Files changed (this session):
- `src/App.tsx` — types, state, handlers, JSX (batch view, recording, export)
- `src/App.css` — full batch view + recording styles
- `src-tauri/src/lib.rs` — `pick_audio_files`, 3 recording commands, `sanitize_filename`, `export_history` updated

#### ⚠️ Known Issues — fix before v2.10.0 release (adversarial code review, 2026-06-29; each item traced to a real path, reviewer over-claims corrected)

**CRITICAL — C1: Streaming Alt+D silently destroys an in-progress meeting recording.**
`start_streaming_transcription` (lib.rs ~L629) lacks the `batch_recording_in_progress` guard that `start_recording` has. Path: batch recording active → main `status` stays `"idle"` → Alt+D → `beginRecording()` (App.tsx:535) → streaming mode → `start_streaming_transcription` → `recorder.start_recording()` → `samples.clear()` wipes the meeting audio, no visible error.
**Fix (4 lines):** add to the top of `start_streaming_transcription`:
```rust
if state.batch_recording_in_progress.load(Ordering::SeqCst) {
    return Err("הקלטת ישיבה בתהליך — עצור אותה לפני הקלטה חדשה".to_string());
}
```

**HIGH — H1:** `start_batch_recording` (lib.rs ~L515) doesn't check for an active streaming session (`state.streaming`). Symmetric to C1: starting a meeting recording while streaming is live clobbers the recorder. Fix: reject if `state.streaming` is `Some` (needs async or try_lock).

**HIGH — H2:** `cancel_batch_recording` (lib.rs ~L563) leaves `batch_recording_in_progress = true` if `recorder.lock()` is poisoned (early `?` return) → permanently blocks short dictation until app restart. The `start_batch_recording` path already clears the flag on lock error; cancel path doesn't. Fix: clear flag before propagating the error.

**HIGH — H3 (root cause of C1):** `AudioRecorder::start_recording` (audio.rs:129) doesn't stop an already-running stream. Re-entrant call clears the buffer and spawns a SECOND VAD thread (the old one never exits — `is_recording` is set `true` again). ⚠️ Reviewer claimed the old CPAL stream thread "blocks forever" — **FALSE**: the old `stream_stop_tx` Sender is dropped, so its `rx.recv()` returns `Err` and the thread self-terminates. Real residue = duplicate VAD thread + buffer clear. Fix: make `start_recording` stop-or-reject when already recording.

**HIGH — H4:** Temp WAV files (`%TEMP%\hd-recording-*.wav`, ~110 MB/hour) are never deleted after `transcribe_file`. Fix: delete in `handleStopBatchRecord` finally-block after transcription resolves.

**MEDIUM/LOW (lower priority, all verified):**
- Frontend Alt+D doesn't check `batchRecording`: in non-streaming the Rust guard's error lands in the *main-view* error state, invisible on the batch screen (recording itself stays protected). Defense-in-depth: bail in the hotkey handler if a batch recording is active.
- `batchRecordTimerRef` not cleared on unmount / when navigating away mid-recording → timer keeps counting in the background.
- `stop_batch_recording_to_file` is `async` but does a blocking ~110 MB WAV write while holding the sync recorder Mutex → stalls a Tokio worker thread. Fix: `spawn_blocking` or make the command sync.
- `restore_recorder_settings` silently no-ops on lock poison → VAD stays off for the rest of the session. Use `unwrap_or_else(|e| e.into_inner())` (the pattern used elsewhere in lib.rs).

**Checked & clean:** WAV header byte layout, i16 clamp before ×32767, no deadlock between `start_batch_recording`/`stop_batch_recording_to_file`, `batch_in_progress` vs `batch_recording_in_progress` are orthogonal, silence check runs before the WAV write.

#### START NEXT SESSION WITH:
1. ~~Fix **C1** + H1–H4~~ — ✅ DONE 2026-06-30 (compile + 14 tests green). See the "Release blockers FIXED" box at the top.
2. **Runtime-verify in `npm run tauri dev`** (Henry):
   - Start a batch recording → press **Alt+D** → confirm it's a clean no-op (no error, meeting recording survives) [C1 + bail].
   - Stop the batch recording → after transcription, confirm `%TEMP%\hd-recording-*.wav` is gone [H4].
   - Export TXT/DOCX → confirm dialog default name = first words of the transcript (not generic `hebrew-dictation-history_...`).
3. Then **Phase 2 (batch)** + release v2.10.0 (compile already green — no need to re-run cargo check / tsc unless code changed).

### Batch Transcription Phase 1 — BUILT + RUNTIME-VERIFIED (by Henry, in `tauri dev`)
File upload → transcribe (cloud Deepgram **or** local whisper, offline) → editable RTL textarea → export TXT/DOCX / inject / copy, with cancel. Additive — short dictation untouched.

**Henry's smoke results:**
- ✅ Cloud (Deepgram) — works.
- ✅ Local (whisper, offline) — works **after the -6 fix** (see below).
- ✅ iPhone **.m4a** — works.
- ✅ Cancel — works.
- ✅ Regression: short dictation responsive during a local batch.
- ✅ Corrupt/empty file — Henry deprioritized the manual check (couldn't find a sample); covered by the `decode::tests::corrupt_file_errors` unit test + the error is already Hebrew (`"פורמט אודיו לא נתמך או קובץ פגום"`).

**All committed locally** (`174ae59`…`7dafc37`, ~11 commits). `cargo build` clean (0 warnings); `cargo test` 14 green; frontend `npm run build` clean. NOT pushed.

### Two bugs/asks fixed this session
1. **Local -6 ("failed to encode")** — root cause: **whisper-rs 0.16.0 `set_abort_callback_safe` is BUGGY** (its trampoline is parameterized by the closure type `F` while `user_data` points to a `Box<dyn FnMut()->bool>` — the progress wrapper gets this right, abort doesn't → reads garbage → returns spurious `true` → whisper aborts encode → -6). **Fix:** bypass the safe wrapper; use raw `set_abort_callback` + a module-static `LOCAL_ABORT: AtomicBool` (single in-flight batch, so no user_data box needed). `cancel_batch` calls `whisper::request_local_abort()`. (whisper.rs)
2. **Hebrew error messages** (Henry: users must understand + know what to do) — added `whisper::whisper_error_to_he()` (GenericError/-6 → "(1) סגור תוכנות לפנות זיכרון, (2) מודל קטן יותר, (3) מחק+הורד מחדש"); translated model download/save/hash + `load_whisper_model` "model not found". **All user-facing local/download/decode errors are now actionable Hebrew.**

---

## Next steps (in order)
1. ✅ **DONE 2026-06-30 — Known Issues C1 (CRITICAL) + H1–H4 fixed** (compile + 14 tests green). Details in the "Release blockers FIXED" box at the top.
2. **Runtime-verify in `npm run tauri dev`** (Henry): batch-record → Alt+D no-op + meeting survives [C1]; temp WAV deleted after transcription [H4]; export dialog default name = first words of transcript.
3. **Phase 2 (batch):** Groq cloud + chunking (`chunk.rs`, 5-10min windows + de-dupe), `save_transcript_next_to`, accuracy toggle.
4. **Release v2.10.0** — after runtime-verify. Bump version in 4 places (package.json, Cargo.toml, tauri.conf.json, **APP_VERSION src/App.tsx:10**). Signed installer, GitHub release, website.

## Remaining MEDIUM/LOW (deferred — non-blocking, from 2026-06-29 review)
- `batchRecordTimerRef` not cleared on unmount → timer keeps counting if navigated away mid-recording.
- `stop_batch_recording_to_file` does a blocking ~110 MB WAV write while holding the sync recorder Mutex on a Tokio worker → `spawn_blocking` or make the command sync.
- `restore_recorder_settings` silently no-ops on lock poison → use `unwrap_or_else(|e| e.into_inner())` (pattern already used elsewhere in lib.rs).
- Remaining English error strings in deep-internal paths (see "Deferred / low-priority" below).

## Future feature ideas (Henry — not scheduled)
- **English → Hebrew transcription:** upload/dictate in English and have it auto-transcribe into Hebrew (i.e. transcribe + translate). Requested 2026-06-30 during the UI pass; explicitly deferred — do NOT build now.

## Deferred / low-priority
- Remaining **English** error strings in deep-internal paths users rarely hit (audio.rs stream init L145-285, settings.rs serialize L427-432, streaming.rs WS L46-96, injector.rs). Translate opportunistically; not urgent.
- Backend guard error strings still say **"הקלטת ישיבה"** (lib.rs C1/H1) — UI now calls this action **"הקלט ותמלל"**; align the Rust strings on the next Rust rebuild.
- `BatchError` enum still deliberately deferred (Hebrew String errors + `"בוטל"` sentinel work fine).

## Still pending from earlier sessions (unchanged)
- **v2.9.1→2.9.3 fixes LOCAL-ONLY, NOT pushed.** `origin/main` = v2.9.0. Critical **keyring** fix (all users lose API keys on restart) sits in these unpushed commits. Decision: bundle → release as **v2.10.0**.

## Key facts
- Signing: `~/.tauri/hebrew-dictation.key` (no password); `export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/hebrew-dictation.key)"` before `npm run tauri build`.
- Repo: `aihenryai/hebrew-dictation`. Website: `aihenryai/Henry-AI-website` (Cloudflare auto-deploy on push to main).
- whisper-rs 0.16 abort-callback bug is real — if upgrading whisper-rs later, re-check the LOCAL_ABORT workaround.
- Decode = symphonia 0.6 + rubato 3 (post-rewrite APIs): `GenericAudioBufferRef::copy_to_vec_interleaved` + rubato `process_all_into_buffer`.
- Dev run for testing: `npm run tauri dev` (repo root). Kill orphans: PowerShell `Get-CimInstance Win32_Process | ? CommandLine -match 'hebrew-dictation' | % { Stop-Process -Id $_.ProcessId -Force }`.
