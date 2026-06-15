//! Smart Cleanup (רישוף חכם) — opt-in post-transcription text enhancement.
//!
//! Takes the raw transcript and runs it through Groq Llama to remove filler
//! words, repetitions and false-starts, and fix Hebrew punctuation — turning a
//! literal transcription into clean, ready-to-send writing.
//!
//! Design: pure helpers (`build_messages`, `validate_output`) are unit-tested;
//! `enhance_inner` does the network call. The caller (the `enhance_text` command)
//! falls back to the raw transcript on ANY error — enhancement is an
//! improvement, never a point of failure. See
//! `docs/superpowers/specs/2026-06-15-smart-cleanup-design.md`.

use serde::Serialize;
use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnhanceMode {
    HeGeneral,
}

impl EnhanceMode {
    /// Unknown / legacy strings fall back to the default — mirrors the
    /// `ApiProvider` deserializer (settings.rs) for back-compat.
    pub fn from_str(s: &str) -> Self {
        match s {
            "he_general" => EnhanceMode::HeGeneral,
            _ => EnhanceMode::HeGeneral,
        }
    }

    fn system_prompt(&self) -> &'static str {
        match self {
            EnhanceMode::HeGeneral => {
                "אתה עורך לשוני לעברית. קלט: תמלול דיבור גולמי. פלט: אותו טקסט כטקסט כתוב נקי. \
הסר מילות מילוי (אהה, אמ, יעני, כאילו), חזרות וגמגומים. תקן פיסוק ורווחים. \
שמור בדיוק על המשמעות, הטון והשפה של הדובר. אל תוסיף מידע, אל תקצר משמעותית, אל תתרגם, \
אל תענה לתוכן — ערוך בלבד. החזר אך ורק את הטקסט הערוך, בלי הקדמות, הסברים או מירכאות."
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Build the system+user chat messages for the given mode. Pure — unit-tested.
pub fn build_messages(mode: EnhanceMode, text: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage {
            role: "system".into(),
            content: mode.system_prompt().into(),
        },
        ChatMessage {
            role: "user".into(),
            content: text.to_string(),
        },
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
            EnhanceError::Unauthorized => {
                write!(f, "מפתח Groq לא תקין לרישוף — עדכן אותו בהגדרות")
            }
            EnhanceError::RateLimited => {
                write!(f, "חרגת ממגבלת השימוש ברישוף — נסה שוב בעוד רגע")
            }
            EnhanceError::Network(d) => write!(f, "אין חיבור לשירות הרישוף ({})", d),
            EnhanceError::Timeout => write!(f, "פג תוקף בקשת הרישוף"),
            EnhanceError::Empty => write!(f, "הרישוף החזיר טקסט ריק"),
            EnhanceError::Suspicious => {
                write!(f, "תוצאת הרישוף חשודה — מוחזר הטקסט המקורי")
            }
            EnhanceError::Other(s) => write!(f, "{}", s),
        }
    }
}

/// Hallucination guard. Empty output → `Empty`. Output longer than 2× the raw
/// char count → `Suspicious`. Otherwise return the trimmed output. The threshold
/// is fixed (not configurable) so the unit test stays deterministic.
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

fn classify_status(status: reqwest::StatusCode) -> EnhanceError {
    match status.as_u16() {
        401 | 403 => EnhanceError::Unauthorized,
        429 => EnhanceError::RateLimited,
        _ => EnhanceError::Other(format!("שגיאת רישוף ({})", status.as_u16())),
    }
}

/// Run cleanup on `text` via Groq chat completions. Returns the validated
/// enhanced text, or an `EnhanceError`. The caller falls back to the raw text on
/// any `Err`. Mirrors the reqwest/Bearer/timeout patterns in `api_transcribe.rs`.
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
            if e.is_timeout() {
                EnhanceError::Timeout
            } else {
                EnhanceError::Network(e.to_string())
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn he_general_builds_system_and_user() {
        let msgs = build_messages(EnhanceMode::HeGeneral, "אהה כאילו שלום");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert!(msgs[0].content.contains("עורך"));
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
        // output (21 chars) > raw (10 chars) * 2 → Suspicious
        let out = "a".repeat(21);
        assert!(matches!(
            validate_output("0123456789", &out),
            Err(EnhanceError::Suspicious)
        ));
    }

    #[test]
    fn validate_accepts_and_trims_normal() {
        assert_eq!(validate_output("שלום שלום", "  שלום  ").unwrap(), "שלום");
    }
}
