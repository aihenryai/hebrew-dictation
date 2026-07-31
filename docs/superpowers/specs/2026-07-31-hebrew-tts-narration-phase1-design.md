# Hebrew TTS Narration — Phase 1 (Local, Generic Voice) — Design Spec

- **Date:** 2026-07-31 · **Revised same day** (rev 2 — architecture corrected after primary-source verification; see §2.1)
- **Status:** Draft (pending spec-review loop + user approval)
- **Author:** Claude (brainstormed with Henry; rev 2 reflection pass verified every load-bearing claim against primary sources)
- **Backlog origin:** `HANDOFF.md` backlog item #4 ("the actual Voicebox-shaped feature") and its research in `HANDOFF-TTS-VOICE-GENERATION.md`. That research picked VoxCPM2 as the best open-source Hebrew **voice-cloning** model, but VoxCPM2 hard-requires NVIDIA GPU + CUDA + Python/PyTorch — it cannot be the zero-friction "local" tier that `whisper-rs` is for STT. This spec fills that gap with a generic-voice local tier; cloning is Phase 2.

---

## 1. Problem & goals

Hebrew-dictation is voice→text. Henry wants the inverse: text→voice, in Hebrew, wrapped for non-technical users the same way this app wraps `whisper-rs` behind Alt+D — nothing for the user to install or configure beyond clicking "download" inside the app.

**Goal (Phase 1):** a local, CPU-only, generic-Hebrew-voice narration feature inside hebrew-dictation. After a one-time in-app download, it works fully offline on any Windows machine, no GPU.

**Success scenario:** Henry types or pastes a paragraph of Hebrew text into a new "הקראה" screen, clicks a button, and within a few seconds hears it read aloud in a natural Hebrew voice — entirely offline, no cloud call, no GPU.

### The guiding principle, stated precisely

The app's established rule is **"the user never installs or configures anything technical"** — not "no Python may exist anywhere on disk." `whisper-rs` satisfied it by being pure Rust; Phase 1 satisfies it with an **app-managed, isolated, on-demand-downloaded runtime** (§3) that the user experiences exactly like downloading a whisper model. The NSIS installer stays small; the user's system Python (or lack of one) is irrelevant; nothing touches PATH or the registry.

### Non-goals (out of scope for Phase 1 — deferred to a separate future Phase 2 spec)

- **Voice cloning** (narrating in Henry's own cloned voice). Needs VoxCPM2 → CUDA → RunPod/GPU decisions that are **blocked on Henry's own RunPod account/billing setup**. Different model, different infrastructure; do not conflate.
- **Voice selection UI.** Factually moot in v1: exactly **one** Hebrew Piper voice exists today (§2.2) — there is nothing to select between.
- **Long-text chunking / streaming playback.** Piper is a local synchronous engine, not a rate-limited API — long input is slower, not blocked. Deferred until real usage demands it (YAGNI).
- Any cloud/remote backend. Phase 1 is 100% local after the one-time download.

---

## 2. Model & runtime decision (rev 2 — corrected)

### 2.1 What rev 1 got wrong, and how we know

Rev 1 planned to spawn the **archived** `rhasspy/piper` compiled Windows binary (`piper.exe`, 2023, MIT) as a sidecar. Primary-source verification killed that plan:

- The only Hebrew Piper voice — `he_IL-saspeech-medium`, added to `rhasspy/piper-voices` days before this spec ("Add ko, he, mr voices", by the Piper author) — declares **`"phoneme_type": "hebrew"`** in its config (verified by fetching the raw `.onnx.json`).
- That phoneme type does not exist in the archived 2023 binary (which knows only `espeak`/`text`). It was introduced in **piper1-gpl v1.6.0 (2026-07-23): "Add Hebrew phonemizer using Nakdimon"** — a neural Hebrew diacritizer that adds nikud before phonemization. Unvocalized Hebrew is exactly why Piper had no Hebrew voice for years (rhasspy/piper issue #538, open since 2024): plain espeak-ng phonemization of nikud-less text is unusable.
- `piper1-gpl` (the active successor, Open Home Foundation) is **GPL-3.0 and ships as a Python package only** (`pip install piper-tts`) — no standalone `.exe` releases.

**Conclusion:** for Hebrew, "compiled-binary Piper sidecar" is not an option. The Hebrew capability *is* the Python package. The architecture must embrace that without violating the guiding principle above.

### 2.2 The voice

`he_IL-saspeech-medium` — 63MB ONNX, 22.05kHz, single male speaker, `medium` quality only. Trained on the **SASPEECH corpus** (the Roboshaul project: Shaul Amsterdamski's voice, from Kan's "חיות כיס" podcast recordings). Hosted in `rhasspy/piper-voices` (repo license MIT). ⚠️ The spike (§6) must check the voice's own MODEL_CARD for license/attribution terms — a corpus built from one real person's broadcast voice may carry attribution or non-commercial conditions that belong in the app's credits screen.

### 2.3 The runtime: piper1-gpl as a managed HTTP sidecar

`piper1-gpl` includes a built-in HTTP server:

```
python -m piper.http_server -m he_IL-saspeech-medium --host 127.0.0.1 --port <port>
POST /synthesize  {"text": "..."}  →  WAV bytes
GET  /info                          →  voice metadata (doubles as health check)
```

The app spawns this once as a warm child process and calls it over localhost HTTP. This beats per-request CLI spawning decisively: the voice model + Nakdimon load **once** at server start (cold-start cost paid once), every subsequent request is fast; WAV arrives as an HTTP body, eliminating rev 1's Windows stdin/stdout binary-piping risk entirely; and concurrent requests are the server's problem, not ours.

### 2.4 Provisioning: `uv` does everything

One-time in-app "download narration engine" flow, driven by Rust:

1. Download **`uv`** (astral-sh — a single static ~20MB binary, MIT/Apache) into app-data, **hash-verified** like every existing model download in `model.rs`.
2. `uv` provisions an isolated Python + venv inside app-data and installs `piper-tts` (≥1.6.0; the spike pins whether the HTTP server needs an extra like `piper-tts[http]`).
3. Download the voice (63MB) via the existing model-download UI patterns; confirm where the Nakdimon model comes from (bundled in the wheel vs. fetched on first use — spike question; if fetched, it must be part of this provisioning step so the feature is truly offline afterwards).

Everything lives under app-data, fully removable, invisible to the system. Disk estimate ~200-300MB total (onnxruntime wheel + venv + voice + Nakdimon) — spike confirms the real number, and the download UI must show it honestly like the whisper model sizes.

**Provisioned-state detection & partial failure (normative):** "provisioned" is defined by an **atomic completion marker** (a small versioned JSON, e.g. `narration-engine.json` recording piper-tts version + voice + paths) written **last**, only after every step above succeeded. Any failure or interruption mid-provisioning (install died halfway, app closed during a download) leaves the marker absent → the state machine stays at *not-provisioned* and the UI simply offers "download" again. Every step is **idempotent and safe to re-run**: `uv` recreates/repairs the venv, the voice download resumes/overwrites per the existing `model.rs` semantics, and stale partial directories are overwritten rather than trusted. No separate "repair" flow — retry *is* the repair.

**Version pinning:** the implementation plan pins an **exact** `piper-tts` version (e.g. `==1.6.x` as verified by the spike), not open-ended `>=` — the HTTP API verified today shouldn't silently drift under a future release. Where practical, the pip install uses `uv`'s hash-pinned requirements for parity with the hash-verified `uv` binary itself.

### 2.5 Licensing

- **GPL-3.0 (piper1-gpl):** fine. It runs as a **separate process** — mere aggregation, nothing GPL links into the app binary. Moreover the app doesn't redistribute it at all: components download from upstream (PyPI/GitHub/HuggingFace) at the user's request.
- **uv:** MIT/Apache. **Voice repo:** MIT (individual voice card checked in spike, §2.2).

### 2.6 Alternatives considered and rejected

| Alternative | Why rejected |
|---|---|
| **Archived `piper.exe` (rev 1's plan)** | Cannot run `phoneme_type: "hebrew"` — predates the Hebrew phonemizer entirely (§2.1). Feeding it unvocalized Hebrew through old espeak-ng is the exact failure mode that kept Hebrew out of Piper for years. |
| **MMS-TTS Hebrew** (`facebook/mms-tts-heb`) via `ort` crate in-process | Community verdict in rhasspy/piper issue #538: quality "not good." Raw research checkpoint, hand-rolled tokenization in Rust, no diacritization story at all. More work for worse output. |
| **Re-implement Hebrew phonemization in Rust** (Nakdimon via `ort` + phoneme mapping) | Research-grade effort duplicating what piper1-gpl just shipped; unjustifiable for v1. |
| **Bundling Python/runtime in the NSIS installer** | Balloons the installer for a feature not every user wants. On-demand provisioning (§2.4) gives the same end state, paid only by users who opt in. |
| **Cloud TTS for Phase 1** | Violates the local/offline premise; cloud belongs to Phase 2's cloning discussion. |

---

## 3. Architecture

```
┌──────────────────────────────────────────────────────────────┐
│ hebrew-dictation app (Rust, Tauri)                            │
│                                                                │
│  narration.rs (NEW)                                           │
│   - NarrationServer: spawn/health-check/kill the sidecar      │
│   - synthesize(text) -> Result<Vec<u8>, Err>   POST /synthesize│
│   - pure fns: server args builder, WAV sanity check           │
│                                                                │
│  provisioning (model.rs EXTENDED or narration_setup.rs)       │
│   - uv download (hash-verified) → venv → piper-tts → voice    │
│   - progress events reusing the whisper-model download UI     │
│                                                                │
│  lib.rs (NEW commands)                                        │
│   - generate_narration(text) -> Vec<u8> (WAV)                 │
│   - ensure_narration_ready() – pre-flight guard               │
│   - narration_setup() – runs provisioning with progress       │
└──────────────────┬─────────────────────────────────────────────┘
                    │ spawns & owns (kill_on_drop / on app exit)
┌──────────────────▼─────────────────────────────────────────────┐
│ python -m piper.http_server (127.0.0.1:<port>, warm sidecar)   │
│  loads he_IL-saspeech-medium + Nakdimon once                    │
└────────────────────────────────────────────────────────────────┘
                    ▲ Tauri IPC (frontend)
┌──────────────────┴─────────────────────────────────────────────┐
│ React — NEW "הקראה" AppView                                     │
│  textarea → "צור קול" → <audio> playback → "שמור כ-WAV"          │
└────────────────────────────────────────────────────────────────┘
```

- **`narration.rs`** owns the sidecar lifecycle: spawn on first use (or on entering the הקראה screen — plan decides), `GET /info` as readiness probe, restart-once-on-crash, and **guaranteed teardown**, layered because `kill_on_drop` alone only covers clean exit: (a) `tokio::process` `kill_on_drop` + explicit kill on app exit for the normal path; (b) the child is assigned to a **Windows Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`**, so a hard app crash / `taskkill` takes the sidecar down too; (c) a **startup stale-sidecar sweep** — on launch, probe the configured port and kill/reclaim any leftover sidecar from a previous unclean death. This is the one genuinely new lifecycle obligation vs. rev 1 and it must be integration-tested. Spawn timing is decided (§4): entering the narration screen spawns the sidecar — no open "plan decides" question.
- **Port:** default `5758` (sibling of the local API's `5757`), configurable in settings (`narration_port`); bind failure → clear Hebrew error, and it's internal-only (`127.0.0.1`). ⚠️ **`--host 127.0.0.1` is security-mandatory, not stylistic** — verified: piper1-gpl's HTTP server defaults to `0.0.0.0` (all interfaces); omitting the flag would expose a synthesis endpoint to the local network. The args-builder unit test (§7) asserts its presence.
- **Idle policy — known accepted tradeoff:** the sidecar stays warm from screen-entry until app exit, inside an app that itself runs in the background all day. A Python + onnxruntime + Nakdimon process plausibly holds a few hundred MB of RAM while warm; keeping it resident is the deliberate v1 choice (cold-start per request would be far worse UX). The spike (§6) measures the real RSS; an idle-timeout shutdown is a possible follow-up only if the measured number is offensive.
- **Pure seams for TDD** (pattern of `enhance.rs::parse_completion`): the server-args builder and a WAV header sanity check are pure functions; HTTP-client logic is testable against a mock localhost server exactly like `local_api.rs`'s tests.
- **Frontend:** new `AppView` "הקראה" reachable from the home screen like batch/settings; textarea, generate button, inline `<audio>`, and "שמור כ-WAV" reusing the `export_history`/`export_srt` save-dialog pattern. If the engine isn't provisioned, the screen shows the download flow (reusing the model-download UI), not an error.

---

## 4. Data flow

1. User opens "הקראה". If the engine isn't provisioned → in-place download flow (§2.4). If provisioned but the sidecar isn't running → spawn it, show a brief "מכין את מנוע ההקראה…" state until `/info` responds.
2. User types/pastes Hebrew text, clicks "צור קול" → `generate_narration(text)`.
3. Rust `POST /synthesize {"text": ...}` → receives WAV bytes → sanity-checks the header → returns them over IPC.
4. Frontend plays inline and offers "שמור כ-WAV".

---

## 5. Error handling

- **Engine not provisioned:** pre-flight guard (`ensure_narration_ready`) returns an actionable Hebrew state the UI renders as the download flow — never a raw spawn failure.
- **Sidecar dead / port conflict / `/synthesize` non-200:** restart once; if still failing, a clear Hebrew error. Never return partial/corrupt audio — all-or-nothing per request (valid WAV or error).
- **Empty/whitespace input:** button disabled client-side (matches `record_utterance`'s blank-guard philosophy).
- **Very long input:** no artificial limit. Known accepted tradeoff: WAV is uncompressed and crosses IPC as one `Vec<u8>`; a huge paste = larger buffer + longer wait. Fine for the §1 "paragraph" scenario; revisit only on real demand.
- **Concurrent clicks:** requests serialize naturally at the single warm server; no app-side lock needed (unlike the mic's hardware-contention locks). Deliberate non-issue.
- **App exit during synthesis:** sidecar is killed with the app; in-flight request dies with it — acceptable, nothing persistent is corrupted.

---

## 6. Rollout order — spike FIRST, before any app code

The Hebrew voice is days old and its quality is **unverified**. Also several provisioning facts are unpinned. So:

1. **Quality + facts spike (throwaway dev venv, zero app code):**
   - `pip install piper-tts` (pin ≥1.6.0; note whether the HTTP server needs an extra), download `he_IL-saspeech-medium`, synthesize test sentences — including a multi-clause sentence and the known Hebrew-TTS trap word **"מתגים"** (documented in `HANDOFF-TTS-VOICE-GENERATION.md` as failing on *every* model tested; expected to fail here too — record it, don't chase it).
   - Include one **mixed-content sentence** (Hebrew + English brand name + digits, e.g. "הורדתי את ChatGPT 5 אתמול") — real narration text contains these, and the Nakdimon/espeak path's behavior on non-Hebrew tokens is unknown.
   - Judge **objectively**: Whisper ASR round-trip (transcribe the output, compare to input) — the established methodology; never "sounds fine to me," and never an LLM's self-report.
   - Pin the facts: where Nakdimon's model comes from (wheel vs. first-use download); real total disk size; server cold-start seconds; per-request latency for a paragraph; **warm-idle RAM (RSS) of the sidecar** (§3's idle-policy tradeoff); the voice MODEL_CARD's license/attribution terms; exact `/synthesize` behavior for long text; the exact `piper-tts` version to pin (§2.4).
2. **If the spike passes:** build the full chain — provisioning, `narration.rs`, commands, the הקראה screen — via the normal plan/TDD flow.
3. **If quality fails:** stop and report back with the recordings + ASR transcripts. Do **not** silently fall back to MMS-TTS or anything else — every alternative was rejected for cause (§2.6), so a Piper failure means a fresh decision with Henry, possibly "wait for the ecosystem" or "jump straight to Phase 2 cloud."
4. **Manual verify with Henry** before Phase 1 is called done: a real paragraph, on his machine, his ears. (An ASR round-trip proves intelligibility, not pleasantness — the human check is for naturalness/pace, and it's the same gate every feature in this app ships through.)

---

## 7. Testing

- **Rust unit tests (pure):** server-args builder; WAV header sanity check; provisioning state machine decisions (not-provisioned / provisioned-not-running / ready).
- **Rust tests (mock HTTP):** `synthesize()` against a mock localhost server — success, non-200, connection-refused, garbage-body cases. Same technique as `local_api.rs`'s existing tests.
- **Integration test (real sidecar), `#[ignore]`-gated** like the loopback-audio test: runs only where the engine is provisioned; spawns, probes `/info`, synthesizes one word, checks WAV validity, kills, **verifies the process is actually gone** (the orphan-prevention obligation from §3).
- **No automated Hebrew-quality regression test** — quality is the §6 spike gate + Henry's manual verify, not a unit test.

---

## 8. Acceptance criteria

- Spike (§6.1) passes the ASR round-trip on real Hebrew sentences, and all spike facts (disk size, Nakdimon source, cold-start, license terms) are written into the implementation plan before coding starts.
- `generate_narration` returns valid complete WAV for non-trivial Hebrew input; `cargo test` green; `cargo build`/`clippy` clean.
- Provisioning is fully in-app (no user-visible Python/pip/terminal anywhere), hash-verifies the `uv` binary, and shows honest download sizes.
- No orphaned sidecar: clean exit kills it (`kill_on_drop` path, integration-tested); hard crash kills it via the Job Object; and the startup sweep reclaims any survivor from an unclean death — all three layers of §3, not assumed.
- Missing-engine states surface as the in-app download flow, never as raw errors.
- Henry manually verifies real output on his machine before this ships.

---

## 9. Explicitly out of scope / backlog for later

- **Phase 2 (separate spec):** voice cloning via VoxCPM2 — backend architecture (self-hosted RunPod GPU pod vs. interim public-demo API), blocked on Henry's RunPod account/billing. All VoxCPM2 tuning knowledge is preserved in `HANDOFF-TTS-VOICE-GENERATION.md`.
- Voice selection (moot until a second Hebrew voice exists) · chunking/streaming · bundling anything into the installer.
- A Codex second-opinion consult was attempted during rev 1 (workspace out of credits, no answer). Rev 2's primary-source verification pass answered the questions it was meant to probe (espeak-ng bundling, licensing, Hebrew phonemization maturity) with better evidence than an opinion; retrying Codex is optional, not a blocker.
