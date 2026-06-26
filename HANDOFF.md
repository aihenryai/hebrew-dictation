# Hebrew Dictation — Session Handoff (2026-06-26)

> **Next session: read this + `memory/hebrew-dictation.md` to continue.**

## Where we are

### Batch Transcription Phase 1 — BUILT + RUNTIME-VERIFIED (by Henry, in `tauri dev`)
File upload → transcribe (cloud Deepgram **or** local whisper, offline) → editable RTL textarea → export TXT/DOCX / inject / copy, with cancel. Additive — short dictation untouched.

**Henry's smoke results:**
- ✅ Cloud (Deepgram) — works.
- ✅ Local (whisper, offline) — works **after the -6 fix** (see below).
- ✅ iPhone **.m4a** — works.
- ✅ Cancel — works.
- ✅ Regression: short dictation responsive during a local batch.
- ⏳ Corrupt/empty file — Henry still to test; error is already Hebrew (`"פורמט אודיו לא נתמך או קובץ פגום"`).

**All committed locally** (`174ae59`…`7dafc37`, ~11 commits). `cargo build` clean (0 warnings); `cargo test` 14 green; frontend `npm run build` clean. NOT pushed.

### Two bugs/asks fixed this session
1. **Local -6 ("failed to encode")** — root cause: **whisper-rs 0.16.0 `set_abort_callback_safe` is BUGGY** (its trampoline is parameterized by the closure type `F` while `user_data` points to a `Box<dyn FnMut()->bool>` — the progress wrapper gets this right, abort doesn't → reads garbage → returns spurious `true` → whisper aborts encode → -6). **Fix:** bypass the safe wrapper; use raw `set_abort_callback` + a module-static `LOCAL_ABORT: AtomicBool` (single in-flight batch, so no user_data box needed). `cancel_batch` calls `whisper::request_local_abort()`. (whisper.rs)
2. **Hebrew error messages** (Henry: users must understand + know what to do) — added `whisper::whisper_error_to_he()` (GenericError/-6 → "(1) סגור תוכנות לפנות זיכרון, (2) מודל קטן יותר, (3) מחק+הורד מחדש"); translated model download/save/hash + `load_whisper_model` "model not found". **All user-facing local/download/decode errors are now actionable Hebrew.**

## Next steps (in order)
1. **DEDICATED UX/UI session (Henry CONFIRMED — "פצצה ברמת על חלל").** The current batch panel is functional but rough: short/technical labels ("📄 TXT"/"⌨️ הדבק"), no empty-state, no cloud-vs-local guidance, crowds the main view. This session = full redesign (likely move batch to its own tab/view, list-style results, premium accessible UX). **FOLD MULTI-FILE INTO THIS SESSION** — Henry wants multi-file upload; it requires a list-view state refactor (`batchTranscript: string` → `results: BatchResult[]`) that the redesign does anyway. Backend for multi-file is ~100 lines (`pick_files` multi-select + serial loop + per-file progress `{stage,pct,fileIndex,fileTotal,fileName}` + per-file Result + cancel-skips-rest). Don't build multi-file on the current panel — it'd be thrown away.
2. **Phase 2 (batch):** Groq cloud + chunking (`chunk.rs`, 5-10min windows + de-dupe), `save_transcript_next_to` (header-less export + overwrite policy), accuracy toggle.
3. **Phase 3:** long in-app meeting recording (RAM MVP) → same orchestrator.
4. **Phase 4:** disk-streaming sink, Opus, ffmpeg HE-AAC fallback.
5. **Release v2.10.0** — bundle bug fixes (incl. critical keyring) + batch. Bump version in 4 places (package.json, Cargo.toml, tauri.conf.json, **APP_VERSION src/App.tsx:10**). Signed installer, GitHub release, website.

## Deferred / low-priority
- Remaining **English** error strings in deep-internal paths users rarely hit (audio.rs stream init L145-285, settings.rs serialize L427-432, streaming.rs WS L46-96, injector.rs). Translate opportunistically; not urgent.
- `BatchError` enum still deliberately deferred (Hebrew String errors + `"בוטל"` sentinel work fine).

## Still pending from earlier sessions (unchanged)
- **v2.9.1→2.9.3 fixes LOCAL-ONLY, NOT pushed.** `origin/main` = v2.9.0. Critical **keyring** fix (all users lose API keys on restart) sits in these unpushed commits. Decision: bundle → release as **v2.10.0**.

## Key facts
- Signing: `~/.tauri/hebrew-dictation.key` (no password); `export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/hebrew-dictation.key)"` before `npm run tauri build`.
- Repo: `aihenryai/hebrew-dictation`. Website: `aihenryai/Henry-AI-website` (Cloudflare auto-deploy on push to main).
- whisper-rs 0.16 abort-callback bug is real — if upgrading whisper-rs later, re-check the LOCAL_ABORT workaround.
- Decode = symphonia 0.6 + rubato 3 (post-rewrite APIs): `GenericAudioBufferRef::copy_to_vec_interleaved` + rubato `process_all_into_buffer`.
- Dev run for testing: `npm run tauri dev` (repo root). Kill orphans: PowerShell `Get-CimInstance Win32_Process | ? CommandLine -match 'hebrew-dictation' | % { Stop-Process -Id $_.ProcessId -Force }`.
