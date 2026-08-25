# Hebrew Voice Generation — Handoff for a New Extension

> **Separate from `HANDOFF.md`** (which tracks the shipped STT/dictation app) on purpose — this is
> research for a **new, not-yet-started** feature: a TTS/voice-generation wrapper, in the spirit of
> the "Voicebox comparison" backlog item already flagged in `HANDOFF.md`'s bottom section (Voicebox
> exposes `POST /generate` on `127.0.0.1:17493` — same shape of idea, opposite direction: hebrew-dictation
> is voice→text, this would be text→voice).
>
> **Origin:** this research happened as a side branch of an unrelated session (building an AI avatar
> video pipeline, `Video-Production/one-off/avatar-poc-fal/`) where we needed to clone Henry's real
> voice for Hebrew narration and tested every viable option along the way. Nothing here is
> implemented yet — no brainstorm, no spec, no code. Read this, then run a proper
> `superpowers:brainstorming` pass before writing anything.

## The product idea (Henry's framing)

An open-source Hebrew-capable voice-generation model exists and genuinely works well — but it's
realistically unusable by a non-technical person (Python, GPU, `pip install`, Gradio/API plumbing).
Henry wants to wrap it in a friendly local app/UI, the same way `hebrew-dictation` wraps
Deepgram/whisper-rs behind `Alt+D`. Possibly ships as a feature *inside* hebrew-dictation (natural
pairing: dictate-in / narrate-out), possibly a standalone sibling app — that product question is
open and belongs in the brainstorm, not decided here.

## The winner: VoxCPM2 (OpenBMB)

Out of everything tested **that is both open-source AND actually good at Hebrew**, there is exactly
one answer.

- Repo: [OpenBMB/VoxCPM](https://github.com/OpenBMB/VoxCPM) · model: [openbmb/VoxCPM2](https://huggingface.co/openbmb/VoxCPM2)
- 2B params, tokenizer-free TTS, trained on 2M+ hours multilingual audio, 30 languages incl. Hebrew
- License allows self-hosting; `pip install voxcpm` (Python ≥3.10, PyTorch ≥2.5, CUDA ≥12.0)
- Public test surface used so far: the official Gradio demo space
  [openbmb/VoxCPM-Demo](https://huggingface.co/spaces/openbmb/VoxCPM-Demo), driven headlessly via
  `gradio_client` — no signup needed, but it's a shared public queue, **not** production-grade
  (rate limits, availability not guaranteed, 50-second hard cap on reference audio).

### Working API shape (public demo, `gradio_client`)

```python
from gradio_client import Client, handle_file
client = Client("openbmb/VoxCPM-Demo")
result = client.predict(
    text_input,               # the Hebrew text to speak
    control_instruction,      # free-text style/gender/pace steering — see below, this matters a lot
    handle_file(ref_wav_path),# reference clip, MUST be ≤50s or the call throws AppError
    False,                    # use_prompt_text (leave False — ASR-derived prompt text was unreliable for Hebrew, see below)
    "",                       # prompt_text_input
    3.0,                      # cfg_value — higher = stronger adherence to reference + control_instruction
    True,                     # do_normalize
    True,                     # denoise
    api_name="/generate",
)
```

There's also an ASR helper endpoint (`/_run_asr_if_needed`) meant to auto-transcribe the reference
clip for `prompt_text_input` — **tried it on a Hebrew clip, it returned garbage (`"."`)**. Don't rely
on it for Hebrew; leave `use_prompt_text=False`.

### The tuning that actually mattered (measured, not guessed)

1. **`control_instruction` is the single highest-leverage knob.** The untuned first pass sounded
   too fast and gender-ambiguous. Adding an explicit instruction fixed both:
   `"Adult male voice, natural Israeli Hebrew accent, calm and steady speaking pace, not rushed, clear articulation"`
   — Henry confirmed this fixed the pacing/gender complaint directly.
2. **`cfg_value=3.0`** (up from the 2.0 default) gave stronger adherence to both the reference voice
   and the control instruction.
3. Reference clip length: tested both 20s and 45s (max allowed) from the same source — **no
   measurable difference** in output quality between the two. Don't over-invest in sourcing a long
   reference for this specific model; 20-30s of clean audio is enough.

### Known remaining defect

One word — **"מתגים" (switches/toggles) consistently comes out as "מדגים" (demonstrates)** — across
*every* model tested this session, not just VoxCPM2 (also failed identically on MiniMax, both old and
new generation). Strong signal this is a hard consonant cluster for Hebrew TTS generally, not a
VoxCPM2-specific bug. Don't burn time trying to fix it model-side; if it matters for a real script,
rephrase around it.

## Everything else that was tried (so the next session doesn't re-run these)

All verified with **objective Whisper transcription** (`whisper-1`, `language=he`) against the exact
same Hebrew test sentence, never by trusting a model's own self-reported quality — see the
"meta-lesson" below, it matters.

| Model | Open source? | Hebrew result | Verdict |
|---|---|---|---|
| **VoxCPM2** | ✅ | Near-perfect after tuning | **Winner** — see above |
| Chatterbox Multilingual (Resemble AI) | ✅ | Catastrophic — literal gibberish despite the docs claiming Hebrew support | Do not use for Hebrew, full stop |
| RVC (Retrieval-based Voice Conversion, via Replicate) | ✅ (technique) | Degraded an already-correct transcript further (new errors introduced) | Wrong tool for this: it's voice *conversion*, and its content encoder (HuBERT) isn't Hebrew-robust. Interesting technique, not useful here |
| MiniMax Speech-02-hd (old gen, via fal) | ❌ commercial | Garbled key words ("ומעכשיו"→"ומחשב", "שומרת"→"שומעת") | Superseded by Speech-2.8 below |
| MiniMax Speech-2.8-hd (new gen, via fal) | ❌ commercial | 100% accurate + good similarity after tuning | Best commercial option for Hebrew, but **not open source** so out of scope for this feature |
| Deepdub (Israeli, dubbing-focused) | ❌ commercial | 100% word-perfect | Excellent accuracy, but Henry judged the voice similarity itself as "too basic" — also not open source |
| ElevenLabs Voice-1 / v3 | ❌ commercial | Fine (existing pipeline default) | Not a clone, not open source, not relevant here |
| ElevenLabs Professional Voice Clone (PVC) | ❌ commercial | **Structurally impossible** | Confirmed via their own help-center language list: Hebrew is not a supported PVC language at all. The API silently accepts `language: "he"` at voice-creation time with no validation error, but the training backend never actually processes it — so it just sits at `not_started` forever with no error surfaced anywhere. Not a bug to work around; don't attempt PVC + Hebrew again |

### The meta-lesson worth carrying into this feature's design

**Don't trust a model's own self-report on Hebrew audio quality — verify objectively.** Gemini 2.5
Pro confidently rated one clone "10/10 flawless pronunciation" when an objective Whisper transcription
proved multiple words were flat wrong. No frontier model has a real measured benchmark for Hebrew (see
[[prompting-evidence-2026]] in the global memory) — this applies as much to judging *TTS output* as to
judging text generation. If this feature ships a "how good is my clone" preview, back it with an
ASR round-trip check, not an LLM's opinion.

## Self-hosting status: blocked on Henry's RunPod signup

Plan in progress when this branched off: deploy a generic PyTorch/CUDA GPU pod on RunPod (no
ready-made VoxCPM2 template exists there), `pip install voxcpm` on it, run generation via SSH/API.
RunPod bills **per hour the pod is running**, not per-request — unlike everything else in this
research (fal, Replicate, Deepdub all bill per-generation) — so a production wrapper would need to
either start/stop the pod around each job, or move to RunPod Serverless (scales to zero between
requests) once there's a working container. **Needs a RunPod account + API key + billing from Henry
before this can continue** — account/payment setup is not something to do on his behalf.

## Reference artifacts (for A/B listening, don't need to regenerate)

All in `Video-Production/one-off/avatar-poc-fal/voice-clone/`:
- `voxcpm2_test_v2.wav` / `voxcpm2_test_v3_longref.wav` — the tuned VoxCPM2 outputs (control_instruction + cfg=3.0), 20s vs 45s reference
- `minimax_28hd_v2_5minref.mp3` — best MiniMax Speech-2.8 result, for comparison
- `deepdub_clone.mp3` — Deepdub's word-perfect-but-basic-sounding output
- `rvc_final_henry.mp3` — the RVC voice-conversion attempt (degraded, for reference on what NOT to do)
- `sample_short.mp3` / `sample_45s.mp3` / `rvc_clean_295s.mp3` — the Henry reference clips used across tests, sourced from `skills_tutorial_full.mp3` (20-min extraction of a real Henry tutorial recording)

## Suggested next step

Run `superpowers:brainstorming` on the actual product shape before writing any code: standalone app
vs. hebrew-dictation feature, local-API-server pattern (matching the Voicebox precedent already in
`HANDOFF.md`) vs. embedded Python subprocess vs. remote RunPod call, and how a Tauri+Rust app would
even shell out to a Python/PyTorch model in the first place (that packaging question — bundling a
Python runtime + CUDA deps inside a Tauri installer — is itself a real open design problem, not a
solved one).
