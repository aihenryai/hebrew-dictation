# English → Hebrew Translation — Design Spec

- **Date:** 2026-07-19
- **Status:** Draft (pending spec-review loop + user approval)
- **Backlog origin:** HANDOFF "Not done — flagged for future": *English → Hebrew translation (upload/dictate in English, auto-translate to Hebrew) — Henry-confirmed roadmap idea from 2026-07-02, not started.*

---

## 1. Problem & scope

Henry consumes English AI/tech content and produces Hebrew. He wants to hand the app an English recording and get a **Hebrew** transcript.

**What already exists (verified in code, not assumed):**
- **English transcription already works.** `VALID_LANGUAGES` (`api_transcribe.rs:486`) includes `"en"`, and the `language` setting flows to both Deepgram and Groq.
- **An LLM post-processing path already exists** — Smart Cleanup (`enhance.rs`): Groq `llama-3.3-70b-versatile`, with pure unit-tested `build_messages` / `validate_output` helpers and a caller that falls back to raw text on any error.
- **That path explicitly forbids translation.** The `HeGeneral` system prompt's rule 4 reads *"אל תוסיף מידע, אל תקצר משמעותית, **אל תתרגם**"*.

So this feature is **not** "transcribe English" — it is **what happens to the text after transcription**, and it is a new `EnhanceMode` variant plus its plumbing, not a new subsystem.

### Scope decisions (Henry, 2026-07-19)

| Decision | Choice | Consequence |
|---|---|---|
| Which paths | **Files/batch only in v1.** Live dictation deferred. | Removes all streaming complexity — no fragment problem, no new hotkey, no toolbar UI. |
| Where the user chooses | **Per-action checkbox**, not a persistent setting. | Avoids the "translate mode left on + Hebrew speech" hazard: a saved global mode would feed Hebrew text to a translate-to-Hebrew prompt. |
| On failure | **Show an error.** | Unlike cleanup — see §4. |
| The English original | **Hebrew replaces it.** | Deliberate data loss; re-transcription is the only way back. |

### Non-goals (v1)

Live-dictation translation · Hebrew→English or any other pair · preserving both texts side by side · translated SRT with original timings · auto-detecting that audio is English.

---

## 2. Architecture

Three small pieces. No new files.

```
batch view: ☑ תרגם לעברית
        │
        ├─ forces this file's transcription language to "en"   (§5.1)
        │
        ▼
  transcribe_file  ──►  English text + segments
        │
        ▼
  translate_text (NEW command, lib.rs)
        │  └─ enhance::enhance_inner(text, EnhanceMode::EnToHe, groq_key)
        ▼
  Hebrew text  ──►  replaces `transcript`, sets `translated: true`  (§5.2)
```

---

## 3. `enhance.rs` — a new mode

Add an `EnToHe` variant alongside `HeGeneral`. Everything else in the file (the `ChatMessage` shape, `build_messages`, `validate_output`, `enhance_inner`, the error enum) is reused unchanged.

- `EnhanceMode::from_str("en_to_he")` → `EnToHe`. **Unknown strings must still fall back to `HeGeneral`** — that back-compat behavior is load-bearing for existing settings files and has an existing test.
- `system_prompt()` for `EnToHe`: translate the user message from English to Hebrew; output **only** the translation — no preamble, no quotes, no addressing the user; natural written Hebrew, not word-for-word; preserve meaning, tone and register; drop speech disfluencies (translation subsumes cleanup, so no second pass is needed); never answer or comment on the content.
- `example_raw()` / `example_clean()` for `EnToHe`: a messy English transcript → clean Hebrew. The one-shot exists for the same reason as `HeGeneral`'s: it anchors the model to "transform the user message" rather than "reply to it".

**`validate_output`'s 2× guard is reused as-is.** It rejects output longer than 2× the input's char count. Hebrew renders the same content in *fewer* characters than English (no written vowels), so an EN→HE translation sits comfortably under the ceiling; the guard still catches a runaway hallucination. A test pins this with realistic EN/HE lengths so a future prompt change can't silently drift past it.

---

## 4. `translate_text` — a separate command, with inverted failure semantics

**Why not reuse `enhance_text`:** that command deliberately re-reads `enhance_enabled` from settings (`lib.rs:336` — *"so a stale frontend can't force enhancement"*). Routing translation through it would mean a user with Smart Cleanup **off** cannot translate. The gate's purpose is to prevent text being sent to Groq *silently*; translation is never silent — it is an explicit per-file checkbox, which **is** the consent. So translation gets its own command and its own gate: a configured Groq key.

```rust
#[tauri::command]
async fn translate_text(state: State<'_, AppState>, text: String) -> Result<String, String>
```
- Reads the Groq key from settings; returns a clear Hebrew error if absent.
- Calls `enhance::enhance_inner(&text, EnhanceMode::EnToHe, &key)`.
- **Propagates errors — it does NOT fall back to the raw text.**

**The inverted fallback is the point.** Cleanup failing silently is benign: you get slightly less polished Hebrew. **Translation failing silently returns English where Hebrew was requested** — indistinguishable from "the checkbox did nothing". So the error surfaces and the English stays visibly untranslated.

---

## 5. Frontend — the checkbox and its two couplings

A `☑ תרגם לעברית` checkbox in the batch view, applying to the files transcribed in that run.

### 5.1 Source-language coupling (must not be left to the user)

Ticking the box transcribes those files as **`"en"`**, overriding the global `language` setting for that run. Without this, a user whose `language` is `"he"` feeds English audio to a Hebrew model, gets gibberish, and then translates the gibberish. Two knobs that must silently agree is a defect, not a feature.

### 5.2 SRT eligibility (a real break, caught in review)

`isSrtEligible` (`App.tsx:1159-1160`) is `status === "done" && !edited && segments.length > 0`. The `edited` flag exists because hand-editing the transcript desynchronizes it from `segments`. **Translation is exactly that desynchronization**: `segments` hold English words and timings while `transcript` becomes Hebrew. Left alone, "🎬 SRT" would export **English subtitles under a Hebrew transcript**.

Fix: add `translated?: boolean` to `BatchResult`, set it on a successful translation, and extend `isSrtEligible` to require `!translated`.

**Do not reuse `edited` for this.** The user did not edit anything; the flag would be a lie to every future reader. (Translated SRT with preserved timings is a legitimate v2 — it needs per-segment translation, which is out of scope here.)

---

## 6. Error handling

| Failure | Behavior |
|---|---|
| No Groq key | Clear Hebrew error; transcript stays English, `translated` stays false. |
| Groq 401/403/429/timeout/network | The existing `EnhanceError` Display strings surface to the user; transcript stays English. |
| Empty or >2× output | `validate_output` rejects → error surfaces; transcript stays English. |
| Transcription itself fails | Unchanged — translation never runs. |

In every failure the item remains a normal, SRT-eligible English result. Failure is always visible and never destructive.

---

## 7. Testing

**Rust (TDD):**
- `from_str("en_to_he")` → `EnToHe`; unknown → `HeGeneral` (back-compat regression guard).
- `build_messages(EnToHe, …)` → 4 messages, translate system prompt, English example as `user`, Hebrew example as `assistant`, the real text **last**.
- `validate_output` accepts a realistic EN→HE pair (Hebrew shorter than 2× the English) and still rejects a runaway.

**Frontend:**
- `isSrtEligible` returns false when `translated` is true (and stays true for an untranslated done result).

---

## 8. Acceptance criteria

- An English audio file with the box ticked produces a **Hebrew** transcript; with it unticked, English — regardless of the global `language` setting.
- A translated item offers **no** SRT export; an untranslated one still does.
- Smart Cleanup **off** does not block translation.
- Any translation failure leaves a visible error and an intact English transcript.
- `cargo test` green (existing 63 + the new ones); `cargo build` no new warnings; `tsc && vite build` clean.
