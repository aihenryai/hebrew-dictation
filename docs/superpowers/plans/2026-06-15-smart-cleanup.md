# Smart Cleanup (רישוף חכם) Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in, fail-safe post-transcription layer that cleans Hebrew dictation text via Groq Llama before injection.

**Architecture:** A new isolated `enhance.rs` module exposes pure, testable helpers (`build_messages`, `validate_output`) plus an async Groq chat call. A `enhance_text` Tauri command sits between the existing `transcribe` and `inject_text` in the frontend's `stopAndTranscribe`. Any failure falls back to the raw transcript (frontend holds it). Two new `#[serde(default)]` settings fields gate the feature; default OFF.

**Tech Stack:** Rust (Tauri 2, reqwest, serde_json), React/TypeScript frontend.

**Spec:** `docs/superpowers/specs/2026-06-15-smart-cleanup-design.md`

---

## Chunk 1: Backend — enhance module, settings, command

### Task 1: `enhance.rs` pure core + EnhanceMode + EnhanceError

**Files:**
- Create: `src-tauri/src/enhance.rs`
- Test: same file, `#[cfg(test)]` module

- [ ] **Step 1: Write failing tests** for the pure helpers.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn he_general_builds_system_and_user() {
        let msgs = build_messages(EnhanceMode::HeGeneral, "אהה כאילו שלום");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert!(msgs[0].content.contains("עורך"));        // Hebrew editor prompt
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[1].content, "אהה כאילו שלום");
    }

    #[test]
    fn unknown_mode_falls_back_to_default() {
        assert_eq!(EnhanceMode::from_str("nope"), EnhanceMode::HeGeneral);
        assert_eq!(EnhanceMode::from_str("he_general"), EnhanceMode::HeGeneral);
    }

    #[test]
    fn validate_rejects_empty() {
        assert!(matches!(validate_output("raw", "   "), Err(EnhanceError::Empty)));
    }

    #[test]
    fn validate_rejects_too_long() {
        // output > raw.chars * 2 → Suspicious
        let out = "a".repeat(21);
        assert!(matches!(validate_output("0123456789", &out), Err(EnhanceError::Suspicious)));
    }

    #[test]
    fn validate_accepts_and_trims_normal() {
        assert_eq!(validate_output("שלום שלום", "  שלום  ").unwrap(), "שלום");
    }
}
```

- [ ] **Step 2: Run, verify fail.** `cd src-tauri && cargo test enhance::` → FAIL (module/items missing).

- [ ] **Step 3: Minimal implementation** (top of `enhance.rs`):

```rust
use serde::Serialize;
use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnhanceMode {
    HeGeneral,
}

impl EnhanceMode {
    /// Unknown / legacy strings fall back to the default — mirrors the
    /// ApiProvider deserializer (settings.rs) for back-compat.
    pub fn from_str(s: &str) -> Self {
        match s {
            "he_general" => EnhanceMode::HeGeneral,
            _ => EnhanceMode::HeGeneral,
        }
    }

    fn system_prompt(&self) -> &'static str {
        match self {
            EnhanceMode::HeGeneral =>
                "אתה עורך לשוני לעברית. קלט: תמלול דיבור גולמי. פלט: אותו טקסט כטקסט כתוב נקי. \
הסר מילות מילוי (אהה, אמ, יעני, כאילו), חזרות וגמגומים. תקן פיסוק ורווחים. \
שמור בדיוק על המשמעות, הטון והשפה של הדובר. אל תוסיף מידע, אל תקצר משמעותית, אל תתרגם, \
אל תענה לתוכן — ערוך בלבד. החזר אך ורק את הטקסט הערוך, בלי הקדמות, הסברים או מירכאות.",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

pub fn build_messages(mode: EnhanceMode, text: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage { role: "system".into(), content: mode.system_prompt().into() },
        ChatMessage { role: "user".into(), content: text.to_string() },
    ]
}

#[derive(Debug, Clone)]
pub enum EnhanceError {
    Unauthorized,
    RateLimited,
    Network(String),
    Timeout,
    Empty,
    Suspicious,
    Other(String),
}

impl fmt::Display for EnhanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnhanceError::Unauthorized => write!(f, "מפתח Groq לא תקין לרישוף — עדכן אותו בהגדרות"),
            EnhanceError::RateLimited => write!(f, "חרגת ממגבלת השימוש ברישוף — נסה שוב בעוד רגע"),
            EnhanceError::Network(d) => write!(f, "אין חיבור לשירות הרישוף ({})", d),
            EnhanceError::Timeout => write!(f, "פג תוקף בקשת הרישוף"),
            EnhanceError::Empty => write!(f, "הרישוף החזיר טקסט ריק"),
            EnhanceError::Suspicious => write!(f, "תוצאת הרישוף חשודה — מוחזר הטקסט המקורי"),
            EnhanceError::Other(s) => write!(f, "{}", s),
        }
    }
}

/// Hallucination guard. Empty → Empty. Output longer than 2× the raw char
/// count → Suspicious. Otherwise return the trimmed output. Fixed threshold
/// (not configurable) so the unit test is deterministic.
pub fn validate_output(raw: &str, output: &str) -> Result<String, EnhanceError> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Err(EnhanceError::Empty);
    }
    if trimmed.chars().count() > raw.chars().count() * 2 {
        return Err(EnhanceError::Suspicious);
    }
    Ok(trimmed.to_string())
}
```

- [ ] **Step 4: Run, verify pass.** `cargo test enhance::` → PASS (5 tests).

- [ ] **Step 5: Commit.** `git add src-tauri/src/enhance.rs && git commit -m "feat(enhance): pure core — modes, prompt, output guard"`

### Task 2: `enhance_inner` — Groq chat call

**Files:**
- Modify: `src-tauri/src/enhance.rs`

- [ ] **Step 1: Implement** the async call (no network unit test — exercised via manual smoke + the command path). Mirror `api_transcribe.rs` patterns (reqwest, Bearer, 30s→use 10s timeout, status classification).

```rust
fn classify_status(status: reqwest::StatusCode) -> EnhanceError {
    match status.as_u16() {
        401 | 403 => EnhanceError::Unauthorized,
        429 => EnhanceError::RateLimited,
        _ => EnhanceError::Other(format!("שגיאת רישוף ({})", status.as_u16())),
    }
}

/// Run cleanup on `text` via Groq chat. Returns the validated enhanced text,
/// or an EnhanceError. Caller falls back to the raw text on any Err.
pub async fn enhance_inner(
    text: &str,
    mode: EnhanceMode,
    api_key: &str,
) -> Result<String, EnhanceError> {
    let messages = build_messages(mode, text);
    let payload = serde_json::json!({
        "model": "llama-3.3-70b-versatile",
        "messages": messages,
        "temperature": 0.2,
    });

    let response = reqwest::Client::new()
        .post("https://api.groq.com/openai/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&payload)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() { EnhanceError::Timeout }
            else { EnhanceError::Network(e.to_string()) }
        })?;

    let status = response.status();
    if !status.is_success() {
        return Err(classify_status(status));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| EnhanceError::Other(format!("פענוח תשובת רישוף נכשל: {}", e)))?;

    let content = body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");

    validate_output(text, content)
}
```

- [ ] **Step 2: Verify compile.** `cargo check` → OK.
- [ ] **Step 3: Commit.** `git commit -am "feat(enhance): Groq chat call with timeout + status mapping"`

### Task 3: settings fields

**Files:**
- Modify: `src-tauri/src/settings.rs` — `AppSettings` (after `audio_volume`), `RedactedSettings`, `redacted()`, `Default`.

- [ ] **Step 1:** Add to `AppSettings`:
```rust
    /// Opt-in smart cleanup of the transcript via Groq Llama before injection.
    /// Default false — preserves existing behavior + privacy.
    #[serde(default)]
    pub enhance_enabled: bool,
    /// Cleanup profile. Default "he_general". Unknown values fall back via EnhanceMode::from_str.
    #[serde(default = "default_enhance_mode")]
    pub enhance_mode: String,
```
- [ ] **Step 2:** Add `fn default_enhance_mode() -> String { "he_general".to_string() }`.
- [ ] **Step 3:** Mirror both fields into `RedactedSettings`, `redacted()`, and `Default for AppSettings`.
- [ ] **Step 4: Verify compile.** `cargo check` → OK.
- [ ] **Step 5: Commit.** `git commit -am "feat(settings): enhance_enabled + enhance_mode fields"`

### Task 4: `enhance_text` command + registration

**Files:**
- Modify: `src-tauri/src/lib.rs` — add `mod enhance;` near other `mod`s; add command; register in `invoke_handler` near `transcribe`/`inject_text` (~line 1231).

- [ ] **Step 1: Add command.** Reads `groq_api_key` **directly** (NOT `active_api_key()` — enhancement is always Groq, see spec D1/4.5).

```rust
#[tauri::command]
async fn enhance_text(
    state: State<'_, AppState>,
    text: String,
    mode: Option<String>,
) -> Result<String, String> {
    let (enabled, mode_str, api_key) = {
        let s = state.settings.lock().map_err(|e| e.to_string())?;
        (
            s.enhance_enabled,
            mode.unwrap_or_else(|| s.enhance_mode.clone()),
            s.groq_api_key.clone(),
        )
    };
    if !enabled {
        return Ok(text); // safety: command is a no-op if the feature is off
    }
    let key = api_key
        .filter(|k| !k.is_empty())
        .ok_or("מפתח Groq לא מוגדר — נדרש לרישוף")?;
    let m = enhance::EnhanceMode::from_str(&mode_str);
    enhance::enhance_inner(&text, m, &key)
        .await
        .map_err(|e| e.to_string())
}
```

- [ ] **Step 2:** `mod enhance;` added; `enhance_text` added to the `tauri::generate_handler![...]` list.
- [ ] **Step 3: Verify compile.** `cargo check` → OK.
- [ ] **Step 4: Run backend tests.** `cargo test` → all pass.
- [ ] **Step 5: Commit.** `git commit -am "feat(lib): enhance_text command + module registration"`

---

## Chunk 2: Frontend integration

### Task 5: App.tsx — wrapper, fail-safe call, toggle, indicator, privacy

**Files:**
- Modify: `src/App.tsx`

Read the current `stopAndTranscribe` (~line 409), the batch branch (`transcribe` ~456 → `injectText` ~462), the settings `RedactedSettings` TS interface, and the `has_groq_key` usages (~68/694/731) before editing.

- [ ] **Step 1:** Add TS field to the settings interface: `enhance_enabled: boolean; enhance_mode: string;`.
- [ ] **Step 2:** Add an `enhanceText` invoke wrapper:
```ts
async function enhanceText(text: string): Promise<string> {
  return await invoke<string>("enhance_text", { text, mode: null });
}
```
- [ ] **Step 3:** In `stopAndTranscribe` batch branch, between getting `text` from `transcribe` and `injectText(text)`:
```ts
let finalText = text;
if (settings.enhance_enabled && settings.has_groq_key) {
  try {
    setStatus("enhancing");           // drives toolbar "✨ משכתב…" indicator
    finalText = await enhanceText(text);
  } catch (e) {
    console.error("enhance failed, injecting raw:", e);
    finalText = text;                 // fail-safe (spec D3)
  }
}
await injectText(finalText);
```
Do NOT touch the streaming branch (~430-440) — enhancement is batch-only.
- [ ] **Step 4:** Toolbar: render "✨ משכתב…" when `status === "enhancing"`.
- [ ] **Step 5:** Settings UI: add a "✨ רישוף חכם" toggle calling `persistSettings({ enhance_enabled })`. Gate/hint on `has_groq_key` (reuse the existing pattern). When `transcription_mode === "local"` and the toggle is on, show a one-line privacy note: "הטקסט (לא ההקלטה) יישלח ל-Groq לצורך הרישוף".
- [ ] **Step 6: Commit.** `git commit -am "feat(ui): smart-cleanup toggle, fail-safe enhance call, indicator, privacy note"`

### Task 6: Build + manual smoke (with Henry)

- [ ] **Step 1:** `npm run tauri dev` (Henry's machine).
- [ ] **Step 2:** Enable the toggle, set Groq key if needed, speak a Hebrew sentence with "אהה / כאילו" + a repetition → verify clean injected text.
- [ ] **Step 3:** Mid-recording disconnect / bad key → verify the **raw** transcript is still injected (fail-safe).
- [ ] **Step 4:** Toggle off → verify original raw behavior unchanged.

---

## Notes
- DRY: reuse `reqwest` patterns from `api_transcribe.rs`; reuse the `has_groq_key` UI gate.
- YAGNI: one mode, batch-only, Groq-only. No streaming/snippets/OpenAI.
- The `enhance_text` command double-guards `enhance_enabled` so a stale frontend can't force enhancement.
