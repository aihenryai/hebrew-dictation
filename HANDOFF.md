# Hebrew Dictation — Session Handoff (2026-06-24)

> **Next session: read this + `memory/hebrew-dictation.md` to continue.**

## Where we are

### Shipped publicly (v2.9.0)
- **Smart Cleanup (רישוף חכם)** — opt-in post-transcription cleanup via Groq Llama. Live on GitHub release v2.9.0 + website (bintechai.com/hebrew-dictation).

### Built locally, NOT published (v2.9.3)
After v2.9.0, Henry reported bugs. Fixed + built signed installers up to **v2.9.3**. The 2.9.3 installer is at:
`src-tauri/target/release/bundle/nsis/הכתבה בעברית_2.9.3_x64-setup.exe` (signed, .sig present).

**9 commits are LOCAL-ONLY on `main` (74903e4 … 97e2538) — NOT pushed.** `origin/main` is still at v2.9.0. Don't push/publish until Henry says (he chose to bundle with the new feature — see Decisions).

5 bugs + 2 UX + NSIS fixed across 2.9.1→2.9.3 (all root-caused via systematic-debugging):
1. **keyring** (CRITICAL, all users): `keyring="3"` had no backend feature → in-memory MOCK store → keys lost on restart. Added `features=["windows-native","apple-native"]`. **Verified**: real `gsk_` key now persists in Credential Manager. (Deepgram had survived only via the `DEEPGRAM_API_KEY` env var.)
2. **settings reset**: persistSettings omitted onboarding/terms/toolbar_position → update_settings serde-defaulted them. Fixed via `AppSettings::merge_frontend_update`.
3. **mic-silence**: muted mic → empty transcript silently. Now `audio::is_effectively_silent` → clear Hebrew error. ✅ Henry confirmed.
4. **floating bar didn't pop up**: startup loads the 1.6GB ivrit model with `status="loading-model"`, which froze Alt+D + record button. Fixed: background (non-blocking) model load, `status` stays idle.
5. **cleanup returned a chat reply** ("send me your text"): Llama treated it as conversation. Fixed with a **few-shot prompt** (messy→clean example) + stricter "you are an editor, not a chat" system prompt. **Verified live** against Groq.
6. **UX**: cleanup ⇄ streaming now **hard-disable** each other (can't check one while the other is on) + explained in settings.
7. **NSIS**: installer/uninstaller Hebrew strings expanded (what "delete data" removes; what each already-installed option does; that user data is kept).

**Pending Henry verification of 2.9.3:** hard-lock toggles + new installer text. (Key-persist, mic, cleanup, bar all confirmed in earlier builds.)

## Next feature: Batch Transcription (the active work)
**Full design spec written + committed:** `docs/superpowers/specs/2026-06-22-batch-transcription-design.md` (4 phases, backed by a 7-agent research workflow).

**What it is:** upload an audio file OR record a long in-app session (30-90 min meeting) → transcribe → editable text area + export TXT/DOCX + optional inject + optional save-next-to-file. 100% client-side, BYO-key, Hebrew-first, no server.

**🔑 Henry's explicit requirement:** BOTH features (file upload AND recording) must work with the **LOCAL model** too (offline), not just cloud. Local is first-class.

**Per-provider (research-backed):** Deepgram Nova-3 = ONE sync request for long files (no chunking, 2GB limit). Groq = must chunk (5-min/3s-overlap) above ~13 min. Local whisper = single `state.full()`, but SLOW on CPU (90 min = 1.5-4 hrs), default `small` not turbo.

**Decode:** pure-Rust `symphonia` + `rubato` (NOT ffmpeg). Features `["mp3","aac","isomp4","alac","vorbis","ogg","wav","flac"]` — ⚠️ aac+isomp4 off by default; test a real iPhone .m4a early.

**3 load-bearing constants to change (currently BLOCK long audio):**
- `whisper.rs`: remove `TRANSCRIBE_TIMEOUT_SECS = 180` → cancellable + progress (needed for local file/long transcription).
- `api_transcribe.rs`: `.timeout(30s)` → 900s for a separate batch client + retry on 503/504.
- `audio.rs`: `MAX_RECORDING_CEILING_SECS = 3600` → raise in long-meeting mode.

## Next steps (in order)
1. **Continue the brainstorming→implementation flow:** spec-review loop on the batch spec (spec-document-reviewer subagent) → Henry reviews → `writing-plans` → implement.
2. **Phase 1 MVP** (cloud + local file upload): `decode.rs` + whisper.rs timeout fix + `transcribe_long` + `batch.rs` (Deepgram single-request + local whisper) + `transcribe_file` command + React panel with cloud/local toggle + textarea + export + inject + cancel.
3. **Phases 2-4:** Groq chunking + auto-save + accuracy toggle (2); long in-app recording cloud+local (3); disk-sink hardening + Opus + ffmpeg HE-AAC fallback (4).
4. **Release:** when ready, push main + build **v2.10.0** (minor — it's a feature, NOT 2.9.3) + GitHub release + website update. The keyring fix ships with it. **If Henry wants the critical keyring fix sooner, publish the 2.9.1-2.9.3 patch first** (push main, release, website bump) — it's a quick path.

## Decisions log
- Don't publish 2.9.3 separately — bundle bug fixes + batch feature, release as one **v2.10.0**.
- Secure key storage direction: **CLOSED** — Credential Manager (DPAPI) is sufficient (Henry's call).
- Product stays **free + BYO-key, client-side** (no managed/server model).

## Key facts
- Signing: `~/.tauri/hebrew-dictation.key` (no password); `export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/hebrew-dictation.key)"` before `npm run tauri build`.
- Bump version in 4 places: package.json, src-tauri/Cargo.toml, src-tauri/tauri.conf.json, **and `APP_VERSION` in `src/App.tsx:10`** (the displayed label — easy to forget).
- Release process (from earlier): GitHub release needs `gh release create` with 2 small assets first, then `gh release upload <exe> --clobber` (resilient to DNS blips). latest.json drives the updater. Git Bash: `gh api -X DELETE` needs `MSYS_NO_PATHCONV=1` + no leading slash.
- Repo: `aihenryai/hebrew-dictation`. Website repo: `aihenryai/Henry-AI-website` (Cloudflare auto-deploy on push to main).
