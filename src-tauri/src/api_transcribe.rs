use crate::settings::ApiProvider;
use reqwest::multipart;
use std::fmt;
use std::time::Duration;

/// Convert f32 samples (16kHz mono) to a WAV byte buffer (PCM16).
pub(crate) fn samples_to_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let num_samples = samples.len();
    let bytes_per_sample: u16 = 2;
    let num_channels: u16 = 1;
    let data_size = (num_samples * bytes_per_sample as usize) as u32;
    // RIFF ChunkSize = 4("WAVE") + 24(fmt chunk) + 8(data header) + data_size
    let file_size = 36 + data_size;

    let mut buf = Vec::with_capacity(44 + data_size as usize);

    // RIFF header
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&file_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    // fmt chunk
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    buf.extend_from_slice(&num_channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * num_channels as u32 * bytes_per_sample as u32;
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    let block_align = num_channels * bytes_per_sample;
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&(bytes_per_sample * 8).to_le_bytes());

    // data chunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());

    for &sample in samples {
        let clamped = (sample * 32768.0).clamp(-32768.0, 32767.0) as i16;
        buf.extend_from_slice(&clamped.to_le_bytes());
    }

    buf
}

/// Categorized API error — kept stable across UI string changes so callers like
/// `test_api_key` can branch on the *kind* of failure, not on a Hebrew substring
/// (which historically caused false positives — see v2.8.0 bug report).
#[derive(Debug, Clone)]
pub enum ApiError {
    /// 401 / 403 — bad or missing key. The key itself should be considered invalid.
    Unauthorized,
    /// 402 — provider says the account is out of credit / requires billing.
    /// The KEY is still valid.
    InsufficientCredit,
    /// 429 — rate limit exceeded. Key is valid.
    RateLimited,
    /// 400 — request was malformed (often: silent or empty audio). Key is valid.
    BadRequest(String),
    /// Network-level failure (no internet, DNS, TLS handshake etc.). Key validity unknown.
    Network(String),
    /// Request timed out. Key validity unknown.
    Timeout,
    /// 5xx — provider server error. Key is presumed valid (we just couldn't reach service).
    Server,
    /// Anything else (4xx not covered above, JSON parse errors, multipart issues, etc.).
    /// Key validity unknown — we don't claim either way.
    Other(String),
}

impl ApiError {
    /// Whether this error means the key itself is bad.
    /// Used by `test_api_key` to decide whether to fail validation.
    pub fn is_key_problem(&self) -> bool {
        matches!(self, ApiError::Unauthorized)
    }

    /// Whether this error means we couldn't reach the service at all.
    /// `test_api_key` surfaces this to the user since it's not a key validity verdict.
    pub fn is_network_problem(&self) -> bool {
        matches!(self, ApiError::Network(_) | ApiError::Timeout)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Unauthorized => {
                write!(f, "מפתח ה-API לא תקין — עדכן אותו בהגדרות")
            }
            ApiError::InsufficientCredit => write!(
                f,
                "נגמר הקרדיט אצל ספק ה-API — החלף ספק בהגדרות או הוסף קרדיט בלוח הבקרה"
            ),
            ApiError::RateLimited => write!(f, "חרגת ממגבלת השימוש — נסה שוב בעוד רגע"),
            ApiError::BadRequest(body) => write!(
                f,
                "בקשה לא תקינה ל-API — ייתכן שההקלטה ריקה או קצרה מדי ({})",
                body
            ),
            ApiError::Network(detail) => {
                write!(f, "אין חיבור לאינטרנט — בדוק את החיבור ונסה שוב ({})", detail)
            }
            ApiError::Timeout => {
                write!(f, "פג תוקף הבקשה — נסה הקלטה קצרה יותר או בדוק את החיבור")
            }
            ApiError::Server => {
                write!(f, "שרת ה-API לא זמין כרגע — נסה שוב בעוד רגע")
            }
            ApiError::Other(s) => write!(f, "{}", s),
        }
    }
}

fn classify_request_error(e: &reqwest::Error) -> ApiError {
    if e.is_timeout() {
        ApiError::Timeout
    } else if e.is_connect() {
        ApiError::Network(e.to_string())
    } else {
        ApiError::Other(format!("שגיאת רשת: {}", e))
    }
}

fn classify_status(status: reqwest::StatusCode, body: &str) -> ApiError {
    match status.as_u16() {
        401 | 403 => ApiError::Unauthorized,
        402 => ApiError::InsufficientCredit,
        429 => ApiError::RateLimited,
        400 => ApiError::BadRequest(truncate_body(body)),
        500..=599 => ApiError::Server,
        _ => ApiError::Other(format!(
            "שגיאת API ({}): {}",
            status.as_u16(),
            truncate_body(body)
        )),
    }
}

fn truncate_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() > 160 {
        let mut t: String = trimmed.chars().take(160).collect();
        t.push('…');
        t
    } else {
        trimmed.to_string()
    }
}

// ── Groq Whisper Turbo API ──
// OpenAI-compatible endpoint, much cheaper (~$0.04/hr vs Deepgram $4/hr).
// Batch only — no streaming support.

async fn transcribe_groq_inner(
    samples: &[f32],
    api_key: &str,
    language: &str,
) -> Result<String, ApiError> {
    let wav_data = samples_to_wav(samples, 16000);

    let file_part = multipart::Part::bytes(wav_data)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| ApiError::Other(format!("Failed to create multipart: {}", e)))?;

    let mut form = multipart::Form::new()
        .part("file", file_part)
        .text("model", "whisper-large-v3-turbo")
        .text("response_format", "json");

    // Groq accepts ISO-639-1 codes. "auto" / "multi" → omit to let Whisper auto-detect.
    if language != "auto" && language != "multi" {
        form = form.text("language", language.to_string());
    }

    let response = reqwest::Client::new()
        .post("https://api.groq.com/openai/v1/audio/transcriptions")
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| classify_request_error(&e))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(classify_status(status, &body));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| ApiError::Other(format!("Failed to parse Groq response: {}", e)))?;

    let transcript = body["text"].as_str().unwrap_or("").trim().to_string();
    Ok(transcript)
}

// ── Deepgram Nova-3 API ──

async fn transcribe_deepgram_inner(
    samples: &[f32],
    api_key: &str,
    language: &str,
) -> Result<String, ApiError> {
    let wav_data = samples_to_wav(samples, 16000);

    // "auto" → default to Hebrew (single-language). "multi" → Nova-3 code-switching (Hebrew+English mid-sentence).
    let lang = if language == "auto" { "he" } else { language };
    let url = format!(
        "https://api.deepgram.com/v1/listen?model=nova-3&language={}&smart_format=true&punctuate=true",
        lang
    );

    let response = reqwest::Client::new()
        .post(&url)
        .header("Authorization", format!("Token {}", api_key))
        .header("Content-Type", "audio/wav")
        .body(wav_data)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| classify_request_error(&e))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(classify_status(status, &body));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| ApiError::Other(format!("Failed to parse Deepgram response: {}", e)))?;

    let transcript = body["results"]["channels"][0]["alternatives"][0]["transcript"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();

    Ok(transcript)
}

/// Long-file Deepgram request for batch transcription. The caller supplies a
/// client with a long (e.g. 900s) timeout; this fn does NOT set its own timeout.
/// Uses paragraph formatting for readable long-meeting output, falling back to the
/// flat transcript. `language` should be "he" — Deepgram nova-3 multilingual does
/// NOT include Hebrew (spec §14.1-A), so never pass "multi" for Hebrew.
pub(crate) async fn transcribe_deepgram_batch(
    client: &reqwest::Client,
    samples: &[f32],
    api_key: &str,
    language: &str,
) -> Result<(String, Vec<crate::srt::TimedSegment>), ApiError> {
    let wav_data = samples_to_wav(samples, 16000);
    let lang = if language == "auto" { "he" } else { language };
    let url = format!(
        "https://api.deepgram.com/v1/listen?model=nova-3&language={}&smart_format=true&punctuate=true&paragraphs=true",
        lang
    );

    let response = client
        .post(&url)
        .header("Authorization", format!("Token {}", api_key))
        .header("Content-Type", "audio/wav")
        .body(wav_data)
        .send()
        .await
        .map_err(|e| classify_request_error(&e))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(classify_status(status, &body));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| ApiError::Other(format!("Failed to parse Deepgram response: {}", e)))?;

    let alt = &body["results"]["channels"][0]["alternatives"][0];
    // paragraphs=true gives a newline-formatted transcript; fall back to the flat one.
    let transcript = alt["paragraphs"]["transcript"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| alt["transcript"].as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    // words[] is present by default (no extra request param needed), each
    // {word, start, end, punctuated_word?} with start/end in fractional seconds.
    let words: Vec<crate::srt::TimedWord> = alt["words"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|w| {
                    let text = w["punctuated_word"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .or_else(|| w["word"].as_str())?;
                    let start = w["start"].as_f64()?;
                    let end = w["end"].as_f64()?;
                    Some(crate::srt::TimedWord {
                        text: text.to_string(),
                        start_ms: (start * 1000.0).round() as u64,
                        end_ms: (end * 1000.0).round() as u64,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let segments = crate::srt::chunk_words_to_cues(
        &words,
        crate::srt::SRT_MAX_WORDS_PER_CUE,
        crate::srt::SRT_MAX_MS_PER_CUE,
    );

    Ok((transcript, segments))
}

// ── Unified entry point ──

/// Languages accepted by the transcription APIs.
/// "multi" enables Deepgram Nova-3 code-switching (Hebrew+English mid-sentence).
const VALID_LANGUAGES: &[&str] = &[
    "auto", "multi", "he", "en", "ar", "fr", "ru", "es", "de", "it", "pt", "ja", "ko", "zh",
];

fn validate_language(language: &str) -> Result<&str, String> {
    if VALID_LANGUAGES.contains(&language) {
        Ok(language)
    } else {
        Err(format!("שפה לא נתמכת: {}", language))
    }
}

async fn transcribe_api_inner(
    provider: &ApiProvider,
    samples: &[f32],
    api_key: &str,
    language: &str,
) -> Result<String, ApiError> {
    let lang = validate_language(language).map_err(ApiError::Other)?;
    match provider {
        ApiProvider::Deepgram => transcribe_deepgram_inner(samples, api_key, lang).await,
        ApiProvider::Groq => transcribe_groq_inner(samples, api_key, lang).await,
    }
}

pub async fn transcribe_api(
    provider: &ApiProvider,
    samples: &[f32],
    api_key: &str,
    language: &str,
) -> Result<String, String> {
    transcribe_api_inner(provider, samples, api_key, language)
        .await
        .map_err(|e| e.to_string())
}

/// Verify that the API key works for the given provider.
///
/// Returns Err **only** when we have positive evidence the key is bad, or when
/// we couldn't reach the network at all. Other errors (400 because we sent
/// silent audio, 402 insufficient credit, 429 rate-limit, 5xx) are treated as
/// "key is valid, the issue is elsewhere" — these are surfaced naturally on the
/// real recording path and shouldn't fail the test button.
///
/// Bug fix (v2.8.0 → v2.8.1): the previous implementation matched on English
/// substrings ("API key invalid", "Cannot connect") that never appeared because
/// the runtime error messages were Hebrew. The result was that ANY 4xx/5xx
/// status was reported as ✓ valid — so users could enter a bogus key, see ✓,
/// and only discover the truth when they tried to actually record.
pub async fn test_api_key(provider: &ApiProvider, api_key: &str) -> Result<(), String> {
    let silence = vec![0.0f32; 8000]; // 0.5s at 16kHz
    match transcribe_api_inner(provider, &silence, api_key, "he").await {
        Ok(_) => Ok(()),
        Err(e) if e.is_key_problem() || e.is_network_problem() => Err(e.to_string()),
        Err(_) => Ok(()),
    }
}
