# Handoff — Hebrew transcription quality (missed words, wrong words)

**Opened:** 2026-08-26 · **Status:** 2026-08-27 — Step 0 infra built, waiting on Henry's real
dictations. Full plan: `~/.claude/plans/c-users-claude-dev-ai-tools-mcp-dev-hebr-shimmering-crescent.md`.
**Raised by Henry:** dictation drops words and produces wrong ones. His idea: add automatic
correction, or a second model that reads the transcript and fixes words using sentence context.

Read this before proposing anything. A large part of the idea already exists in the codebase,
and the reason it does not solve the problem is specific and fixable.

## 2026-08-27 session — what's built, what's still open

Henry decided: **measure before deciding** on any fix, keep researching the keyterm/vocabulary
angle before building it, and keep Groq→Gemini frozen unless the measurement points at it.

**Built (code, tested, not yet exercised on real data):**
- `settings::AppSettings.debug_save_audio` (default `false`, settings.json-only, no frontend
  toggle — same pattern as `local_api_enabled`) + `merge_frontend_update` preservation + test.
- `save_debug_sample()` in `lib.rs`, called from `stop_streaming_transcription`: when the flag
  is on, writes the dictation's audio (`write_wav_16k_mono`, already existed) and Deepgram's raw
  text to `%APPDATA%/hebrew-dictation/samples/<unix-ms>.{wav,deepgram.txt}`. All failures are
  logged and swallowed — can't affect the real dictation path. `cargo test`: 123 passed.
- `benchmark/run_benchmark.py` extended with two backends the Phase-0-era script didn't have:
  **Deepgram streaming** (`transcribe_deepgram_streaming`, mirrors `streaming.rs`'s
  accumulate-`is_final`-segments logic) and **Deepgram batch + keyterm**
  (`--keyterms "term1,term2"`). Both smoke-tested offline (WAV parsing, URL-encoding of Hebrew
  keyterms) — not yet run against real Deepgram, no `.env` configured on this machine.
  `--local-model ivrit-ai/whisper-large-v3-turbo-ct2` documented as the Hebrew-tuned faster-whisper
  build (found via research, distinct from the ggml build the app itself uses).
- `docs/research/2026-08-27-vocabulary-mechanisms.md` — keyterm/replace/prompt comparison across
  all three engines, verified against live Deepgram/Groq docs. Research only, nothing implemented;
  Hebrew's prefix-attachment (ב/ו/ה/כ/ל/מ/ש) is flagged as the reason `replace=` alone won't
  scale and why classifying Henry's actual errors (below) has to come first.

**Still open — needs Henry, not code:**
1. **Step 0א (no code, ~20 min):** 5 real dictations, compare intended text vs on-screen text vs
   `mcp__hebrew-dictation__get_last_transcript`. This is the fork in the road — if screen ≠ MCP,
   the bug is injection (`injector.rs`) and none of the ASR work below is relevant.
2. Turn on `debug_save_audio: true` in settings.json (no UI yet) for those 5 dictations so the
   samples land in `%APPDATA%/hebrew-dictation/samples/` for benchmark re-use.
3. Copy those samples into `benchmark/samples/sample_NN/` with `reference.txt` (what Henry meant
   to say), fill `benchmark/.env` from `.env.example`, run
   `python run_benchmark.py --keyterms "..."` — produces `results.md` with WER per engine.
4. Only after that data exists: decide whether keyterm/replace/engine-switch/Gemini-unfreeze is
   worth building, per the research doc above.

---

## 1. The finding that should shape the whole session

**Smart Cleanup already exists** (`src-tauri/src/enhance.rs`) — a post-transcription pass through
Groq `llama-3.3-70b-versatile` at temperature 0.2. But it is deliberately built as a *cleaner*,
not a *corrector*. Its system prompt says, verbatim:

> (3) הסר מילות מילוי (אהה, אמ, יעני, כאילו), חזרות וגמגומים, ותקן פיסוק ורווחים.
> (4) **שמור בדיוק על המשמעות**, הטון והשפה של הדובר. **אל תוסיף מידע**, אל תקצר משמעותית…

Rule 4 forbids exactly the thing Henry is asking for. If Whisper hears *כלב* where the speaker
said *כלוב*, Smart Cleanup is instructed to leave it alone — the sentence is already fluent, and
"fix a word that is wrong but plausible" is outside its brief.

**So the gap is not "there is no second model". It is that the second model is told not to do
this job.** Any proposal should start from that, not from building a new pipeline stage.

### Two more facts that likely matter more than they look

- **`enhance_enabled` defaults to `false`** (`settings.rs`). Smart Cleanup is opt-in. **Confirm
  with Henry whether it is even switched on for him** before analysing its output quality — the
  errors he is describing may be coming from raw transcription with no cleanup pass at all.
- **Streaming is on by default** (`streaming_enabled: true`) and injects **per final segment**.
  A per-segment corrector sees one fragment at a time and has far less context than a corrector
  that runs on the whole utterance. Whatever is designed here has to state which of the two
  paths it applies to — they are different call sites (see `lib.rs` `inject_text` vs
  `streaming.rs::handle_message`).

---

## 2. Where the errors could be coming from — establish this before fixing

Do not assume the fix belongs in post-processing. Defaults today:

| Setting | Default |
|---|---|
| `transcription_mode` | `AutoFallback` |
| `api_provider` | `Deepgram` (Nova-3) |
| `streaming_enabled` | `true` |
| `enhance_enabled` | **`false`** |

The app also ships **ivrit.ai**, a local model fine-tuned on ~5,000 hours of Hebrew (Knesset +
Wikipedia). If Deepgram Nova-3 is the source of the word errors, a model actually trained on
Hebrew may beat any amount of downstream correction — and it costs nothing per use.

**First real task of the session: get a measurement, not an impression.** Have Henry record a
handful of representative dictations, then compare the same audio across Deepgram streaming,
Deepgram batch, Groq Whisper Turbo, and local ivrit.ai. Without that, there is no way to tell
whether to fix the recognizer or the corrector, and no way to prove any change helped.

Note the streaming/batch distinction is itself a candidate: streaming commits to words with less
right-hand context than a batch pass over the whole utterance.

---

## 3. Directions, roughly in order of cost

1. **Loosen the Smart Cleanup brief** — add an `EnhanceMode` variant that is allowed to correct
   likely mis-recognitions using sentence context, while still forbidding invention. Cheapest
   path; the plumbing, the fail-safe, and the tests all already exist. `EnhanceMode` is already
   an enum with a `from_str` fallback, so a second mode fits the existing shape.
   ⚠️ Raises the stakes on a known failure: `enhance_inner` did once inject a truncated
   completion silently (fixed `dd082f8` by checking `finish_reason == "length"`). A corrector
   that may rewrite words needs at least that much guarding, probably more.
2. **Change the corrector model.** `llama-3.3-70b-versatile` is a general model with no particular
   Hebrew strength. **⚠️ Henry explicitly PAUSED the Groq→Gemini switch on 2026-07-29** — a
   deliberate hold, not a rejection, and his stated blocker (does Gemini need a credit card?) is
   resolved: AI Studio issues free-tier keys with no card. **Do not implement it without asking
   him again.** This session is a natural moment to revisit that decision, since it is now the
   direct route to the thing he wants.
3. **Change the recognizer** — ivrit.ai, or batch instead of streaming. No per-use cost, no extra
   latency from a second network call, and it attacks the error at the source.
4. **A custom vocabulary / keyword-boost layer.** Deepgram supports keyword boosting. Henry's
   dictations are full of recurring proper nouns (tool names, client names, Hebrew tech jargon)
   that a general model reliably gets wrong. Cheap, targeted, and complements any of the above.

These are not exclusive. 3+4 attack the cause; 1+2 attack the symptom.

---

## 4. Constraints to respect

- **Fail-safe is non-negotiable.** Smart Cleanup returns raw text on any error. A corrector that
  can *change words* must never be able to make output worse than not running — and unlike
  cleanup, a bad correction is invisible to the user, who sees fluent, confident, wrong text.
- **Latency is a feature.** This runs between speaking and text appearing. A correction pass that
  adds a noticeable pause may be rejected regardless of accuracy.
- **Free tiers have caps.** Groq `llama-3.3-70b-versatile` is 100k tokens/day. This was the exact
  reason EN→HE translation was shelved (`docs/superpowers/specs/2026-07-19-en-to-he-translation-design.md`)
  — read that spec before assuming a per-dictation LLM call is free at Henry's volume.
- **No Hebrew measurement exists on any frontier model** ([[prompting-evidence-2026]]). Nobody's
  benchmark answers "which model is best at Hebrew" — hence the measurement task above.

---

## 5. Questions for Henry, first thing

1. **Is Smart Cleanup actually enabled for you?** (Settings → רישוף חכם.) It is off by default.
2. **Can you give 3–5 real examples** — what you said vs what appeared? Dropped words and wrong
   words are different failures with different fixes, and right now we have neither separated
   nor quantified.
3. **Which mode are you dictating in** — cloud streaming (default) or local?
4. **Revisit Gemini?** The switch you paused on 2026-07-29 is the most direct route to what you
   are asking for.

---

## 6. State of the app as this opens

v2.13.1 released and live on both platforms, CI builds and signs both from a tag push, website
current, Cloudflare deploy-failure alerts configured and tested. Nothing is mid-flight; this
brief starts from a clean tree.

Full project context: `memory/hebrew-dictation.md`. Release history: `HANDOFF.md`.
