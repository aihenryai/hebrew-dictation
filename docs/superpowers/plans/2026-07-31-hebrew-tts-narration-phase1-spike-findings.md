# Hebrew TTS Phase 1 — Spike Findings

**Date run:** 2026-08-06 · **Verdict: CONDITIONAL — needs Henry's decision, not a clean GO/NO-GO** (see §Verdict)

---

## Pinned Facts (for Chunk 2+)

- **`piper-tts` exact version:** `1.6.0` (installed via `pip install "piper-tts>=1.6.0"`, resolved to 1.6.0 — no newer patch available at spike time).
- **HTTP extra required:** yes — plain `pip install piper-tts` gives `ModuleNotFoundError: No module named 'flask'` when running `python -m piper.http_server`. Must install `pip install "piper-tts[http]"`. **This is a required fix to Chunk 3's provisioning plan** — `PIPER_TTS_VERSION` should drive an install of `piper-tts[http]==1.6.0`, not bare `piper-tts==1.6.0`.
- **Voice download URLs, sizes, hashes:**
  - `he_IL-saspeech-medium.onnx`: **63,221,984 bytes**, SHA-256 `3dc067debc9e782a8a0d095dbb58786648743d406366dcc2aa81009660873b4d`
  - `he_IL-saspeech-medium.onnx.json` (config, needed alongside the .onnx): 5,269 bytes
  - Both downloaded via `python -m piper.download_voices he_IL-saspeech-medium --download-dir .` — this resolves the voice name to the real HuggingFace URLs internally; Chunk 3's `VOICE_URL` constant should point at the resolved HF URL (`https://huggingface.co/rhasspy/piper-voices/resolve/main/he/he_IL/saspeech/medium/he_IL-saspeech-medium.onnx`), confirmed reachable.
- **Nakdimon model source:** **bundled directly in the `piper-tts` wheel** (`piper/hebrew/nakdimon.onnx`, installed automatically with the package) — NOT a separate first-use download. This simplifies Chunk 3: no extra download step needed for Nakdimon, it's already present once `piper-tts[http]` is pip-installed.
- **License/attribution — ⚠️ real constraint, not a footnote:** the `saspeech` dataset (MODEL_CARD points to `openslr.org/134`) is **"Custom non-commercial" license, copyright owned by IPBC (Israeli Public Broadcast Corporation)**, the voice is Shaul Amsterdamski's recorded speech from Kan's "חיות כיס" podcast. Explicit terms: no commercial/broadcast use, no political use, no implying IPBC endorsement, must not "bring harm to Shaul Amsterdamski and/or the IPBC." **hebrew-dictation is currently free/MIT-licensed, so this is fine today** — but this specific voice **cannot be used if the app is ever monetized/commercialized**. Needs real attribution text in the app's credits screen (not just a passing mention), and this constraint should be written into the spec's licensing section, not just this findings doc.
- **Cold-start / latency / RAM:**
  - Cold-start: not cleanly measured (the poll caught the server already warm due to tool round-trip delay) — budget ~2-5s based on how quickly it became responsive; re-measure precisely during real implementation with a tighter timing harness.
  - Per-request latency (warm, ~2-sentence input): **~940ms**.
  - Warm-idle RAM: **~448MB** RSS for the server's `python.exe` process after several requests (started around 154MB right after model load, grew to 448MB — worth confirming in real implementation whether this is a one-time growth to a ceiling or genuinely unbounded).
- **`/synthesize` behavior:** returns raw WAV bytes with a **misleading `Content-Type: text/html` header** (Flask's response-type default, not explicitly overridden by piper1-gpl) — the body itself is a correct, valid WAV. **This does not affect the Rust plan** (Chunk 2's `synthesize()` never checks Content-Type, only the RIFF/WAVE magic bytes + status code), but is worth a one-line code comment when implementing so nobody "fixes" it by trusting the header.
- **`/info` response shape confirmed:** `{"last": null, "voice": {"language": "he", "name": "he_IL-saspeech-medium", "num_speakers": 1}}` before first use; `last` populates with `{text, synthesize_seconds, phonemes, alignments}` after — matches the spec's assumed shape.

---

## ⚠️ Real bug found in my own test methodology (not the product) — worth documenting so it isn't rediscovered

The first full run of the 5-sentence ASR check produced near-total gibberish across every sentence (e.g. "אני בודק את המערכת." → "האם חתר"). Root-caused via `/info`'s `last.text` field: **curl's `-d '{"text":"..."}'` on Windows/Git-Bash silently mangled the Hebrew UTF-8 text into literal `?` characters** before it ever reached the server — a shell/argv encoding artifact, not a server or model bug. Confirmed by re-sending the identical requests via Python's `urllib` (proper UTF-8 JSON body, byte-for-byte how Rust's `reqwest::Client::post(url).json(&body)` will construct the real request) — `/info` then showed the correct Hebrew text server-side. **This does not affect the real Rust implementation at all** (Rust builds the JSON body via `serde`, never touches a shell), but it's a reminder that `curl -d` with non-ASCII text on Windows is not a trustworthy way to manually test this API — use a proper HTTP client (Python `urllib`/`requests`, PowerShell `Invoke-RestMethod` with `ConvertTo-Json`, or a real Rust test) instead.

---

## Objective quality results (ASR round-trip via `faster-whisper` "small", proper UTF-8 requests)

| Sentence | Original | Transcribed | Assessment |
|---|---|---|---|
| s2 (multi-clause) | "אחרי שסיימתי את הישיבה, יצאתי לקנות קפה, ופגשתי חבר ותיק ברחוב." | "אחרי שסיימתי את הישיבה יצאתי לקנות **כפה** ו**בראשתי** חבר ותיק ברחוב" | **Good.** Near-exact; the two diffs are plausible ASR confusions on close-sounding words (כפה/קפה), not obviously TTS failures. |
| s3 (trap word) | "בדקתי את כל המתגים בלוח החשמל." | "בדקתי את כל ה**מתאגים** בלוח החשמל" | **Good, and consistent with prior research.** Only the known hard word "מתגים" is slightly off (מתגים→מתאגים) — matches `HANDOFF-TTS-VOICE-GENERATION.md`'s existing finding that this exact word fails on *every* Hebrew TTS model tested so far. Not a new problem, a confirmed old one. |
| s5 (paragraph, ~80 words) | (long paragraph, see spec) | (near-exact, minor letter-level slips: כלים חדשים→כלים חרשים, ולומדים→ולמדים, etc.) | **Good.** Structure, word boundaries, and meaning all correct; errors are typical ASR-on-longer-audio noise, not TTS breakdown. |
| s4 (mixed Hebrew+English+digit) | "הורדתי את ChatGPT 5 אתמול." | "חורנתי תמיד תמיד מלטמון" | **Bad — a real, reproducible limitation.** Non-Hebrew tokens (brand names, digits) inside Hebrew text break badly. Expected given the phonemizer is Hebrew-specific (Nakdimon + Hebrew IPA), but confirmed here, not just theorized. |
| s1 (short, 4 words) | "אני בודק את המערכת." | Wildly inconsistent across repeated attempts: "אני מודה כתה למה דרך" / "אני מותקת אמר, איך?" / "אני בודק את המהורך" / "אני בודוק את המערכת" (this last one nearly perfect) | **Unreliable — the most important finding of this spike.** |

### The s1 finding, investigated further (this is the real risk, not s4)

Isolated direct-library synthesis (`voice.synthesize_wav()`, one-shot, fresh-loaded voice) of this *exact same sentence* produced a **perfect** transcription on the first try. But calling it repeatedly — via the real HTTP server, and via a hand-replicated version of the HTTP server's own code path (same `SynthesisConfig`, same `include_alignments=True`, same warm/reused voice object) — produced **different, sometimes badly garbled, output on different calls of the identical text**, unpredictably. Tried reducing `noise_scale`/`noise_w_scale` from the voice's defaults (0.667/0.8) down to 0.1/0.1 (less stochastic sampling) — **instability persisted**, just not eliminated by that knob alone.

> **Addendum (2026-08-14, after Phase 1 shipped and was manually tested):** the instability is real but **partly a speed artifact**, which this spike missed by only tuning `noise_scale`/`noise_w_scale` and never varying `length_scale`. Re-measured with the same sentence, same ASR round-trip, 6 takes per setting: at `length_scale` **1.0 → 0/6 exact**, at **1.18 → 2/6 exact** — and the failures change character, from unrecognizable ("אניבודי כתמדר", "אני רוצה כתמלך") to near-misses that differ by one letter ("אני בודק את המערכ"). Slowing the phoneme rate does not eliminate the variance but materially reduces it, so 1.18 became the shipped default. Separately, piper's `--sentence-silence` defaults to **0.0** — no pause between sentences at all — which this spike never tested and which accounted for much of the "runs together / ignores punctuation" impression; it now ships at 0.45s.

**What this means:** this is not "the voice is bad" — longer, natural sentences work well and consistently in this testing. It's specifically that **short utterances synthesized on a warm, repeatedly-used voice instance show real run-to-run quality variance** — the same input can come out clean or garbled depending on the vocoder's internal stochastic sampling, which isn't seeded. This is exactly the kind of thing a warm, long-lived sidecar (this feature's whole architecture) would expose in real use: a user typing a short sentence could get a great result once and garbage the next time, unpredictably, for no visible reason.

---

## Verdict

**Not a clean GO. Not a clean NO-GO either.** The evidence supports a real product decision, not a technical unknown:

- **What works well:** natural, realistic-length Hebrew sentences (the actual target use case — "a paragraph of narration text" per spec §1) — consistently good, intelligible, sometimes near-perfect.
- **What's a known, already-accepted limitation:** the "מתגים" word class — already documented, already accepted as a cross-model Hebrew TTS limitation, not new.
- **What's a new, real limitation:** mixed Hebrew+English+digit content breaks. Worth a note in the UI ("כתבו בעברית בלבד לתוצאה הכי טובה") or a pre-flight warning, not necessarily a blocker.
- **What's a genuine open risk:** short-utterance output is **stochastically unreliable** — not consistently bad, just consistently *inconsistent*, in a way this spike could not fully resolve (noise-scale reduction didn't cleanly fix it, and further tuning — different noise values, `speaker_id` handling, or a "generate twice and keep the better one" heuristic — is a real engineering rabbit hole, not a quick fix).

Per this plan's own §6.3 rule ("do not silently fall back to MMS-TTS or anything else — every alternative was rejected for cause"), and given the mixed nature of this result, **this needs your call, not a unilateral one:**

1. **Ship anyway, framed honestly** — the real target use case (paragraphs) tests well; short-sentence flakiness is a real but survivable rough edge for a v1, especially since users can just retry a bad generation.
2. **Investigate the instability further before building the app UI** — try more noise-scale combinations, `speaker_id` variants, or whether this is a known upstream piper1-gpl issue (worth checking their GitHub issues) before committing engineering time to Chunks 2-7.
3. **Stop here** — the instability is disqualifying for a "reliable narration tool" bar, revisit Hebrew TTS options later as the ecosystem matures (piper's Hebrew support is literally days old).

I lean toward **option 1** (the failure mode is "occasionally needs a retry," not "unusable" — and the target use case tests consistently well) but this is exactly the kind of quality-bar call that's yours to make, not mine.

**All raw audio files, the mangled-vs-correct comparison, and the debug scripts are preserved in `C:\Users\אורח\scratch\piper-spike\` for your own listening if you want to judge s1's garbled samples yourself before deciding.**
