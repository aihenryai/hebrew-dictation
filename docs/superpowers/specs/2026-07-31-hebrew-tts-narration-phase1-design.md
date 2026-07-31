# Hebrew TTS Narration — Phase 1 (Local, Generic Voice) — Design Spec

- **Date:** 2026-07-31
- **Status:** Draft (pending spec-review loop + user approval)
- **Author:** Claude (brainstormed with Henry)
- **Backlog origin:** `HANDOFF.md` backlog item #4 ("the actual Voicebox-shaped feature") and its full background research in `HANDOFF-TTS-VOICE-GENERATION.md` (same repo). That research picked VoxCPM2 as the best open-source Hebrew **voice-cloning** model, but flagged it needs a real NVIDIA GPU + CUDA + Python/PyTorch — too heavy to be a zero-friction "local" tier the way `whisper-rs` is for STT. This spec is the brainstorm that resolves that gap.

---

## 1. Problem & goals

Hebrew-dictation is voice→text. Henry wants the inverse: text→voice, in Hebrew, wrapped for non-technical users the same way this app wraps `whisper-rs` behind Alt+D — no Python, no pip install, no GPU shopping list.

The app already has a working precedent for "local" in the STT direction: `whisper-rs` (pure Rust bindings to whisper.cpp) ships as a downloadable model file, runs on any CPU, zero Python/CUDA dependency. VoxCPM2 — the best *voice-cloning* model researched — cannot meet that bar; it hard-requires CUDA.

**Goal (this spec, Phase 1):** a local, CPU-only, generic-Hebrew-voice narration feature inside hebrew-dictation, matching whisper-rs's zero-friction local UX: download a model, no GPU required, works on any Windows machine.

**Success scenario:** Henry types or pastes a paragraph of Hebrew text into a new "הקראה" screen, clicks a button, and within a few seconds hears it read aloud in a generic Hebrew voice — entirely offline, no cloud call, no GPU.

### Non-goals (out of scope for Phase 1 — deferred to a separate future Phase 2 spec)

- **Voice cloning** (narrating in Henry's own cloned voice). This needs VoxCPM2, which needs CUDA. Phase 2 will decide standalone-vs-embedded backend questions (RunPod self-hosted GPU pod vs. an interim public-demo API) — **blocked today on Henry's own RunPod account/billing setup**, not something to do on his behalf. Do not conflate Phase 1's generic voice with Phase 2's cloning; they are different models with different infrastructure needs.
- Voice selection UI beyond a single default Hebrew voice (Piper may ship more than one; picking/exposing multiple is a later nice-to-have, not required for v1).
- Long-text chunking/streaming playback. No LLM-style token quota applies here (Piper is a local synchronous process, not a rate-limited API) — a long input is just slower, not blocked. Chunking is deferred until real usage shows it's needed (YAGNI).
- Any cloud/remote backend for Phase 1. Phase 1 is 100% local by design.

---

## 2. Decision: Piper TTS as a sidecar process

**Chosen:** [Piper](https://github.com/rhasspy/piper) (rhasspy), invoked as a compiled sidecar binary — Rust spawns `piper.exe`, feeds Hebrew text on stdin, reads WAV bytes back on stdout.

**Why:**
- Piper ships as a small compiled binary with no Python/PyTorch runtime dependency — the closest architectural match to `whisper-rs`'s "download a model file, run on CPU" UX that already exists in this app.
- VITS architecture, ONNX Runtime bundled internally; voices are separate downloadable `.onnx` files — fits the existing model-download flow (`model.rs`) almost exactly.
- Hebrew voice models were added to the official `piper-voices` repo (HuggingFace/GitHub) within days of this spec being written (July 2026) — Hebrew support exists today, not hypothetically.
- This repo already has a proven precedent for the "spawn a compiled sidecar binary" pattern: the earlier on-device Hebrew-cleanup research (see `HANDOFF.md`) chose to run llama.cpp as a sidecar specifically to avoid a `ggml` static-link collision with whisper.cpp in the same binary. Piper-as-sidecar follows the same shape, for the same category of reason (keep unrelated native dependencies out of the main binary).

### Alternatives considered and rejected

| Alternative | Why rejected |
|---|---|
| **MMS-TTS Hebrew** (`facebook/mms-tts-heb`) via embedded ONNX Runtime (`ort` Rust crate), in-process | Also VITS-based, so no clear quality edge over Piper — but it's a raw HuggingFace research checkpoint, not a purpose-built app-ready engine. Would need hand-rolled tokenizer/preprocessing in Rust or a separate export step. More integration work for no clear benefit. |
| **Embedded Python runtime** (coqui-tts, or Piper's own Python package) | Rejected on principle — avoiding embedded Python for local inference is the established pattern in this codebase (`whisper-rs` was chosen specifically to avoid it for STT). Would balloon installer size and packaging complexity, undermining the entire "friendly for non-technical users" premise. |

### Open risk, explicitly not resolved by this spec

Piper's Hebrew voices are brand new (added days before this spec). Quality is **unverified** — nobody has listened to or objectively measured Piper's Hebrew output yet. Piper depends on `espeak-ng` for phonemization, and `espeak-ng`'s historical Hebrew support maturity is unknown to us. **It is also not yet confirmed whether `espeak-ng` ships statically bundled inside the Piper binary or is a separate runtime dependency** — if the latter, `ensure_piper_model_available()` (§3) needs to check for it too, or a user could pass the pre-flight guard and still fail at spawn. **§6 (rollout order) makes resolving both of these the first build step, before any UI work** — see there.

(A second-opinion consult with Codex was attempted during this brainstorm to sanity-check the architecture and probe these specific risk questions; the Codex workspace was out of credits and returned no answer. This spec proceeds on Claude's own analysis; the Codex consult can be retried later once credits are refilled, but is not a blocker for starting the quality spike in §6.)

---

## 3. Architecture

```
┌────────────────────────────────────────────┐
│ hebrew-dictation app (Rust, Tauri)          │
│                                              │
│  narration.rs (NEW)                         │
│   - build_piper_command(text, model_path)   │  pure, unit-testable
│   - run_piper(text) -> Result<Vec<u8>,Err>  │  spawns piper.exe, stdin/stdout
│                                              │
│  model.rs (EXTENDED)                        │
│   - piper binary + voice .onnx download,    │  mirrors existing whisper
│     mirrors existing model download flow    │  model download UX
│                                              │
│  lib.rs (NEW commands)                      │
│   - generate_narration(text) -> Vec<u8>     │
│   - ensure_piper_model_available()          │  pre-flight guard
└──────────────────┬───────────────────────────┘
                    │ Tauri IPC
┌──────────────────▼───────────────────────────┐
│ Frontend (React) — NEW "הקראה" AppView        │
│  textarea -> "צור קול" button -> <audio>      │
│  -> "שמור כ-WAV" (reuses export save-dialog)  │
└────────────────────────────────────────────────┘
```

- **`narration.rs`** (new module, alongside `local_api.rs`, `enhance.rs`): owns the Piper process invocation. `build_piper_command` is a pure function (text + model paths → `Command` args) so the "did we build the right invocation" logic is unit-testable without actually spawning a process — same seam pattern as `enhance.rs`'s `parse_completion`.
- **`model.rs`** extension: the `piper.exe` binary and the Hebrew voice `.onnx` file are downloaded on demand, **not bundled in the NSIS installer** — same reasoning as the existing whisper models (keeps installer size down, matches the "download a model" UX users already know from this app). This inherits `model.rs`'s existing expected-size + SHA-256 verification on every download — **not optional for `piper.exe` specifically**, since (unlike an inert model weights file) it is an executable that gets spawned as a subprocess, making an unverified download a higher-severity risk than for the whisper models.
- **New Tauri commands:**
  - `generate_narration(text: String) -> Result<Vec<u8>, String>` — returns raw WAV bytes.
  - `ensure_piper_model_available() -> Result<(), String>` — pre-flight guard, mirrors the existing `ensure_local_meeting_model_available` pattern (checked *before* spawning Piper, not discovered via a failed spawn).
- **Frontend:** a new `AppView` ("הקראה"), reachable from the home screen the same way the existing batch/settings screens are. Textarea for input Hebrew text, a generate button, inline `<audio>` playback, and a "שמור כ-WAV" export button reusing the existing `tauri-plugin-dialog` save pattern already used by `export_history`/`export_srt`.

---

## 4. Data flow

1. User opens the "הקראה" screen from the home view.
2. Types or pastes Hebrew text into a textarea.
3. Clicks "צור קול" → frontend invokes `generate_narration(text)`.
4. Backend: `ensure_piper_model_available()` checks the binary + voice model are present. If not, returns an actionable Hebrew error (mirrors the whisper-model-missing guard) — **checked before spawning**, not surfaced as a cryptic process-spawn failure.
5. Rust spawns `piper.exe` via `tokio::process::Command`, writes the Hebrew text to stdin as UTF-8, reads WAV bytes from stdout to completion.
6. Bytes return to the frontend over Tauri IPC; the UI plays them inline (`<audio>`) and shows a "שמור כ-WAV" button.

---

## 5. Error handling

- **Model/binary not installed:** pre-flight guard (§3) returns a clear Hebrew error pointing at Settings to download, before any process is spawned.
- **Piper process fails to spawn, crashes, or exits non-zero:** surfaced as a Hebrew error; no partial/corrupt audio is ever returned to the frontend (all-or-nothing: either a full valid WAV or an error, never a truncated buffer).
- **Empty or whitespace-only input:** the "צור קול" button is disabled client-side — no backend round-trip for a no-op request (matches the empty-guard pattern already used elsewhere in this app, e.g. `record_utterance`'s blank-text guard).
- **Very long input text:** no artificial length limit in Phase 1. This is not an LLM API with a token quota — Piper is a local synchronous process, so a long input just takes longer, it isn't blocked or billed. If real usage later shows a need for chunking or a progress indicator, that's a follow-up, not a Phase-1 requirement. Known tradeoff, accepted deliberately: WAV is uncompressed and returned as a single `Vec<u8>` over Tauri IPC, so a genuinely long paste means a larger in-memory buffer and a multi-second UI hitch — acceptable for the "a paragraph" success scenario in §1, revisit only if real usage pushes past that.
- **Concurrent generation requests** (e.g. double-clicking "צור קול"): each `piper.exe` spawn is stateless and independent, unlike the mic recorder's hardware-contention lock — two overlapping generations are a deliberate non-issue, not a gap.

---

## 6. Rollout order — quality spike BEFORE UI work

Because Piper's Hebrew voice is unverified and brand-new (§2), building the full UI/backend integration before confirming the voice is usable risks throwing away work. Build order:

1. **Quality spike (do this first, no UI, no Tauri integration):** download the Piper binary + a Hebrew `.onnx` voice manually, run one test sentence, and judge the output **objectively** — an ASR round-trip check (transcribe the generated audio back with Whisper, compare to the input text), exactly the methodology `HANDOFF-TTS-VOICE-GENERATION.md` already established and validated for VoxCPM2 ("don't trust a model's own self-report on Hebrew audio quality — verify objectively"). Do not proceed past this step on the strength of a human just listening and going "sounds fine." **This spike also settles the open §2 question:** does the Windows Piper release bundle `espeak-ng` statically, or is it a separate dependency that `ensure_piper_model_available()` needs to check for too? Confirm and write the answer into the plan before implementation starts.
2. **If the spike passes:** build the full chain — `narration.rs`, the `model.rs` extension, the two Tauri commands, the "הקראה" screen.
3. **If the spike fails (quality too poor to ship):** stop and report back before writing any app code. Do not silently fall back to MMS-TTS or another model without a fresh decision — a Piper-shaped failure doesn't automatically make MMS-TTS's tokenizer/ONNX integration effort worth it; that trade-off needs to be re-evaluated with the actual failure in hand.
4. **Manual verify with Henry** (can't be automated): generate a real paragraph of Hebrew text on his machine, listen, confirm it's usable for the intended use case, before considering Phase 1 done.

---

## 7. Testing

- **Rust unit tests:** `build_piper_command` (pure — args are correct for a given text/model-path combination, no process spawned), and a WAV-header sanity check on returned bytes (structurally valid WAV, not garbage/empty).
- **Integration test (real process spawn):** gated/skipped when the Piper binary isn't present on the test machine — same `#[ignore]`-style pattern already used for the loopback-audio test in this repo, not a hard CI dependency on a downloaded binary.
- **No automated Hebrew-quality test** — that's inherently the ASR-round-trip spike in §6, done once as a build-order gate, not a repeatable test suite (Hebrew TTS quality isn't a thing to regression-test the way "does the WAV parse" is).

---

## 8. Acceptance criteria

- Quality spike (§6.1) passes an objective ASR round-trip check on a real Hebrew test sentence before any app code is written.
- `generate_narration` returns valid, complete WAV bytes for non-trivial Hebrew input; `cargo test` green; `cargo build`/`cargo clippy` clean.
- Missing-model case surfaced as a clear pre-flight Hebrew error, never a raw process-spawn failure.
- Henry manually verifies real output on his own machine before this is considered shippable (no automated substitute for this step).

---

## 9. Explicitly out of scope / backlog for later

- Phase 2 (separate spec, not started): voice cloning via VoxCPM2, backend architecture (self-hosted RunPod GPU pod vs. an interim public-demo API), blocked on Henry's own RunPod account/billing.
- Voice selection among multiple Piper Hebrew voices, if more than one exists.
- Any packaging of Piper/models into the installer itself (stays download-on-demand, matching whisper).
- A retried Codex second-opinion consult (attempted during this brainstorm, failed on out-of-credits — not a blocker, just unfinished).
