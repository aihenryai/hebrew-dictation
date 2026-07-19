# English → Hebrew Translation — Design Spec

- **Date:** 2026-07-19
- **Status:** Draft (spec-review round 2)
- **Backlog origin:** HANDOFF "Not done — flagged for future": *English → Hebrew translation (upload/dictate in English, auto-translate to Hebrew) — Henry-confirmed roadmap idea from 2026-07-02, not started.*

---

## 1. Problem & scope

Henry consumes English AI/tech content and produces Hebrew. He wants to hand the app an English recording and get a **Hebrew** transcript.

**What already exists (verified in code):**
- **English transcription already works** — `VALID_LANGUAGES` (`api_transcribe.rs:487`) includes `"en"`.
- **An LLM post-processing path already exists** — Smart Cleanup (`enhance.rs`): Groq `llama-3.3-70b-versatile`, pure unit-tested `build_messages` / `validate_output`, caller falls back to raw text on any error.
- **That path explicitly forbids translation** — `HeGeneral`'s prompt rule 4: *"אל תוסיף מידע, אל תקצר משמעותית, **אל תתרגם**"*.
- **`enhance_text` is invoked exactly once in the whole frontend** — `App.tsx:552`, inside the live-dictation handler. **The batch path has no LLM post-processing today.** This feature adds the first.

So this is **not** "transcribe English" — it is what happens to the text *after* transcription: a new `EnhanceMode` variant plus its plumbing.

### Scope decisions (Henry, 2026-07-19)

| Decision | Choice |
|---|---|
| Which paths | **Picked files only in v1** (see §2.1). Live dictation deferred. |
| Where the user chooses | **Per-action checkbox**, not a persistent setting — a saved global mode would eventually feed Hebrew text to a translate-to-Hebrew prompt. |
| On failure | **Show an error** (see §4). |
| The English original | **Hebrew replaces it.** Deliberate loss; re-transcription is the only way back. |

### Non-goals (v1)

Live-dictation translation · meeting/recording modes (§2.1) · Hebrew→English or any other pair · preserving both texts · translated SRT with original timings · auto-detecting that audio is English · **chunking long transcripts** (§4.3).

---

## 2. Architecture

```
batch view, "בחר קבצים" flow only:  ☑ תרגם לעברית
        │
        ├─ pre-flight guard: local engine + ivrit-* model → refuse, explain  (§5.2)
        ├─ transcribe_file(..., language: "en")   [batch currently hardcodes "he" — §5.1]
        │
        ▼
   English text + segments
        │
        ▼
   translate_text  (NEW command)  ──►  enhance_inner(text, EnToHe, groq_key, 60s)
        │
        ▼
   Hebrew text replaces `transcript`; `translated: true`   (§5.3)
```

### 2.1 Which flows the checkbox covers — and which it deliberately doesn't

The batch view drives three different capture paths. **v1 covers only the first:**

| Flow | In v1? | Why |
|---|---|---|
| **"בחר קבצים"** — pick existing audio/video files | ✅ **Yes** | This *is* the stated use case: English interviews, videos, podcasts. |
| **"הקלט ותמלל"** with Mic / System | ❌ No | Recording English *through the app* to translate it is an edge case; excluding it costs nothing and keeps v1 one flow. |
| **Meeting modes** (`stop_call_recording`, CallCloud/CallLocal) | ❌ No | These bypass `transcribe_file` entirely (`App.tsx:1112-1119`) and produce text with **`"אני: "` / `"הצד השני: "` labels baked in as plain content**. An LLM translating that would very likely mangle the speaker prefixes, since they are not structured metadata to it. Translating meetings needs its own design. |

The checkbox is therefore rendered only in the file-picking flow, not next to the record button.

---

## 3. `enhance.rs` — a new mode

Add an `EnToHe` variant alongside `HeGeneral`. The `ChatMessage` shape, `build_messages`, `enhance_inner` and `EnhanceError` are reused.

- `EnhanceMode::from_str("en_to_he")` → `EnToHe`. **Unknown strings must still fall back to `HeGeneral`** — load-bearing back-compat with an existing test.
- `system_prompt()` for `EnToHe`: translate the user message English→Hebrew; output **only** the translation — no preamble, quotes, or addressing the user; natural written Hebrew, not word-for-word; preserve meaning, tone and register; drop speech disfluencies; never answer or comment on the content.
- `example_raw()` / `example_clean()`: a messy English transcript → clean Hebrew. Anchors the model to "transform the user message", not "reply to it" — same rationale as `HeGeneral`'s one-shot.

### 3.1 ⚠️ The 2× output guard must NOT be reused as-is

`validate_output` rejects output longer than `raw.chars().count() * 2` (`enhance.rs:120`). That ceiling is calibrated for cleanup, where output ≈ input. **For translation it rejects correct results on short input:**

| English input | chars | Correct Hebrew | chars | Ratio |
|---|---|---|---|---|
| `AI` | 2 | `בינה מלאכותית` | 13 | **6.5×** |
| `USA` | 3 | `ארצות הברית` | 11 | **3.7×** |

Short snippets are exactly what a user tries first, and §4 makes failures loud — so this would greet the feature with a confusing "suspicious output" error on its first use.

**Fix — make the ceiling mode-owned, leaving `HeGeneral` byte-identical:**

```rust
impl EnhanceMode {
    /// Max output length before we call the result a hallucination.
    fn max_output_chars(&self, raw_chars: usize) -> usize {
        match self {
            // Cleanup output ≈ input. Unchanged from the original guard.
            EnhanceMode::HeGeneral => raw_chars * 2,
            // Translation legitimately expands short input ("AI" → "בינה מלאכותית"),
            // so a pure multiple is wrong. An additive floor covers short strings
            // while the multiple still catches a runaway on long ones.
            EnhanceMode::EnToHe => (raw_chars * 3).max(raw_chars + 200),
        }
    }
}
```
`validate_output` takes the mode and uses this ceiling. Empty output stays rejected for both.

---

## 4. `translate_text` — a separate command, with inverted failure semantics

**Why not reuse `enhance_text`:** it re-reads `enhance_enabled` from settings (`lib.rs:336-352` — *"so a stale frontend can't force enhancement"*), so a user with Smart Cleanup **off** could not translate. That gate exists to prevent text reaching Groq *silently*; translation is never silent — the per-file checkbox **is** the consent.

```rust
#[tauri::command]
async fn translate_text(state: State<'_, AppState>, text: String) -> Result<String, String>
```
- Requires a Groq key; clear Hebrew error if absent.
- Calls `enhance::enhance_inner(&text, EnhanceMode::EnToHe, &key, TRANSLATE_TIMEOUT)`.
- **Propagates errors — no silent fallback to raw text.**

**The inverted fallback is the point.** Cleanup failing silently is benign — slightly less polished Hebrew. **Translation failing silently returns English where Hebrew was requested**, indistinguishable from "the checkbox did nothing".

### 4.1 The 10s timeout must not be reused

`enhance_inner` hardcodes `Duration::from_secs(10)` (`enhance.rs:153`), calibrated for a short dictation utterance. Batch files are far longer, and per §4 every slow translation would surface as a hard error. Make the timeout a parameter: `enhance_text` keeps **10s**; `translate_text` uses **60s**.

### 4.2 Input-length guard (prevents a confusing far-away failure)

A long transcript can exceed the model's context window, which surfaces as an opaque Groq API error. v1 refuses up-front instead: if the transcript exceeds **30,000 characters**, return a clear Hebrew error saying the transcript is too long to translate in one pass. Chunking is a v2 (§Non-goals).

---

## 5. Frontend

A `☑ תרגם לעברית` checkbox in the file-picking flow (§2.1).

### 5.1 Source language — batch currently hardcodes Hebrew

**Correction to an earlier draft of this spec:** the batch path does **not** read the global `language` setting. It passes `language: "he"` literally (`App.tsx:1006`, and `1114`/`1118` for the recording paths). The global setting flows only to live dictation (`languageRef.current`, `App.tsx:544`).

So there is no "two knobs must agree" hazard. The change is simply: when the box is ticked, pass **`language: "en"`** instead of the hardcoded `"he"` for that run. `BatchOpts.language` is already a real per-call field (`batch.rs`), so nothing new is needed backend-side.

### 5.2 ⚠️ Pre-flight guard: ivrit-* models ignore the language argument

`whisper.rs` (`transcribe` ~82-86, `run_long_transcription` ~187-192) forces `effective_lang = "he"` whenever the model name starts with `ivrit-`, **regardless of the language passed in**. So on the local engine with an ivrit model, the `"en"` override is silently discarded: English audio is decoded with a forced Hebrew token (garbled), and the garbage is then fed to the translator — producing a plausible-looking but wrong Hebrew result rather than a clean failure.

**Guard before transcription starts:** if the box is ticked while the engine is local *and* the selected model is `ivrit-*`, refuse with a clear Hebrew error explaining the model is Hebrew-locked and to switch models or use the cloud engine. This mirrors the existing pre-record model guard used by `CallLocal`.

### 5.3 ⚠️ SRT eligibility

`isSrtEligible` (`App.tsx:1159-1160`) is `status === "done" && !edited && segments.length > 0`. The `edited` flag exists because hand-editing desynchronizes the transcript from `segments`. **Translation is exactly that desynchronization**: `segments` hold English words and timings while `transcript` becomes Hebrew. Left alone, "🎬 SRT" would export **English subtitles under a Hebrew transcript**, and the combined multi-file export (`App.tsx:1217`, which filters on the same predicate) would too.

Fix: add `translated?: boolean` to `BatchResult`, set it on successful translation, and extend `isSrtEligible` to require `!translated`. Both the per-item and combined export paths inherit the fix, since both filter through that one predicate.

**Do not reuse `edited`** — the user edited nothing; the flag would lie to every future reader.

**TXT / Word exports stay available:** they export the transcript text itself, which is legitimately the Hebrew translation. Only SRT is wrong, because only SRT reads `segments`.

---

## 6. Error handling

| Failure | Behavior |
|---|---|
| Local engine + `ivrit-*` model | Refused **before** transcription (§5.2); nothing is spent. |
| Transcript > 30,000 chars | Clear error (§4.2); transcript stays English. |
| No Groq key | Clear Hebrew error; transcript stays English. |
| Groq 401/403/429/timeout(60s)/network | Existing `EnhanceError` Display strings surface; transcript stays English. |
| Empty or over-ceiling output | `validate_output` rejects (§3.1); transcript stays English. |
| Transcription itself fails | Unchanged — translation never runs. |

In every failure the item remains a normal, **SRT-eligible English result**. Failure is always visible and never destructive.

---

## 7. Testing

**Rust (TDD):**
- `from_str("en_to_he")` → `EnToHe`; unknown → `HeGeneral` (back-compat guard).
- `build_messages(EnToHe, …)` → 4 messages, translate system prompt, English example as `user`, Hebrew example as `assistant`, real text **last**.
- `max_output_chars`: `HeGeneral` still returns exactly `raw*2` (no behavior change); `EnToHe` admits the `"AI"` → `"בינה מלאכותית"` case (13 ≤ ceiling for raw=2) and still rejects a runaway on long input.
- `validate_output` with `EnToHe` accepts that short-input expansion and rejects empty.

**Frontend:**
- `isSrtEligible` is false when `translated` is true, and still true for an untranslated done result.

---

## 8. Acceptance criteria

- An English file with the box ticked yields a **Hebrew** transcript; unticked, English.
- A short English clip (e.g. one sentence naming a tool) translates without a "suspicious output" error — the §3.1 regression.
- A translated item offers **no** SRT export (per-item *and* combined); TXT/Word still work; an untranslated one still offers SRT.
- Smart Cleanup **off** does not block translation.
- Ticking the box on the local engine with an `ivrit-*` model refuses up-front with an actionable message.
- Any failure leaves a visible error and an intact English transcript.
- The existing **63 passing / 1 ignored** stay green; `cargo build` no new warnings; `tsc && vite build` clean.
