# English → Hebrew Translation — Design Spec

- **Date:** 2026-07-19
- **Status:** Draft (spec-review round 3 — chunked architecture)
- **Backlog origin:** HANDOFF "Not done — flagged for future": *English → Hebrew translation (upload/dictate in English, auto-translate to Hebrew) — Henry-confirmed roadmap idea from 2026-07-02, not started.*

---

## 1. Problem & scope

Henry consumes English AI/tech content and produces Hebrew. He wants to hand the app an English recording — **interviews, videos, podcasts** — and get a Hebrew transcript.

**What already exists (verified in the code v1 actually traverses):**
- **English transcription already works in the batch path.** `transcribe_deepgram_batch` builds `…&language={}` into the nova-3 URL (`api_transcribe.rs:308`), and `run_long_transcription` passes the language token to whisper. (Note: `VALID_LANGUAGES`/`validate_language` at `api_transcribe.rs:486-496` guards only the **live-dictation** path via `transcribe_api_inner`; batch never calls it. Cited here so the next reader doesn't mistake it for batch's gate.)
- **An LLM post-processing path exists** — Smart Cleanup (`enhance.rs`): Groq `llama-3.3-70b-versatile`, pure unit-tested helpers, caller falls back to raw on error.
- **It explicitly forbids translation** — `HeGeneral` rule 4: *"…**אל תתרגם**"*.
- **`enhance_text` is invoked exactly once in the frontend** — `App.tsx:552`, live dictation only. **The batch path has no LLM post-processing today**; this feature adds the first.

### Scope decisions (Henry, 2026-07-19)

| Decision | Choice |
|---|---|
| Which flows | **Picked files only** (§2.1). Live dictation, recordings, meeting modes deferred. |
| Where the user chooses | **Per-action checkbox** — a persistent global mode would eventually feed Hebrew text to a translate-to-Hebrew prompt. |
| On failure | **Show an error**, keep the English (§4). |
| The English original | **Hebrew replaces it.** Deliberate loss. |
| **Long files** | **Chunked translation** (§4) — a single call cannot do the job (§4.1). |

### Non-goals (v1)

Live-dictation translation · recording flows and meeting modes (§2.1) · any pair other than EN→HE · preserving both texts · translated SRT with original timings · auto-detecting that audio is English · **cross-chunk terminology context** (§4.5).

---

## 2. Architecture

```
"בחר קבצים"  +  ☑ תרגם לעברית
     │
     ├─ pre-flight: local engine + ivrit-* model → refuse, explain      §5.2
     ├─ transcribe_file(..., language: "en")   [batch hardcodes "he" today — §5.1]
     │
     ▼   English text + segments
     │
     ├─ translate_transcript(text)  ── Rust owns chunking ──►  §4
     │     split into ≤3000-char chunks on sentence boundaries
     │     for each chunk:  enhance_inner(chunk, EnToHe, key, 30s)
     │                      ├─ finish_reason must be "stop"   §4.3
     │                      └─ validate_output(mode-aware)     §3.1
     │     emit progress event per chunk;  abort on cancel     §4.4
     │
     ▼   Hebrew text replaces `transcript`; `translated: true`  §5.3
```

### 2.1 Which flows the checkbox covers

| Flow | v1? | Why |
|---|---|---|
| **"בחר קבצים"** | ✅ | The stated use case: existing English interviews/videos/podcasts. |
| **"הקלט ותמלל"** (Mic/System) | ❌ | Recording English *through the app* to translate is an edge case. |
| **Meeting modes** (`stop_call_recording`) | ❌ | They bypass `transcribe_file` (`App.tsx:1112-1119`) and produce text with **`"אני: "` / `"הצד השני: "` baked in as plain content** — an LLM would mangle those prefixes. Needs its own design. |

The checkbox renders only in the file-picking flow.

---

## 3. `enhance.rs` — a new mode

Add `EnToHe` alongside `HeGeneral`; `ChatMessage`, `build_messages`, `enhance_inner` and `EnhanceError` are reused.

- `from_str("en_to_he")` → `EnToHe`; **unknown strings still fall back to `HeGeneral`** (load-bearing back-compat, existing test).
- `system_prompt()`: translate English→Hebrew; output **only** the translation; natural written Hebrew, not word-for-word; preserve meaning, tone, register; drop disfluencies; never answer or comment on the content.
- `example_raw()` / `example_clean()`: messy English → clean Hebrew, anchoring "transform the message", not "reply to it".

### 3.1 The 2× ceiling must not be reused as-is

`validate_output` rejects output `> raw.chars().count() * 2` (`enhance.rs:120`, **strictly `>`** — the existing test depends on that: 21 rejected against a ceiling of 20). Calibrated for cleanup, it rejects **correct** short translations:

| English | chars | Hebrew | chars | Ratio |
|---|---|---|---|---|
| `AI` | 2 | `בינה מלאכותית` | 13 | **6.5×** |
| `USA` | 3 | `ארצות הברית` | 11 | **3.7×** |

Make the ceiling mode-owned, leaving `HeGeneral` behaviour byte-identical:

```rust
fn max_output_chars(&self, raw_chars: usize) -> usize {
    match self {
        EnhanceMode::HeGeneral => raw_chars * 2,            // unchanged
        EnhanceMode::EnToHe    => (raw_chars * 3).max(raw_chars + 200),
    }
}
```

**Honest limitation:** the additive floor dominates for `raw ≤ 100` (crossover exactly at 100), so on short input the ceiling admits ~200 chars of slack — enough that the classic conversational failure (*"אשמח לעזור! שלח לי בבקשה את הטקסט…"*, ~40-80 chars) would pass it. **On short input the one-shot example is the sole defence against that failure, not this ceiling.** No length rule can separate a legitimate 13-char translation from a 60-char refusal; the ceiling's real job is catching runaways on long input, and §7 tests it there.

> Changing the signature touches `validate_output`'s three existing call sites (`enhance.rs:206, 213, 221`) — mechanical, but the plan must budget for it.

---

## 4. Chunked translation

### 4.1 Why a single call cannot work

Three independent limits converge, and they were mutually incompatible in the previous draft:
- **Truncation (§4.3)** — a long completion can stop at the model's cap.
- **Wall-clock** — a 30,000-char input yields ~10-15k output tokens, which does not complete in 60s at this model's throughput.
- **Coverage** — an hour-long podcast is ~60,000 chars. A length ceiling low enough to be safe refuses the very files v1 exists for, *and* the length is only knowable **after** the user has paid the full transcription cost.

Chunking dissolves all three: every call is small, fast, individually verifiable, and there is no length ceiling.

### 4.2 Splitting

A pure, unit-tested Rust function — the TDD seam:

```rust
/// Split `text` into chunks of at most `max_chars`, breaking only at sentence
/// boundaries so no chunk starts or ends mid-sentence. A single sentence longer
/// than `max_chars` becomes its own (over-long) chunk rather than being cut.
pub fn split_for_translation(text: &str, max_chars: usize) -> Vec<String>
```
- `max_chars = 3000` (≈500 English words). Output ≈2500 Hebrew chars ≈1200 tokens — completes in seconds, far from any completion cap.
- Splits on sentence-final punctuation (`.` `!` `?` and their newline-adjacent forms). **Never splits mid-sentence**; an over-long sentence passes through whole (rare, and the ceiling in §3.1 still bounds its output).
- Chunking lives in **Rust, not TypeScript** — this project has a Rust test suite and no frontend test infrastructure, so the logic belongs where it can be tested.

### 4.3 Per-chunk truncation guard (the silent-data-loss bug)

`enhance_inner` currently sends no `max_tokens` and reads `choices[0].message.content` without inspecting `finish_reason` (`enhance.rs:143-178`). A truncated completion returns **half a translation**, which — being *shorter* than the input — passes every existing guard. Combined with "Hebrew replaces English", that silently overwrites the transcript with half a result and the original is unrecoverable. It is invisible to §6's table and grows more likely with length.

**Fix:** `enhance_inner` must read `choices[0].finish_reason` and treat anything other than `"stop"` (notably `"length"`) as an error — a new `EnhanceError::Truncated`. This is required for the cleanup path too, not only translation.

### 4.4 Orchestration, progress and cancel

```rust
#[tauri::command]
async fn translate_transcript(app: AppHandle, state: State<'_, AppState>, text: String)
    -> Result<String, String>
```
- Groq key required; clear Hebrew error if absent.
- `split_for_translation(&text, 3000)` → translate each chunk with `EnhanceMode::EnToHe`, **30s** per chunk.
- Emits a `translation-progress` event per chunk (`{done, total}`) — the app already uses Tauri events for live transcription, so this follows the existing pattern.
- **Checks the cancel flag between chunks**, so cancelling actually stops the work. (Today `cancel_batch` trips flags consumed only by decode/transcribe, and the loop's cancel check sits at the top of each iteration — an in-flight translation would otherwise run to completion after the user cancels.)
- **Any chunk failing fails the whole translation** — never return a partial. Returning "chunks 1-3 of 7" would recreate the truncation bug at a different layer.
- Joins the translated chunks with a single space/newline preserving paragraph breaks.

### 4.5 Known limitation — no cross-chunk context

Each chunk is translated independently, so terminology can drift between chunks (e.g. a product name rendered two ways). Passing the previous chunk's tail as context would reduce this at the cost of tokens and complexity; deferred to v2 and listed as a non-goal so it is a known trade, not an oversight.

### 4.6 Why a separate command, not `enhance_text`

`enhance_text` re-reads `enhance_enabled` (`lib.rs:336-352` — *"so a stale frontend can't force enhancement"*), so a user with Smart Cleanup **off** could not translate. That gate prevents text reaching Groq *silently*; translation is never silent — the per-file checkbox **is** the consent.

**Failure semantics are inverted from cleanup, deliberately.** Cleanup failing silently is benign. **Translation failing silently returns English where Hebrew was requested** — indistinguishable from "the checkbox did nothing". So errors propagate; there is no fallback to raw text.

---

## 5. Frontend

A `☑ תרגם לעברית` checkbox in the file-picking flow.

### 5.1 Source language — batch hardcodes Hebrew today

The batch path does **not** read the global `language` setting: it passes `language: "he"` literally (`App.tsx:1006`; `1114`/`1118` for the recording paths). The global setting reaches only live dictation (`languageRef.current`, `App.tsx:544`). *(An earlier draft of this spec claimed a "two knobs must agree" hazard — that hazard does not exist; this is the correction.)*

Change: when ticked, pass **`language: "en"`** instead of the hardcoded `"he"` for that run. `BatchOpts.language` is already a per-call field (`batch.rs`); nothing new backend-side.

### 5.2 Pre-flight guard: ivrit-* models ignore the language argument

`whisper.rs` (`transcribe` ~82-86, `run_long_transcription` ~187-192) forces `effective_lang = "he"` when the model name starts with `ivrit-`, **regardless of the argument**. On the local engine with such a model the `"en"` override is silently discarded: English audio decodes against a forced Hebrew token, and that garbage is then translated — a plausible-looking wrong result rather than a clean failure.

**Guard before transcription starts:** ticked + local engine + `ivrit-*` model → refuse with an actionable Hebrew error (switch model, or use the cloud engine). Mirrors the existing precedent `batch::ensure_local_meeting_model_available` (`lib.rs:608-613`), which reads `s.preferred_model`.

> Read the same field the precedent reads (`s.preferred_model`). Note the model that actually runs is the loaded engine's `model_name` (`lib.rs:471-474`), set by the last `load_model` call. Normally in sync; the plan should state which one it reads rather than leaving it implicit.

### 5.3 SRT eligibility

`isSrtEligible` (`App.tsx:1159-1160`) is `status === "done" && !edited && segments.length > 0`. The `edited` flag exists because hand-editing desynchronizes the transcript from `segments`. **Translation is exactly that desynchronization** — `segments` hold English words and timings while `transcript` becomes Hebrew. Left alone, "🎬 SRT" would export **English subtitles under a Hebrew transcript**, and so would the combined multi-file export (`App.tsx:1217`, same predicate).

Fix: add `translated?: boolean` to `BatchResult`, set it on success, and extend `isSrtEligible` to require `!translated`. Both export paths inherit the fix through that one predicate.

**Do not reuse `edited`** — the user edited nothing; the flag would lie to every future reader.

**TXT / Word exports stay available** — they read `r.transcript` (`exportSingle`/`exportBatch`), never `segments`, so the Hebrew is legitimately what they should export. Only SRT is wrong.

### 5.4 Translation stage in the UI

`batchStage` is `"decoding" | "transcribing" | "done"`. Translation bolts a deliberately-slow multi-call stage onto the end of each file; with no stage the view would sit looking finished while it runs, which reads as a hang — the opposite of §4.6's loud-failure intent. Add a **`"translating"`** stage showing chunk progress (`מתרגם… 3/7`) from the `translation-progress` event. Precedent: the live path already does `setStatus("enhancing")` (`App.tsx:550`).

---

## 6. Error handling

| Failure | Behavior |
|---|---|
| Local engine + `ivrit-*` model | Refused **before** transcription (§5.2); nothing spent. |
| No Groq key | Clear Hebrew error; transcript stays English. |
| Any chunk: 401/403/429/timeout(30s)/network | Existing `EnhanceError` strings surface; **whole** translation fails; transcript stays English. |
| Any chunk truncated (`finish_reason != "stop"`) | New `EnhanceError::Truncated` (§4.3); whole translation fails; transcript stays English. |
| Any chunk empty or over ceiling | `validate_output` rejects (§3.1); whole translation fails. |
| User cancels mid-translation | Stops at the next chunk boundary (§4.4); transcript stays English. |
| Transcription itself fails | Unchanged — translation never runs. |

In every failure the item remains a normal, **SRT-eligible English result**. Failure is always visible and never destructive.

---

## 7. Testing

**Rust (TDD):**
- `from_str("en_to_he")` → `EnToHe`; unknown → `HeGeneral`.
- `build_messages(EnToHe, …)` → 4 messages, translate prompt, English example `user`, Hebrew example `assistant`, real text **last**.
- `max_output_chars`: `HeGeneral` returns exactly `raw*2` (no behaviour change); `EnToHe` admits `"AI"`→`"בינה מלאכותית"` (13 ≤ 202 for raw=2) and still rejects a runaway at raw=5000 (ceiling 15000).
- `split_for_translation`: text under the limit → one chunk; multi-sentence text → splits only at sentence ends, no chunk exceeds `max_chars`, **rejoining the chunks reproduces the input** (no text lost or duplicated — the property that matters most); a single over-long sentence becomes one over-long chunk rather than being cut.
- `finish_reason != "stop"` → `EnhanceError::Truncated` (parser-level test on a canned response body).

**Frontend:**
- `isSrtEligible` false when `translated` is true; still true for an untranslated done result.

---

## 8. Acceptance criteria

- An English file with the box ticked yields a **Hebrew** transcript; unticked, English.
- **A ~1-hour English podcast translates end to end** — chunk progress advances, no truncation, no timeout. This is the criterion the previous single-call design failed.
- A short English clip (one sentence naming a tool) translates without a "suspicious output" error (§3.1).
- **Manual QA, named observable:** an English clip containing *um* / *you know* yields Hebrew with **no filler artifacts** — pins the "translation subsumes cleanup" assumption, which no unit test can check.
- A translated item offers **no** SRT export (per-item *and* combined); TXT/Word still work; untranslated items still offer SRT.
- Smart Cleanup **off** does not block translation.
- Ticked + local engine + `ivrit-*` model refuses up-front with an actionable message.
- Cancelling mid-translation stops at the next chunk boundary and leaves the English intact.
- Any failure leaves a visible error and an intact English transcript.
- The existing **63 passing / 1 ignored** stay green; `cargo build` no new warnings; `tsc && vite build` clean.
