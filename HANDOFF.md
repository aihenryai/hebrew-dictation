# Hebrew Dictation — Session Handoff

> **Next session: read this + `memory/hebrew-dictation.md` + `memory/hebrew-dictation-changelog.md` to continue.**

---

## ✅ 2026-07-19 — FIXED: `enhance_inner` silently accepted truncated Groq completions (`dd082f8`)

Found while designing translation; **independent of that feature — it was a real defect in the shipped Smart Cleanup path.** `enhance_inner` read `choices[0].message.content` without checking `finish_reason`, so a completion Groq cut off at its output cap returned partial text that — being shorter than the input — passed `validate_output`'s only guards (empty, >2× length). Silent truncated cleanup, no error.

Fixed via a pure `parse_completion(body, raw)`: `finish_reason == "length"` → new `EnhanceError::Truncated`, then the existing content validation. Matched explicitly on `"length"` (not `!= "stop"` — the file's `as_str().unwrap_or("")` reads an absent field as `""`, which would fail every response). 5 parser-level tests on canned bodies including the absent-field trap. `cargo test` **68 / 1 ignored**, clippy clean. Not released — ships with the next build.

---

## 🧊 2026-07-19 — EN→HE translation: spec written and 3× reviewed, then SHELVED (Henry's call)

Spec: `docs/superpowers/specs/2026-07-19-en-to-he-translation-design.md` — brainstormed with Henry, three full review rounds, all issues fixed. **No code written.** Stopped at the pre-implementation gate because the scope grew far beyond the original "add an `EnhanceMode` variant" framing *and* a feasibility check came back badly.

**⛔ The feasibility finding — read this before reviving the feature.** Groq free tier for `llama-3.3-70b-versatile`: **12,000 TPM / 100,000 TPD** ([official](https://console.groq.com/docs/rate-limits)). A 1-hour podcast ≈ 60,000 chars ≈ 20 chunks ≈ **~50,000 tokens**, so:
- TPM caps throughput at ~4-5 chunks/minute → a 1-hour file takes **4-5 minutes minimum**, no matter how fast Groq is. Pacing is required, not just backoff.
- **One podcast burns half the daily token budget.** Two per day hits the cap.

→ **If revived, decide the provider first.** Henry has `OPENAI_API_KEY` and `GEMINI_API_KEY` too; bulk translation is a completely different load profile from cleaning one dictated sentence, and the two need not share a provider. Do not start implementing before this is settled.

**What the spec settled (Henry's decisions, don't re-litigate):** picked files only (not dictation, recordings, or meeting modes) · per-action checkbox, never a persistent global mode · failures are loud, English left intact · Hebrew replaces the English · chunked translation (a single call is provably unworkable — spec §4.1).

**Non-obvious traps the reviews found, all already written into the spec:**
1. The 2× output ceiling **rejects correct short translations** — `"AI"` → `"בינה מלאכותית"` is 6.5×. The ceiling must become mode-owned.
2. **`ivrit-*` whisper models force Hebrew regardless of the language argument** (`whisper.rs` ~82-86, ~187-192), so an English file would decode to garbage and then be "translated". Needs a pre-flight guard.
3. **Translation desynchronizes `transcript` from `segments`**, so SRT export would emit English subtitles under a Hebrew transcript. Needs a `translated` flag in `isSrtEligible` — *not* reuse of `edited`.
4. Batch **hardcodes `language: "he"`** (`App.tsx:1006`) and never reads the global setting.
5. Meeting-mode text carries `"אני: "` / `"הצד השני: "` as plain content an LLM would mangle.

---

## 📦 2026-07-19 — v2.12.1 RELEASE PREPARED (version bumped, green) — awaiting Henry's signed build

Version bumped **2.12.0 → 2.12.1** in all 4 files (package.json, Cargo.toml, tauri.conf.json, Cargo.lock, commit `e1fc6ec`). Release candidate verified: `cargo test` **68/1**, `tsc && vite build` clean. **Not pushed, not built, not released** — the signed build needs Henry's `TAURI_SIGNING_PRIVATE_KEY`.

What 2.12.1 contains: the `/transcript` `{text, seq, at}` rework (was returning fragments under streaming) + the `finish_reason` truncation fix in Smart Cleanup. No consumer-facing UI change — this is a reliability release plus a dev-only opt-in API.

**Henry's remaining steps are a runbook** (build → rename → merge latest.json keeping the darwin block → `gh release create` → website version bump only): it was written to the session scratchpad (`v2.12.1-runbook.md` + `v2.12.1-release-notes.md`). If that scratchpad is gone, the recipe is in `memory/hebrew-dictation.md` and the v2.12.0 changelog entry; the only new wrinkles this round are (a) the website "מה חדש" headline is an editorial call since there's no consumer feature, and (b) `prerender-seo.js` showed no dictation version string — verify before bumping it.

⚠️ **The MCP server cannot work until 2.12.1 is installed** — the running 2.12.0 returns `{text}` only, which the MCP's payload validator correctly rejects. So the mic-verify (below, #1) and MCP registration both wait on this build.

---

## 🆕 2026-07-19 — dictation MCP server + a latent `/transcript` bug fixed. NOT RELEASED.

Full brainstorm → spec → plan → subagent-TDD, two review gates per task. Spec: `docs/superpowers/specs/2026-07-19-dictation-mcp-design.md`. Plan: `docs/superpowers/plans/2026-07-19-dictation-mcp.md`.

**App (`main`, `43cf9fd`→`0b5dbee`, 63 tests / 1 ignored, 0 warnings):**
- `GET /transcript` now returns **`{text, seq, at}`** (additive — `text` still there). `seq` = monotonic counter, `at` = unix ms.
- 🐞 **Latent bug fixed:** `streaming_enabled` defaults **true**, and `streaming.rs:163` injects **per final segment**. The old stamp lived inside `inject_text_defocused`, so the shipped `/transcript` returned only the **last fragment** of a streaming dictation — i.e. it has been returning fragments to anyone using it since it shipped. Stamping moved to the two **utterance boundaries**: the `inject_text` command (`lib.rs`) and `stop_streaming_transcription`. `local_api::record_utterance` is now the single mutation door (`bump_transcript`/`transcript_json`/`now_unix_ms` are private, so the empty-guard can't be bypassed).
- ⚠️ **`seq` counts completed-transcript EVENTS, not spoken utterances** — a paste of an older batch result bumps it too and stamps `at` with *paste* time. This distinction escaped into docs **four times** during the build (field comment → struct doc → module header → tool description). If you touch this, grep the whole crate for the claim; don't just fix the site you were pointed at.

**MCP server — NEW: `AI-Tools/MCP-Dev/hebrew-dictation-mcp/` (its OWN git repo, `574319d`→`e184adb`, 12 tests):**
- Node/TS, `@modelcontextprotocol/sdk` ^1.29, **stdio**. `src/client.ts` (all logic, zero MCP imports, unit-tested vs a real local mock HTTP server) + `src/index.ts` (pure wiring). README has the `.mcp.json` snippet.
- Tools: **`get_last_transcript`** (non-blocking) · **`wait_for_dictation`** (blocks until `seq` exceeds a per-call baseline) · **`dictation_status`** (diagnostics).
- ⚠️ **It never starts the mic.** It only watches the counter; you dictate with the app's own hotkey. The name misleads — the descriptions say so explicitly.
- ⚠️ **`timeout_seconds` defaults to 55, not 120, on purpose.** The MCP SDK's `DEFAULT_REQUEST_TIMEOUT_MSEC = 60000`, so a client aborts the tool call at 60s with a generic error before a longer wait could return gracefully. Measured: 55.4s graceful, 4.6s margin. The 1..600 range is real but >55 needs the *client's* request timeout raised. Proper fix (progress notifications) deferred — needs a `progressToken` the client may never send; analysis is in a `// TODO` in `index.ts`.

### ⏳ What's left — NOT done

1. **🎤 The one hop never tested end-to-end (needs a human + mic).** Everything from `/transcript` rightward was verified against a live Rust server driving a live MCP client. Left of it — mic → Deepgram → `final_text` accumulation → `record_utterance` — is untested. **Sharpest surviving assumption: is `final_text` complete when `record_utterance` reads it?** `stop()` waits **at most 5s** (`streaming.rs:120`) for Deepgram's trailing `is_final` messages; a late one means `seq` bumps with a **truncated** sentence. (It truncates the UI identically, so it's pre-existing and symmetric — but the MCP is the first consumer that can't eyeball the result.)

   Set `"local_api_enabled": true` in settings.json, restart the app, run this, then press Alt+D, say **one multi-clause sentence**, press Alt+D again:
   ```
   powershell -NoProfile -Command "while($true){try{$r=Invoke-RestMethod http://127.0.0.1:5757/transcript;Write-Host ('seq={0}  text={1}' -f $r.seq,$r.text)}catch{Write-Host 'not reachable'};Start-Sleep 1}"
   ```
   **Pass = `seq` rises by exactly 1 (not 3) and `text` is the whole sentence.** Falsifies both the per-fragment-bump and truncation risks at once. This is spec §6's acceptance criterion.

2. **Not released, not registered.** No GitHub Release, no version bump, no auto-update. The MCP server isn't in any `.mcp.json` yet.

3. **Open decision — the non-streaming injection gate.** `inject_text` records only `if result.is_ok()`, while `stop_streaming_transcription` records unconditionally. So with `streaming_enabled: false`, a dictation whose injection fails (`injector.rs:42,45` can error; `App.tsx:495-501` swallows it with an empty `catch {}`) never bumps `seq` — `wait_for_dictation` times out and the agent reports "no dictation" though the user spoke, silently. Not a regression (matches old behavior), but it contradicts the "last completed transcript" concept. Either record on both paths regardless of injection, or document the gate.

4. **Known UX seam (inherent, not a bug).** The app injects into whatever holds OS focus — when you dictate *for the agent*, that's the Claude Code prompt box. The sentence lands in your input box **and** returns through the tool, so you must clear the box before pressing enter. Unavoidable while "no remote-record trigger" stays a non-goal.

5. **Deferred, recorded in TODOs:** `outputSchema` for the three tools (purely additive later) and `structuredContent` shape consistency (**not** additive once automation writes against it — currently a non-issue only because the package is `"private": true`, publishing is a spec non-goal, and the sole consumer is in-workspace). Minor: `dictation_status`'s `reachable` can never be `false` (the unreachable path returns `isError` instead); `record_utterance` is arguably misnamed given `seq` counts events not utterances; README doesn't name the `local_api_port` setting.

---

## ✅ v2.12.0 — FULLY RELEASED (2026-07-14) — meeting transcription

**Released end-to-end + verified each hop (not assumed):**
- **App:** `main` @ `b91e076`, version bumped in all 3 files (package.json / Cargo.toml / tauri.conf.json). All work pushed to `origin/main`.
- **GitHub Release [v2.12.0](https://github.com/aihenryai/hebrew-dictation/releases/tag/v2.12.0)** = **Latest**, signed `.exe` (204MB) + `.sig` + `latest.json`. ✅ **macOS block in `latest.json` preserved byte-identical** (still points to v2.10.1 — mac auto-update NOT broken). Updater endpoint `releases/latest/download/latest.json` verified HTTP 200 → `2.12.0`, both platform blocks present.
- **Auto-update IS LIVE:** every Windows user on 2.11.0 gets 2.12.0 on next launch.
- **Website** `bintechai.com/hebrew-dictation` (repo `Henry-AI-website` @ `c1f8d7d`) → 2.12.0, real "מה חדש" copy (2 meeting-mode cards replacing SRT/batch), new answer-first `<section>` in `prerender-seo.js` content + 2 `featureList` entries. Both domains verified live HTTP 200 with the new content. (No arrows — site RTL rule.)

**What shipped in 2.12.0** (full brainstorm→spec→plan→subagent-TDD, specs/plans in `docs/superpowers/{specs,plans}/2026-07-12-recording-modes-ux.md`):
1. 🔒 **`CallLocal` — private on-device meeting:** mic+system → `audio::mix_to_mono` → **forced-local whisper**, no speaker separation, audio never leaves the machine. Pre-record model guard. `cargo test` 55/1, two-stage review ✅.
2. 📞 **`CallCloud`** (renamed from `Call`) — speaker separation "אני:/הצד השני:". **Real-audio VERIFIED in a live Zoom call 2026-07-13.**
3. 🎛 **Source-selector regroup:** two groups (הקלטה רגילה / פגישות) + a **light `engine-toggle`** (☁ ענן / 💾 מקומי) at the top that dims for meetings. Meeting cards benefit-labeled, symmetric desc lines ("כל אחד בנפרד" vs "יחד, ללא הפרדה לדוברים"). ⚠️ the "· קובעות מנוע בעצמן" header note was tried and **Henry rejected it as redundant — do not re-add**.
4. ⚙ **Settings reachable from the batch screen too** — labeled turquoise `.btn-settings-labeled` (⚙ הגדרות) on home + batch header; tracks a **return view** so "חזור" from settings returns to origin. ⚠️ **CSS class-name collision lesson:** a labeled settings button was first named `.btn-settings`, which already existed (a 36px round icon rule) → it was forced into a circle with the label spilling out of the frame. Renamed to `.btn-settings-labeled`. **Before adding a class, grep App.css for the name.** The `.container` is only **340px** wide — measure header/controls rows for overflow (DOM measurement harness works when screenshots don't).
5. 🐞 **Onboarding API-key silent-discard bug FIXED** (`4f4ac3c`): the `.wizard-card` div's `onClick` reset the form and the key `<input>`/"בדוק" button were nested inside it, so a click bubbled up and wiped the typed key → `set_api_key` never called, no error. Fixed 3 ways (early-return on re-click, `stopPropagation`, and `completeOnboarding` now re-reads `get_settings` to VERIFY the key landed). Full write-up in `memory/hebrew-dictation.md`.

### ⏳ ONLY open item — real-audio manual verify of the LOCAL meeting mode (Henry, can't automate)
`CallCloud` was verified in Zoom; **`CallLocal` (🔒 פגישה — פרטית במכשיר) has NOT been exercised on real audio yet.** With a whisper model downloaded (Henry has `small`): install 2.12.0 → תמלול קובץ → 🔒 פגישה — פרטית במכשיר → record while system audio plays → stop → expect ONE local mono transcript, **no "אני:/הצד השני:"**. Low-risk (reuses the same dual-recorder capture Zoom already proved; only the mix+local-transcribe tail is unexercised on live audio), but not yet confirmed. If it fails, that's the thing to debug next session.

### Backlog (unchanged, not scheduled)
English→Hebrew translation · Windows code-signing cert (kills SmartScreen "unknown publisher") · Local-API MCP wrapper (#1 local API shipped, #2 MCP adapter pending) — see the "Voicebox comparison" section below.

---

## ✅ v2.11.0 — FULLY RELEASED (2026-07-05)

**Released end-to-end:** app on `main` (commit `0078eed` HEAD at release time) · GitHub Release [v2.11.0](https://github.com/aihenryai/hebrew-dictation/releases/tag/v2.11.0) with signed `.exe` + `.sig` + `latest.json` · website `bintechai.com/hebrew-dictation` updated with real "מה חדש" copy (not just version bump) · Henry manually verified everything in `npm run tauri dev` before release.

**Windows only.** macOS stays on v2.10.1 pending Yogev's next build — expected lag, not a bug (see macOS release recipe in `memory/hebrew-dictation.md` "Known Limitations").

## ⚠️ Found after release: real Mac user hit "app is damaged, move to Trash"

Not Hebrew/localization, not a Claude Code mistake — classic Gatekeeper "damaged" dialog (stricter than the usual "unidentified developer" one, no "Open Anyway" bypass). Likely cause: the `.app` was zipped for distribution with plain `zip`/Finder-compress instead of `ditto`, which can corrupt the ad-hoc signature; combined with the browser-download quarantine flag, Gatekeeper refuses to open it. **Before the next mac build request to Yogev, send him the packaging instructions in `memory/hebrew-dictation.md`'s macOS release recipe section (`ditto -c -k --sequesterRsrc --keepParent` + `codesign`/`spctl` verification) — don't let this repeat.** Website fixed in **three passes** (`151224c` → `51ea4fb` → `46d2edc`): first buried the Mac fix as a gray aside inside a box titled "Windows: SmartScreen" (Henry couldn't find it even after it shipped); second gave macOS its own equally-weighted, clearly-titled box; third swapped a "type xattr -cr then drag the file in" two-step for one direct copy-paste command with the exact filename Henry confirmed from the user's screenshot (`~/Downloads/הכתבה\ בעברית.app`) — he explicitly prefers a ready-to-run command with a stated assumption over a fool-proof multi-step flow, see `memory/feedback_copypaste_full_command.md`. **Lesson 1:** platform-specific help text needs an equally prominent, correctly-labeled home, never nested in the other platform's box. **Lesson 2:** this "one ready command, not a two-step dance" preference applies to instructions Henry forwards to end users too, not just commands he runs himself.

## 🍎 macOS audit + fixes (2026-07-07) — "no audio" root cause found & fixed in-repo

A Mac user (on the lagging v2.10.1 build) kept hitting **"לא נקלט קול מהמיקרופון … הגדרות Windows ← פרטיות ← מיקרופון"**. Full audit (systematic-debugging + codebase sweep) found the app has **zero macOS configuration** — no `bundle.macOS`, no `Info.plist`, no entitlements, and **not one `#[cfg(target_os="macos")]` anywhere**. That single gap cascades into every Mac problem below.

### ✅ Fixed in-repo this session (Windows-verified: 29/29 tests green; `generate_context!` accepts the new config)
- **P1 · mic permission = root cause of "no audio".** Capture is **native** (cpal→CoreAudio, NOT WebView getUserMedia), so macOS TCC gates it. With no `NSMicrophoneUsageDescription`, the OS denies the mic → cpal gets **silence** → the `is_effectively_silent` guard (`lib.rs:262`) fires. Fix: added `src-tauri/Info.plist` (`NSMicrophoneUsageDescription` — Tauri auto-merges it) + `src-tauri/Entitlements.plist` (`com.apple.security.device.audio-input` — required under Tauri's default hardened runtime) + a `"macOS"` block in `tauri.conf.json` (`entitlements` + `minimumSystemVersion: 10.15`). NB: the Info.plist key helps immediately; the entitlement matters once hardened-runtime/signing is on.
- **P2 · error text sent Mac users to *Windows* Settings.** String was hardcoded, no platform branch. Fix (TDD): pure `mic_permission_path_for(os)` helper in `lib.rs` (macOS → "הגדרות המערכת ← פרטיות ואבטחה ← מיקרופון"), wired at `lib.rs:262`, new test `mic_permission_path_is_platform_specific`.

### ⚠️ MUST verify on a real Mac (can't from Windows)
Rebuild the Mac app with the new config → does the **mic-permission prompt** appear on first record and audio capture? **Immediate triage for the current user:** does **"הכתבה בעברית"** appear under **System Settings → Privacy & Security → Microphone**? Listed → toggle on (instant relief). **Not listed** → confirms the missing-usage-string root cause; needs the rebuild.

### 🔧 Still open — needs macOS-specific code + a Mac to test (NOT started)
- **P3 · text injection (enigo→CGEvent) needs Accessibility permission.** ✅ *Partial (this session):* added a `#[cfg(target_os="macos")]` `AXIsProcessTrusted()` guard in `injector::inject_text` that returns actionable Hebrew guidance (→ הגדרות המערכת ← פרטיות ואבטחה ← נגישות) instead of silently typing nothing. Windows-verified (cfg-excluded there; hint unit-tested) — but **the macOS FFI itself is unverified; must compile-check on a real Mac build.** ⏳ *Remaining:* the guidance only reaches the **command/batch** path — the default **streaming** path does `let _ = inject_text_defocused(...)` (swallows the Err), so streaming users still get no feedback. Needs a proactive check/banner (`AXIsProcessTrustedWithOptions` prompt, or a startup/dictation-start check). `inject_text_defocused`'s 80ms hide/restore (`lib.rs:993-1042`) is also Windows-tuned.
- **P6 · default hotkey `alt+d` = Option+D on Mac** (dead-key → `∂`); registers but poor UX. Consider a `#[cfg(target_os="macos")]` Cmd-based default (avoid Cmd+D = bookmark). UI hardcodes "Alt + D" strings too.
- **P7 (low) · no cpal sample-format negotiation** (`audio.rs` assumes f32) — a non-f32 default input would fail.

### 🏗️ Needs Yogev's Mac + Apple Developer ID (build/signing, not code)
- **P4 · "app is damaged" (Gatekeeper)** — no signing/notarization (ad-hoc sig, fragile; see the packaging section above). Real fix: Developer ID signing + notarization + `ditto`. Same signing-cert decision as Windows SmartScreen (both un-decided).
- **P5 · no Mac auto-update** — `bundle.targets: ["nsis"]` emits Windows artifacts only, so `latest.json` has no macOS entry. Add a macOS bundle target on the Mac build.

### ❓ Coordination blocker — confirm before assuming the fix ships
**How does Yogev build the Mac version?** `targets: ["nsis"]` (Windows-only) means his Mac build overrides the target somehow. Builds **from this repo** on a Mac → P1 config flows in automatically. Uses a **separate config/fork** → the new `Info.plist` + `Entitlements.plist` + `bundle.macOS` block must be ported to his setup.

## 🎯 NEXT UP: meeting transcription — Zoom/Meet audio + speaker diarization

Both ideas were flagged from the island-io/mila comparison. Diarization (#1) is now **code-complete** (below); system-audio (#2) is next and still needs a design pass.

### ✅ 1. Speaker diarization — CODE-COMPLETE this session (TDD, 2026-07-07), one live check before ship
Done via red-green-refactor (the Deepgram parser had **zero** tests before — now covered). Touched only `srt.rs` + `api_transcribe.rs` (+ `whisper.rs` sets `speaker: None`), exactly as scoped:
- `TimedWord`/`TimedSegment` gained `speaker: Option<u32>` (`#[serde(default)]` on the segment so it survives the export IPC round-trip). `chunk_words_to_cues` now splits a cue on speaker change; `flush_cue` stamps each cue's speaker. With diarization off every word is `None`, so `None != None` never fires → behavior byte-identical to before.
- Extracted `parse_deepgram_words()` (pure, unit-tested) reads `w["speaker"]`; `transcribe_deepgram_batch` calls it and now sends `&diarize=true`. **Cloud-only** by design — whisper.cpp has no diarization (mirrors the existing "streaming is Deepgram-only" precedent).
- **Output = auto-label (Option A):** `render_srt` prefixes cues with `דובר 1:` / `דובר 2:` **only when a file actually has ≥2 speakers** — single-speaker dictation stays byte-for-byte clean, multi-speaker calls get labels with no toggle. Counted per-file (a clean file in a mixed batch export isn't labeled just because a sibling had two speakers). To switch to always-on / a manual checkbox = flip one condition in `render_srt`. Numeric labels only (Deepgram 0 → "דובר 1"), no name matching.
- **No TypeScript change needed:** App.tsx passes `segments` back to `export_srt` opaquely (pass-through, not reconstructed), so `speaker` survives without touching the TS interface. Surfacing speakers in the UI transcript view = optional follow-up, not v1.
- **Tests: 28/28 green** (`srt` 12, incl. 3 new speaker tests; `api_transcribe` 2 new parser tests — first ever for that parser). No new compiler/clippy warnings in the touched files.
- ⏳ **SHIP-GATE (only open item):** one real diarized request — `nova-3` + `he` + `diarize=true` on a **2-speaker Hebrew** clip — to confirm Deepgram actually populates `speaker` in `words[]` (the flagged 2026-05 "Batch Diarization v2" nuance) and the exported SRT shows `דובר 1:/דובר 2:`. Needs Henry's Deepgram key + a two-person recording (or just run the app on any 2-voice audio → export SRT). **Not yet run. Not yet committed.**

### 2. System-audio capture for meetings — ✅ ALL 20 TASKS DONE + reviewed 2× + PUSHED · ✅ REAL-AUDIO VERIFIED 2026-07-13 (Henry, live Zoom call, CallCloud/"פגישה בענן" — speaker separation "אני:/הצד השני:" worked). NOT released to users.

> **State:** brainstormed → spec approved (`docs/superpowers/specs/2026-07-09-system-audio-capture-design.md`, `e44977e`) → **20-task TDD implementation plan** authored + adversarially reviewed (`docs/superpowers/plans/2026-07-09-system-audio-capture.md`) → **fully implemented under strict TDD via subagent-driven-development (2026-07-10).**
> **Design locked:** three sources (`Mic`/`System`/`Call`). Call captures mic + system separately → stereo WAV (L=mic, R=system) → Deepgram `multichannel=true` → "אני"/"הצד השני". Batch-only v1, cloud-only (multichannel is Deepgram-only), Windows-only via the `wasapi` crate.
> **DONE (2026-07-10):** Chunks 3-6 (Tasks 6-20) all landed as atomic commits `68a688a` → `0073d8b`, plus a post-review Critical fix `af30355`. `cargo build` = **0 warnings**, `cargo test` = **50 passed, 1 ignored** (the `#[ignore]`d loopback capture), frontend `tsc && vite build` = clean. The full Mic/System/Call flow is wired end-to-end: source selector (Windows-gated) → `start_batch_recording(source)` → WASAPI loopback + cpal mic → `stop_call_recording` → `interleave_stereo` → `samples_to_wav_stereo` → `transcribe_deepgram_multichannel` → "אני:/הצד השני:" text + per-file `SpeakerLabelStyle` SRT export.
> **⚠️ wasapi 0.23 gotcha (fixed):** `get_default_device` is a `DeviceEnumerator` **method**, not a free fn — plan snippet was wrong, shipped code uses `DeviceEnumerator::new().and_then(|e| e.get_default_device(&Direction::Render))`.
> **⚠️ Final-review Critical (fixed `af30355`):** the Cancel button renders for every source but `cancel_batch_recording` only stopped the mic → a cancelled System/Call left the loopback thread running + bricked all future System/Call starts (re-entrancy guard) until restart. Cancel now drains the system recorder too.
> **✅ REAL-AUDIO VERIFIED 2026-07-13:** Henry ran CallCloud ("פגישה בענן") in a live Zoom call — speaker separation into "אני:/הצד השני:" worked. This proves the whole chain end-to-end (WASAPI loopback + dual-recorder capture + stereo interleave + Deepgram multichannel). The `#[ignore]`d `loopback_captures_playing_audio` unit test remains optional/never-run, but the behavioral gate it stood in for is now PASSED via the live call.

The original research that led here:
Today's mic capture uses `cpal` (cross-platform). To also capture the OTHER side of a Zoom/Meet call (what's playing through the speakers, not just the mic), Windows needs **WASAPI loopback capture** — a distinct API mode, not just "another cpal input device." `cpal`'s own WASAPI backend is not confirmed to expose loopback directly; the dedicated [`wasapi`](https://docs.rs/wasapi) crate does, with a documented loopback capture example (simultaneous capture + render on separate threads) — that's the concrete starting point, verify at implementation time whether newer `cpal` has closed this gap. Real design questions Henry needs to weigh in on before implementation starts, not just engineering: (a) mix mic + loopback into one stream, or keep them as two channels/two transcripts merged after the fact (affects diarization quality — mixing loses the "which side of the call" signal for free, that a separate-channels approach would preserve); (b) new permission/UX flow (Windows will very likely surface its own "app is capturing audio" indicator, similar to screen-recording prompts) — needs product decisions, not just code. Recommend brainstorming this properly (spec+plan, like SRT export got) rather than jumping straight to implementation, given the open design questions.

**Suggested order:** ~~diarization first~~ ✅ code-done (pending one live 2-speaker check) → ~~system-audio design pass~~ ✅ spec + reviewed 20-task plan done (2026-07-09) → ~~implement the plan~~ ✅ **DONE + pushed (`60b086c`, 2026-07-12)**.

---

## ✅ DONE 2026-07-12 — 4th recording mode (`CallLocal`) + source-selector UI regroup

**State:** brainstorm (all 4 design Qs closed with Henry) → spec (`docs/superpowers/specs/2026-07-12-recording-modes-ux.md`, reviewer-approved) → 6-task TDD plan (`docs/superpowers/plans/2026-07-12-recording-modes-ux.md`, reviewer-approved) → **fully implemented via subagent-driven TDD, committed + PUSHED to origin/main (`6949ed7`→`75863af`, 2026-07-12; Henry approved the push). NOT released to users (no GitHub Release / installer / auto-update).** `cargo test` = **55 passed / 1 ignored**, `cargo build` = **0 warnings**, clippy = clean for touched code (6 warnings are all pre-existing in untouched files), `tsc && vite build` = clean. Final two-stage review (spec-compliance + code-quality) = both ✅, no Critical/Important.

**What shipped (per the design below):** enum `Call`→`CallCloud` + new `CallLocal`; pure `mix_to_mono` (audio.rs, avg+silence-pad); `CallLocal` drains BOTH recorders → mixes to mono → existing mono file path → **forced-local whisper**; pre-record model guard (symmetric to CallCloud's Deepgram-key guard); meeting-specific silence message. Frontend: two labeled groups ("הקלטה רגילה" / "פגישות"), benefit-led meeting cards ("עם זיהוי דוברים" / "פרטית במכשיר"), context-dependent cloud/local selector (shown only for mic/system), standalone transparency note deleted.

**⏳ ONLY open item — MANUAL-VERIFY on real Windows audio (Henry, can't automate):** with a whisper model downloaded, `npm run tauri dev` → batch view → pick **פגישה — פרטית במכשיר**, speak while system audio plays, stop → expect one **local mono** transcript (no "אני/הצד השני"). Also: no-model → guard error fires *before* recording; cancel mid-CallLocal then start a meeting again works (system-recorder drain, the af30355 regression gate); the cloud/local cards appear only for mic/system. Then push when satisfied.

---

## 📐 Original design (approved by Henry 2026-07-12) — for reference

**Goal:** add a 4th mode and reorganize the batch source selector for MAXIMUM UX clarity (Henry's explicit #1 priority — "מאוד מאוד חשוב שחוויית המשתמש תהיה מאוד מאוד ברורה").

**The 4 modes, in 2 visual groups:**
- **קבוצה א׳ — הקלטה רגילה:** 🎙 מיקרופון · 🔊 אודיו מערכת (both mono; both keep the cloud/local choice)
- **קבוצה ב׳ — פגישות:**
  - 📞 **פגישה בענן** — mic+system SEPARATED (stereo → Deepgram multichannel), labeled "אני:"/"הצד השני:". = today's `Call`. Cloud-only.
  - 🔒 **פגישה מקומית** — mic+system MIXED into ONE mono transcript, transcribed LOCALLY (whisper), **NO speaker separation**. = the NEW mode. Privacy: audio never leaves the machine.

**Why the new local mode:** `Call` is cloud-only (multichannel is Deepgram-only), so a privacy-conscious user can't transcribe a meeting without uploading audio. Mode 4 fills that gap; trade-off = losing "who said what" (local whisper has no diarization).

**Open design questions for the (brief) brainstorm→spec:**
1. Exact button labels + group headers / visual separation.
2. **The cloud/local mode-card interaction (the crux):** today a separate "מצב תמלול" cloud/local selector (`batchMode`) exists. Once the meeting buttons ENCODE cloud vs local, that separate selector becomes redundant/confusing for meetings. Decide: hide cloud/local cards for meeting modes? apply them only to Mic/System? Henry's clarity bar lives here.
3. Mode-4 mechanics: MIX mic+system to mono (new `mix_to_mono(mic, system)` — average the two 16k-mono buffers, pad shorter side with silence like `interleave_stereo` does) → existing `write_wav_16k_mono` → frontend `transcribe_file` with LOCAL mode. Force local, or default-local-allow-cloud?
4. Backend `RecordingSource` naming: today `Mic`/`System`/`Call`; add a 4th (`Call`=cloud, add `CallLocal`/`Meeting`) or rename to `CallCloud`/`CallLocal` for clarity.

**Technical starting points (grounded in shipped code):**
- `recorders_for_source` (batch.rs): mode 4 drives BOTH recorders → `(true, true)`, like Call.
- Mode-4 stop path = closer to the System file-path than to Call's inline multichannel: stop both → `mix_to_mono` → `write_wav_16k_mono` → return path → `transcribe_file` (local). Possibly a mixing branch inside `stop_batch_recording_to_file`.
- Windows-only for both meeting modes (WASAPI). Reuse the shipped `SystemAudioRecorder`.
- The Call cloud-transparency note added in `dfb14d7` (App.tsx) becomes moot/relocated once cloud-vs-local is explicit in the buttons — revisit it.

**Process (same discipline as the shipped feature):** brainstorming (Henry DECLINED mockups → go straight to a tight spec) → `docs/superpowers/specs/2026-07-1X-recording-modes-ux.md` → spec-review loop → user review → `superpowers:writing-plans` → `superpowers:subagent-driven-development` TDD (red→green→atomic commit, controller verifies each, adversarial review at the end). Direct-to-main, no PR.

**Baseline to build on:** `main` == `origin/main` @ `60b086c`; `cargo test` 50 passed +1 ignored; frontend `tsc && vite build` clean; signed installer at `src-tauri/target/release/bundle/nsis/הכתבה בעברית_2.11.0_x64-setup.exe`.

---

### What shipped

1. **SRT subtitle export** from batch file transcription — per-item and combined (multi-file, cumulative time offset, no gap) export, both cloud (Deepgram `words[]` bucketing, ~10 words/4s per cue) and local (whisper.cpp native `max_len(42)`/`split_on_word` segmentation) routes. New `srt.rs` pure module (9 unit tests) + `export_srt` command (mirrors `export_history`'s save-dialog pattern) + frontend "🎬 SRT" buttons gated by a shared `isSrtEligible` predicate (hidden if the transcript was hand-edited — segments would no longer match). Built via spec+plan+6 reviewed implementation tasks — see `docs/superpowers/specs/2026-07-03-srt-export-design.md` / `docs/superpowers/plans/2026-07-03-srt-export.md` for full detail if extending this later (e.g. the "Out of scope" section lists what v1 deliberately skipped: history export, filename-matches-video, configurable chunking, VTT).

2. **Floating idle-button focus bug, fixed at the actual root cause.** Symptom: dictating via the floating button (mouse click, not Alt+D) sometimes injected text nowhere instead of the target app. First fix attempt (extend the `inject_text` Tauri command's window-hide/restore trick to the "toolbar" window too) had **zero effect** — because `streaming_enabled` defaults to `true`, and streaming's live per-segment injection lives in a completely separate call site (`streaming.rs::handle_message`) that bypassed the command entirely. Real fix: extracted the hide→wait→inject→restore logic into a shared `inject_text_defocused(app, text)` helper in `lib.rs`, used by **both** the command and the streaming path. Henry confirmed fixed in both modes.

3. **Per-item action-button row layout** — 5 buttons (inject/copy/TXT/Word/SRT) no longer fit one row; fixed with a deliberate 2-row grouping (quick actions / export formats). Henry explicitly rejected icon-only labels (confused users historically) and plain flex-wrap ("doesn't look intentional") — don't re-propose either without new information.

### Gotcha for next release

`npm run tauri build` needs **both** `TAURI_SIGNING_PRIVATE_KEY` **and** `TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""` exported — the key file is in `rsign`-encrypted format even with a blank password, and omitting the password var silently skips the updater `.sig` generation with **no error message** (the `.exe` still builds fine). Cost me one full rebuild cycle this round — set both from the start next time.

## Not done — flagged for future, NOT scheduled

- **English → Hebrew translation** (upload/dictate in English, auto-translate to Hebrew) — Henry-confirmed roadmap idea from 2026-07-02, not started.
- **Windows SmartScreen / code-signing certificate** — researched 2026-07-05: as of 2026, EV certs no longer bypass SmartScreen instantly (Microsoft closed that loophole in 2024) — OV and EV now both need the same reputation-building period. Azure Trusted/Artifact Signing ($9.99/mo, cheapest option) is **not available to Israel** (individual devs limited to US/Canada; orgs to US/Canada/EU/UK). Realistic path: a standard OV cert (~$200-400/yr, e.g. Sectigo) — replaces "Unknown Publisher" with a verified name and starts the reputation clock, but doesn't eliminate the warning immediately. Henry has not yet decided whether to purchase — ask before assuming this is wanted.

## 🔮 Backlog — Voicebox comparison (Henry, 2026-07-07): programmatic access

Henry compared hebrew-dictation to Voicebox and flagged three items to "add to the plans." Ordered by leverage — none scheduled yet:

**1. Local API — ✅ SHIPPED (2026-07-09), then materially reworked 2026-07-19 (see the top section).** `src-tauri/src/local_api.rs` — a `tiny_http` server on `127.0.0.1:5757`, **opt-in** (`local_api_enabled` in settings.json, off by default; port via `local_api_port`), bind failure non-fatal.

> ⚠️ **This bullet used to claim:** *"`GET /transcript` returns the last injected transcript… Verified wired: `inject_text_defocused` writes `last_transcript` after every successful injection (dictation and streaming)."* **That wiring was real but WRONG, and this sentence is what recorded the bug as verified-correct.** `streaming_enabled` defaults true and streaming injects **per segment**, so `/transcript` returned only the *last fragment* of a dictation. Fixed 2026-07-19: `GET /transcript` now returns `{text, seq, at}`, and stamping happens at the two **utterance boundaries** (the `inject_text` command; `stop_streaming_transcription`), never inside `inject_text_defocused`. Full detail in the top section.

Remaining polish: no UI toggle, no WebSocket live stream. (Unit tests: now 7 in `local_api.rs`.) **This unblocked #2 (the MCP wrapper) — now built.** Original research below —

**1-orig. Local API (REST/WebSocket on `127.0.0.1`) — highest leverage, the real gap.** The app is UI+hotkey only, no programmatic surface; Voicebox exposes `POST /generate` on `127.0.0.1:17493`. A small local server here (e.g. `GET /transcribe` → last transcript, or a WebSocket live stream) would let the **cloud-agent and the video pipeline consume dictation without driving the UI**. The transcription pipeline already exists (Deepgram/Groq/whisper-rs via `run_transcribe_file`) and the app already runs tokio → it's "wrap the existing pipeline in an embedded server." ⚠️ **Impl note:** `tauri-plugin-http` is an *outbound* client (a fetch replacement), **not** a server — to host an endpoint use embedded `axum`/`tiny_http` on a background task, not that plugin. **Design point first:** localhost still lets any local process — and browser pages via `fetch('http://127.0.0.1:…')` — reach it; gate anything that can trigger recording or return history behind a token (a read-only "last transcript" endpoint is low-risk). **Enabler for #2.**

**2. MCP server around the dictation — ✅ BUILT 2026-07-19 (see the top section).** Lives at `AI-Tools/MCP-Dev/hebrew-dictation-mcp/` — **its own git repo**, deliberately, so its commits never land in the shared `claude-dev` root repo the nightly job owns. Node/TS + `@modelcontextprotocol/sdk` ^1.29, stdio, three tools. The predicted shape was right: pull-based, no server→model push. NOT released/registered anywhere yet — see the top section for what's left.

*Original research:* Voicebox's MCP lets agents *speak* in the cloned voice; the inverse here gives Claude Code/Cursor voice *dictation into the agent session* (dictate-into-agent, not just into a text field). Best built as a thin wrapper over #1's local API rather than re-embedding the transcription logic — so **#1 first, then #2 is a small adapter.**

**3. LLM text-cleanup — ⚠️ ALREADY EXISTS, do NOT rebuild.** Henry's read was "the app injects raw STT with no punctuation/disfluency cleanup," and he rightly YAGNI-flagged it. Code check: it's **already shipped as "Smart Cleanup / רישוף חכם" (`enhance.rs`, spec `docs/superpowers/specs/2026-06-15-smart-cleanup-design.md`)** — runs the transcript through Groq Llama-3.3-70b to strip fillers (אהה/אמ/יעני/כאילו), repetitions and false-starts and fix Hebrew punctuation, with a hallucination guard (>2× raw length → reject; plus a truncation guard added 2026-07-19 — `finish_reason=="length"` → reject, see the top section) and graceful fallback to raw text on any error. It's **opt-in** (`enhance_enabled` setting), wired via the `enhance_text` command. The cloud batch path also already sends `smart_format=true&punctuate=true`, so cloud transcripts are already punctuated. → **Not an engineering gap.** The only real questions: (a) product — should Smart Cleanup be more discoverable / default-on? (b) does it cover the **streaming**-inject path or only batch? (streaming bypasses the command path per the v2.11.0 focus-bug fix — verify). Nothing to build unless Henry wants it always-on.

## Key facts (unchanged, still accurate)

- Signing: `~/.tauri/hebrew-dictation.key` — **encrypted format, needs `TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""` explicitly** (see gotcha above; supersedes any earlier "no password" note).
- Repo: `aihenryai/hebrew-dictation`. Website: `aihenryai/Henry-AI-website` (Cloudflare auto-deploy on push to main).
- Dev run for testing: `npm run tauri dev` (repo root) — Rust changes need the process restarted, not just HMR (HMR only covers the frontend).
- Kill orphaned dev processes: PowerShell `Get-CimInstance Win32_Process | ? CommandLine -match 'hebrew-dictation' | % { Stop-Process -Id $_.ProcessId -Force }`.
