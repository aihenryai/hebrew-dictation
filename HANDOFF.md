# Hebrew Dictation — Session Handoff (2026-06-25)

> **Next session: read this + `memory/hebrew-dictation.md` to continue.**

## Where we are

### Batch Transcription Phase 1 — CODE-COMPLETE, compiles, NOT yet runtime-verified
File upload → transcribe (cloud Deepgram **or** local whisper, offline) → editable RTL textarea → export TXT/DOCX / inject / copy, with cancel. Additive — short dictation untouched.

- **All code written + committed locally** (8 commits, `174ae59`…`d417912`). `cargo test` = **14 tests green**; frontend `npm run build` (tsc + vite) = clean.
- **Spec rev2** (`docs/superpowers/specs/2026-06-22-batch-transcription-design.md`, §14 authoritative) + **plan** (`docs/superpowers/plans/2026-06-25-batch-transcription-phase1.md`). Both reviewed by multi-agent passes.
- **Key build facts discovered:** symphonia **0.6** + rubato **3** are *post-rewrite* APIs (totally different from 0.5). decode.rs uses `GenericAudioBufferRef::copy_to_vec_interleaved::<f32>` + rubato `process_all_into_buffer` (handles chunk/partial/delay-trim/flush internally — a hand-rolled flush loop mismanaged offsets and was replaced).
- **What's left in Phase 1 = manual smoke only (Task 3.5, needs Henry):** run `npm run tauri dev`, test (1) cloud mp3 w/ Deepgram key, (2) local mp3 w/ downloaded model, (3) cancel mid-run, (4) real iPhone .m4a, (5) regression: short dictation responsive during a local batch, (6) empty/garbage file → clear Hebrew error.

### Concurrency invariant (the critical correctness point)
Local batch locks `whisper_engine` ONLY for `create_long_state()`, then runs `state.full()` off-lock in `spawn_blocking` → short dictation stays responsive during a multi-hour run. Cloud cancel drops the in-flight reqwest future via `tokio::select!` on a `Notify`; local cancel via `set_abort_callback_safe`.

## Still pending from the PREVIOUS session (unchanged)
- **v2.9.1→2.9.3 bug fixes still LOCAL-ONLY, NOT published.** `origin/main` = v2.9.0. Critical **keyring** fix (all users lose API keys on restart) is in these unpushed commits. Henry's decision (2026-06-25): **bundle bug fixes + batch feature → release together as v2.10.0** (do NOT publish 2.9.3 separately). If he changes his mind, the 2.9.1-2.9.3 patch is a quick publish.
- 2.9.3 installer built: `src-tauri/target/release/bundle/nsis/הכתבה בעברית_2.9.3_x64-setup.exe`. Pending Henry verify: hard-lock toggles + installer text.

## Next steps (in order)
1. **Henry runs the Phase 1 manual smoke** (above). Fix anything it surfaces.
2. **Phase 2:** Groq cloud + chunking (`chunk.rs`, 5-10min windows + de-dupe), `save_transcript_next_to` (header-less export + overwrite policy), accuracy toggle. Plan: write `docs/superpowers/plans/2026-06-2x-batch-transcription-phase2.md`.
3. **Phase 3:** long in-app meeting recording (RAM MVP, raised ceiling, VAD-off) → same orchestrator (cloud/local).
4. **Phase 4:** disk-streaming sink (ringbuf + hound writer thread), Opus encode, ffmpeg HE-AAC fallback.
5. **Release:** bundle everything → **v2.10.0** (minor). Bump version in 4 places (package.json, Cargo.toml, tauri.conf.json, **APP_VERSION in src/App.tsx:10**). Build signed installer, GitHub release, website update.

## Key facts
- Signing: `~/.tauri/hebrew-dictation.key` (no password); `export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/hebrew-dictation.key)"` before `npm run tauri build`.
- Repo: `aihenryai/hebrew-dictation`. Website repo: `aihenryai/Henry-AI-website` (Cloudflare auto-deploy on push to main).
- Release process: `gh release create` with 2 small assets first, then `gh release upload <exe> --clobber` (resilient to DNS blips). Git Bash: `gh api -X DELETE` needs `MSYS_NO_PATHCONV=1` + no leading slash.
- Phase 1 = Deepgram cloud only (no Groq batch yet); near-silence + empty-file guards in place; BatchError enum deliberately deferred (Hebrew String errors + `"בוטל"` sentinel).
