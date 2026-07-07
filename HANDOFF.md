# Hebrew Dictation — Session Handoff (2026-07-05)

> **Next session: read this + `memory/hebrew-dictation.md` + `memory/hebrew-dictation-changelog.md` to continue.**

## ✅ v2.11.0 — FULLY RELEASED (2026-07-05)

**Released end-to-end:** app on `main` (commit `0078eed` HEAD at release time) · GitHub Release [v2.11.0](https://github.com/aihenryai/hebrew-dictation/releases/tag/v2.11.0) with signed `.exe` + `.sig` + `latest.json` · website `bintechai.com/hebrew-dictation` updated with real "מה חדש" copy (not just version bump) · Henry manually verified everything in `npm run tauri dev` before release.

**Windows only.** macOS stays on v2.10.1 pending Yogev's next build — expected lag, not a bug (see macOS release recipe in `memory/hebrew-dictation.md` "Known Limitations").

## ⚠️ Found after release: real Mac user hit "app is damaged, move to Trash"

Not Hebrew/localization, not a Claude Code mistake — classic Gatekeeper "damaged" dialog (stricter than the usual "unidentified developer" one, no "Open Anyway" bypass). Likely cause: the `.app` was zipped for distribution with plain `zip`/Finder-compress instead of `ditto`, which can corrupt the ad-hoc signature; combined with the browser-download quarantine flag, Gatekeeper refuses to open it. **Before the next mac build request to Yogev, send him the packaging instructions in `memory/hebrew-dictation.md`'s macOS release recipe section (`ditto -c -k --sequesterRsrc --keepParent` + `codesign`/`spctl` verification) — don't let this repeat.** Website fixed in **three passes** (`151224c` → `51ea4fb` → `46d2edc`): first buried the Mac fix as a gray aside inside a box titled "Windows: SmartScreen" (Henry couldn't find it even after it shipped); second gave macOS its own equally-weighted, clearly-titled box; third swapped a "type xattr -cr then drag the file in" two-step for one direct copy-paste command with the exact filename Henry confirmed from the user's screenshot (`~/Downloads/הכתבה\ בעברית.app`) — he explicitly prefers a ready-to-run command with a stated assumption over a fool-proof multi-step flow, see `memory/feedback_copypaste_full_command.md`. **Lesson 1:** platform-specific help text needs an equally prominent, correctly-labeled home, never nested in the other platform's box. **Lesson 2:** this "one ready command, not a two-step dance" preference applies to instructions Henry forwards to end users too, not just commands he runs himself.

## 🎯 NEXT UP (Henry wants this next): meeting transcription — Zoom/Meet audio + speaker diarization

Both ideas below were flagged from the island-io/mila comparison; Henry has now confirmed he wants to actually start on this. Researched feasibility 2026-07-06 — concrete enough to start from, not just "look into it":

### 1. Speaker diarization — cheap, do this first
Deepgram's batch API already supports it natively: add `&diarize=true` to the existing request URL in `transcribe_deepgram_batch` (`api_transcribe.rs`), and each entry in the `words[]` array gains a `speaker: <int>` field (0, 1, 2…) alongside `word`/`start`/`end` — **the exact same array the SRT feature (v2.11.0) already parses.** This means diarization mostly falls out of infrastructure that already exists: extend `srt::TimedWord`/`TimedSegment` with an optional `speaker: Option<u32>`, thread it through `chunk_words_to_cues`, and either (a) label cues in the SRT output ("דובר 1: ...") or (b) surface it in the UI transcript view. **Cloud-only** — whisper.cpp (the local route) has no built-in diarization, so this feature would only work in Deepgram mode; decide with Henry whether that's an acceptable v1 scope (mirrors the existing "streaming is Deepgram-only" precedent) or whether local-mode users need a fallback message. One version-nuance to verify at implementation time: Deepgram's 2026-05 "Batch Diarization v2" model change — shouldn't affect Henry's standard (non-self-hosted) API usage, but worth a quick real-request check before shipping. **Numeric labels only** (speaker 0/1/2) — no name matching; that'd need separate UI (e.g., let the user relabel "דובר 1" → a real name after the fact).

### 2. System-audio capture for meetings — bigger lift, do second
Today's mic capture uses `cpal` (cross-platform). To also capture the OTHER side of a Zoom/Meet call (what's playing through the speakers, not just the mic), Windows needs **WASAPI loopback capture** — a distinct API mode, not just "another cpal input device." `cpal`'s own WASAPI backend is not confirmed to expose loopback directly; the dedicated [`wasapi`](https://docs.rs/wasapi) crate does, with a documented loopback capture example (simultaneous capture + render on separate threads) — that's the concrete starting point, verify at implementation time whether newer `cpal` has closed this gap. Real design questions Henry needs to weigh in on before implementation starts, not just engineering: (a) mix mic + loopback into one stream, or keep them as two channels/two transcripts merged after the fact (affects diarization quality — mixing loses the "which side of the call" signal for free, that a separate-channels approach would preserve); (b) new permission/UX flow (Windows will very likely surface its own "app is capturing audio" indicator, similar to screen-recording prompts) — needs product decisions, not just code. Recommend brainstorming this properly (spec+plan, like SRT export got) rather than jumping straight to implementation, given the open design questions.

**Suggested order:** diarization first (small, rides on existing SRT infrastructure, ships fast) → system-audio capture second (bigger, needs a real design pass — use the brainstorming skill before touching code, same as SRT export).

### What shipped

1. **SRT subtitle export** from batch file transcription — per-item and combined (multi-file, cumulative time offset, no gap) export, both cloud (Deepgram `words[]` bucketing, ~10 words/4s per cue) and local (whisper.cpp native `max_len(42)`/`split_on_word` segmentation) routes. New `srt.rs` pure module (9 unit tests) + `export_srt` command (mirrors `export_history`'s save-dialog pattern) + frontend "🎬 SRT" buttons gated by a shared `isSrtEligible` predicate (hidden if the transcript was hand-edited — segments would no longer match). Built via spec+plan+6 reviewed implementation tasks — see `docs/superpowers/specs/2026-07-03-srt-export-design.md` / `docs/superpowers/plans/2026-07-03-srt-export.md` for full detail if extending this later (e.g. the "Out of scope" section lists what v1 deliberately skipped: history export, filename-matches-video, configurable chunking, VTT).

2. **Floating idle-button focus bug, fixed at the actual root cause.** Symptom: dictating via the floating button (mouse click, not Alt+D) sometimes injected text nowhere instead of the target app. First fix attempt (extend the `inject_text` Tauri command's window-hide/restore trick to the "toolbar" window too) had **zero effect** — because `streaming_enabled` defaults to `true`, and streaming's live per-segment injection lives in a completely separate call site (`streaming.rs::handle_message`) that bypassed the command entirely. Real fix: extracted the hide→wait→inject→restore logic into a shared `inject_text_defocused(app, text)` helper in `lib.rs`, used by **both** the command and the streaming path. Henry confirmed fixed in both modes.

3. **Per-item action-button row layout** — 5 buttons (inject/copy/TXT/Word/SRT) no longer fit one row; fixed with a deliberate 2-row grouping (quick actions / export formats). Henry explicitly rejected icon-only labels (confused users historically) and plain flex-wrap ("doesn't look intentional") — don't re-propose either without new information.

### Gotcha for next release

`npm run tauri build` needs **both** `TAURI_SIGNING_PRIVATE_KEY` **and** `TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""` exported — the key file is in `rsign`-encrypted format even with a blank password, and omitting the password var silently skips the updater `.sig` generation with **no error message** (the `.exe` still builds fine). Cost me one full rebuild cycle this round — set both from the start next time.

## Not done — flagged for future, NOT scheduled

- **English → Hebrew translation** (upload/dictate in English, auto-translate to Hebrew) — Henry-confirmed roadmap idea from 2026-07-02, not started.
- **Windows SmartScreen / code-signing certificate** — researched 2026-07-05: as of 2026, EV certs no longer bypass SmartScreen instantly (Microsoft closed that loophole in 2024) — OV and EV now both need the same reputation-building period. Azure Trusted/Artifact Signing ($9.99/mo, cheapest option) is **not available to Israel** (individual devs limited to US/Canada; orgs to US/Canada/EU/UK). Realistic path: a standard OV cert (~$200-400/yr, e.g. Sectigo) — replaces "Unknown Publisher" with a verified name and starts the reputation clock, but doesn't eliminate the warning immediately. Henry has not yet decided whether to purchase — ask before assuming this is wanted.

## Key facts (unchanged, still accurate)

- Signing: `~/.tauri/hebrew-dictation.key` — **encrypted format, needs `TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""` explicitly** (see gotcha above; supersedes any earlier "no password" note).
- Repo: `aihenryai/hebrew-dictation`. Website: `aihenryai/Henry-AI-website` (Cloudflare auto-deploy on push to main).
- Dev run for testing: `npm run tauri dev` (repo root) — Rust changes need the process restarted, not just HMR (HMR only covers the frontend).
- Kill orphaned dev processes: PowerShell `Get-CimInstance Win32_Process | ? CommandLine -match 'hebrew-dictation' | % { Stop-Process -Id $_.ProcessId -Force }`.
