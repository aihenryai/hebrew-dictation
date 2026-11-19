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

/// Convert an **interleaved stereo** f32 buffer (L,R,L,R… at `sample_rate`) to a
/// 2-channel PCM16 WAV byte buffer. Kept deliberately SEPARATE from `samples_to_wav`
/// (which hardcodes `num_channels = 1`) so the Call-mode multichannel body can be
/// 2-channel without changing any existing mono caller (groq/deepgram single+batch).
pub(crate) fn samples_to_wav_stereo(interleaved: &[f32], sample_rate: u32) -> Vec<u8> {
    let num_samples = interleaved.len(); // total across both channels
    let bytes_per_sample: u16 = 2;
    let num_channels: u16 = 2;
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

    // interleaved is already L,R,L,R… — write straight through.
    for &sample in interleaved {
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

/// Whisper's `prompt` for Groq: a short Hebrew sentence in the style we want
/// back (punctuated, English names in Latin letters), then the user's
/// dictionary words. Measured 2026-11-19 on whisper-large-v3-turbo with the
/// app's "auto" language: Hebrew 30.4% to 29.8% WER, code-switched 24.3% to
/// 20.4%, English unchanged at 0%; near-silence came back the same with or
/// without it (no prompt leaking into the text). Groq caps the prompt at 224
/// tokens, so dictionary words stop once the prompt reaches `GROQ_PROMPT_MAX_CHARS`.
const GROQ_PROMPT_STYLE: &str =
    "שלום, זו הכתבה בעברית. היא כוללת פיסוק מלא, משפטים ברורים ומילים באנגלית כמו Google ו-Zoom.";
const GROQ_PROMPT_MAX_CHARS: usize = 200;

pub(crate) fn groq_prompt(terms: &[String]) -> String {
    let mut out = GROQ_PROMPT_STYLE.to_string();
    for (i, t) in terms.iter().enumerate() {
        let sep = if i == 0 { " " } else { ", " };
        if out.chars().count() + sep.chars().count() + t.chars().count() > GROQ_PROMPT_MAX_CHARS {
            break;
        }
        out.push_str(sep);
        out.push_str(t);
    }
    out
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
    if language != "en" {
        form = form.text("prompt", groq_prompt(&crate::dictionary::terms()));
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

/// The language code that is safe to put in a Deepgram URL.
///
/// `"he"` and `"en"` pin one language. Everything else — `"auto"` (the default
/// since 2.16.0), a legacy `"multi"`, or any other code — becomes `"multi"`,
/// Nova-3's multilingual model, which picks the language per sentence.
///
/// History: until 2.15.3 this mapped everything to `"he"`, because in 09.2026
/// `multi` did not include Hebrew and was rejected. Measured live 2026-11-10
/// with the app's own key: `multi` now transcribes Hebrew at the same quality as
/// `he` (batch and streaming), and an English sentence inside a Hebrew file came
/// out perfect — while `language=he` dropped that sentence entirely, which is
/// how an English meeting produced an empty transcript. `detect_language=true`
/// is NOT an option: it misread Hebrew as Portuguese in the same test.
///
/// Correction 2026-11-19, measured on 40 FLEURS clips: on pure Hebrew `multi`
/// makes about half again as many errors as `he` (30.6% vs 20.1%). So batch
/// dictation and file requests in "auto" now send both and keep the better one
/// (`deepgram_auto_pick`); streaming and call mode still use `multi` alone.
pub(crate) fn normalize_language(lang: &str) -> &str {
    match lang {
        "he" | "en" => lang,
        _ => "multi",
    }
}

/// True when the transcript may contain Hebrew, so the Hebrew-only fixes
/// (day-name ordinals, spoken punctuation) should run.
pub(crate) fn may_contain_hebrew(lang: &str) -> bool {
    lang != "en"
}

/// Extra query parameters every Deepgram request carries.
///
/// `language_hint` (nova-3, only valid with `language=multi`): "multi" covers
/// 55 languages, and an unclear Hebrew word came back as Persian in Arabic
/// script, Spanish, Galician or Croatian (Henry, 2026-11-17: "משרבב מילים
/// בערבית"). Measured on a real 5,141-word Hebrew meeting: 16 Spanish words
/// without the hints, none with them. The hints bias, they do not restrict,
/// so `streaming::foreign_script_only` stays as the safety net.
///
/// `mip_opt_out=true`: the audio is not kept for Deepgram's model training.
pub(crate) fn deepgram_common_params(lang: &str) -> &'static str {
    if lang == "multi" {
        "&language_hint=he&language_hint=en&mip_opt_out=true"
    } else {
        "&mip_opt_out=true"
    }
}

fn is_hebrew_or_english_tag(lang: &str) -> bool {
    lang == "he" || lang == "en"
}

/// A letter no Hebrew or English word is written with (Arabic script, "č",
/// "é"), or Spanish's inverted punctuation.
pub(crate) fn is_auto_language(lang: &str) -> bool {
    !is_hebrew_or_english_tag(lang)
}

pub(crate) fn has_foreign_letter(word: &str) -> bool {
    word.chars().any(|c| {
        c == '¿'
            || c == '¡'
            || (c.is_alphabetic() && !c.is_ascii() && !('\u{0590}'..='\u{05FF}').contains(&c))
    })
}

/// Safety net behind `language_hint`, for the multilingual model only.
/// Deepgram tags every word with the language it heard; a word tagged as
/// neither Hebrew nor English is a mishearing ("اشرح", "¿Qué?", "Čomatoti?"
/// all came out of Hebrew speech, 2026-11-17) and typing it is worse than
/// leaving it out.
///
/// Returns the transcript without those words, or `None` when nothing was
/// removed and the service's own transcript should be used as is. A foreign
/// tag on a plain ASCII word ("Henry" heard as French) is kept unless the
/// whole segment is foreign, so names are not lost.
pub(crate) fn without_foreign_words(alt: &serde_json::Value) -> Option<String> {
    let words = alt["words"].as_array()?;
    let tag = |w: &serde_json::Value| w["language"].as_str().map(str::to_owned);
    let tagged = words.iter().filter(|w| tag(w).is_some()).count();
    if tagged == 0 {
        return None;
    }
    let any_known = words
        .iter()
        .any(|w| tag(w).is_some_and(|l| is_hebrew_or_english_tag(&l)));
    let text = |w: &serde_json::Value| {
        w["punctuated_word"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| w["word"].as_str())
            .unwrap_or("")
            .to_string()
    };
    let mut removed = false;
    let kept: Vec<String> = words
        .iter()
        .filter_map(|w| {
            let word = text(w);
            let foreign = tag(w).is_some_and(|l| !is_hebrew_or_english_tag(&l));
            if foreign && (!any_known || has_foreign_letter(&word)) {
                removed = true;
                None
            } else {
                Some(word)
            }
        })
        .filter(|w| !w.is_empty())
        .collect();
    removed.then(|| kept.join(" "))
}

/// `&dictation=true` for the languages Deepgram supports it on.
///
/// Deepgram's own spoken-punctuation feature ("period" → "."). Per its docs it
/// is **English only**, which is why Hebrew has `crate::punctuation` instead.
/// Dictation paths only — someone saying "period" inside a recorded meeting
/// means the word, so this must not reach the batch or multichannel URLs.
pub(crate) fn dictation_params(lang: &str) -> &'static str {
    if lang == "en" {
        "&dictation=true"
    } else {
        ""
    }
}

/// `&keyterm=` boosts for the words a spoken language-switch command is made of.
///
/// Nova-3 Hebrew supports Keyterm Prompting (Deepgram, Feb 2026). It steers the
/// model while it listens rather than editing the output afterwards, so at worst
/// it does nothing — it cannot invent text. Added because the captured failure
/// was Deepgram mishearing "באנגלית" as "אנלית": `lang_switch` now tolerates
/// that after the fact, and this reduces how often it happens at all.
///
/// Streaming only — the switch feature is streaming-only.
pub(crate) fn language_switch_keyterm_params(lang: &str, enabled: bool) -> String {
    if !enabled {
        return String::new();
    }
    let terms: &[&str] = match lang {
        "he" => &["באנגלית", "בעברית", "אנגלית", "עברית"],
        "en" => &["Hebrew", "English"],
        _ => return String::new(),
    };
    let mut out = String::new();
    for term in terms {
        out.push_str("&keyterm=");
        out.push_str(&urlencoding_percent_encode(term));
    }
    out
}

/// Deepgram's `smart_format` misfires on Hebrew day-names: "ביום שני" ("on
/// Monday") comes back as "ביום 2º" — U+00BA MASCULINE ORDINAL INDICATOR, the
/// Spanish/Portuguese/Italian glyph for "2nd" (1º, 2º, 3º…). This is not a
/// mis-hearing — Deepgram recognized the word correctly and then reformatted
/// it into the wrong language's ordinal convention. Confirmed reproducible
/// live with Henry 2026-09-01 (session that captured this): "ביום 5º",
/// "ביום 2º", "ביום 3º", "ביום 1º" all appeared for חמישי/שני/שלישי/ראשון,
/// while the SAME words stayed correct when not directly preceded by "ביום"
/// (e.g. "וחמישי", "ורביעי" in "ביום שני וחמישי" — only the word right after
/// "ביום" gets reformatted). `replace` deterministically undoes the specific
/// wrong string Deepgram is known to emit — no guessing, no LLM, can't invent
/// text that wasn't there. Hebrew has no native use for "º" at all, so this
/// can't collide with a legitimate transcript.
/// Deepgram docs: find terms must be lowercase; digits aren't cased, so no
/// issue here. https://developers.deepgram.com/docs/find-and-replace
pub(crate) fn day_ordinal_replace_params(lang: &str) -> String {
    if !may_contain_hebrew(lang) {
        // The bug is specific to Hebrew day-names, which can appear under
        // "he" or "multi" (accepted by Deepgram with multi, tested 2026-11-10).
        return String::new();
    }
    const DAYS: [(&str, &str); 7] = [
        ("1º", "ראשון"),
        ("2º", "שני"),
        ("3º", "שלישי"),
        ("4º", "רביעי"),
        ("5º", "חמישי"),
        ("6º", "שישי"),
        ("7º", "שבת"),
    ];
    let mut out = String::new();
    for (find, replacement) in DAYS {
        out.push_str("&replace=");
        out.push_str(&urlencoding_percent_encode(find));
        out.push(':');
        out.push_str(&urlencoding_percent_encode(replacement));
    }
    out
}

/// Minimal percent-encoding for a Deepgram query-param value: UTF-8 bytes,
/// percent-escape everything outside `A-Za-z0-9-_.~`. No external crate
/// needed for the small, fixed inputs `day_ordinal_replace_params` calls this
/// with.
fn urlencoding_percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

/// Whether the multilingual transcript should win over the Hebrew one, for an
/// "auto" request that was sent to Deepgram both ways.
///
/// Measured 2026-11-19 (FLEURS Hebrew dev 40 clips, 12 code-switched and 6
/// English TTS sentences): `language=he` 20.1% WER on Hebrew but it deletes
/// English speech (100% on English); `multi` 30.6% on Hebrew, 0% on English.
/// This pick: 20.1% Hebrew, 21.4% code-switched (both alone: 30.1% / 25.2%),
/// 0% English. Multi wins only when its transcript is mostly Latin-script
/// words (English speech; a "multi" mishearing comes out in Arabic script and
/// does not count), when the Hebrew model dropped most of the speech, when it
/// fell back to niqqud spelling of foreign sounds while multi heard Latin
/// words, or when multi heard a whole English sentence (4+ Latin words in a row).
pub(crate) fn prefer_multi(he: &str, multi: &str) -> bool {
    let is_latin = |w: &str| {
        let mut letters = w.chars().filter(|c| c.is_alphabetic()).peekable();
        letters.peek().is_some() && letters.all(|c| c.is_ascii())
    };
    let multi_words: Vec<&str> = multi.split_whitespace().collect();
    if multi_words.is_empty() {
        return false;
    }
    let latin = multi_words.iter().filter(|w| is_latin(w)).count();
    if latin * 2 >= multi_words.len() {
        return true;
    }
    if he.split_whitespace().count() * 2 < multi_words.len() {
        return true;
    }
    let niqqud = he.chars().any(|c| ('\u{0591}'..='\u{05C7}').contains(&c));
    if niqqud && latin > 0 {
        return true;
    }
    let mut run = 0;
    for w in &multi_words {
        run = if is_latin(w) { run + 1 } else { 0 };
        if run >= 4 {
            return true;
        }
    }
    false
}

/// Run an "auto" request both ways at once and keep the better transcript
/// (`prefer_multi`). If one request fails the other is used; if both fail the
/// multilingual one's error is returned. Costs two Deepgram requests.
async fn deepgram_auto_pick<T, F, Fut>(run: F, text_of: fn(&T) -> &str) -> Result<T, ApiError>
where
    F: Fn(&'static str) -> Fut,
    Fut: std::future::Future<Output = Result<T, ApiError>>,
{
    let (he, multi) = tokio::join!(run("he"), run("multi"));
    match (he, multi) {
        (Ok(h), Ok(m)) => Ok(if prefer_multi(text_of(&h), text_of(&m)) { m } else { h }),
        (Ok(h), Err(_)) => Ok(h),
        (Err(_), Ok(m)) => Ok(m),
        (Err(_), Err(e)) => Err(e),
    }
}

async fn transcribe_deepgram_inner(
    samples: &[f32],
    api_key: &str,
    language: &str,
) -> Result<String, ApiError> {
    let wav_data = samples_to_wav(samples, 16000);
    if normalize_language(language) == "multi" {
        return deepgram_auto_pick(
            |lang| deepgram_dictation_once(wav_data.clone(), api_key, lang),
            |s: &String| s.as_str(),
        )
        .await;
    }
    deepgram_dictation_once(wav_data, api_key, language).await
}

async fn deepgram_dictation_once(
    wav_data: Vec<u8>,
    api_key: &str,
    language: &str,
) -> Result<String, ApiError> {
    let lang = normalize_language(language);
    let url = format!(
        "https://api.deepgram.com/v1/listen?model=nova-3&language={}&smart_format=true&punctuate=true{}{}{}{}",
        lang,
        deepgram_common_params(lang),
        day_ordinal_replace_params(lang),
        dictation_params(lang),
        crate::dictionary::keyterm_params()
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

    let alt = &body["results"]["channels"][0]["alternatives"][0];
    let cleaned = if lang == "multi" { without_foreign_words(alt) } else { None };
    let transcript = cleaned
        .unwrap_or_else(|| alt["transcript"].as_str().unwrap_or("").to_string())
        .trim()
        .to_string();

    Ok(transcript)
}

/// Long-file Deepgram request for batch transcription. The caller supplies a
/// client with a long (e.g. 900s) timeout; this fn does NOT set its own timeout.
/// Uses paragraph formatting for readable long-meeting output, falling back to the
/// flat transcript. `language` goes through `normalize_language`, so "auto"
/// becomes Deepgram's multilingual model.
pub(crate) async fn transcribe_deepgram_batch(
    client: &reqwest::Client,
    samples: &[f32],
    api_key: &str,
    language: &str,
) -> Result<(String, Vec<crate::srt::TimedSegment>), ApiError> {
    let wav_data = samples_to_wav(samples, 16000);
    if normalize_language(language) == "multi" {
        return deepgram_auto_pick(
            |lang| deepgram_batch_once(client, wav_data.clone(), api_key, lang),
            |r: &(String, Vec<crate::srt::TimedSegment>)| r.0.as_str(),
        )
        .await;
    }
    deepgram_batch_once(client, wav_data, api_key, language).await
}

async fn deepgram_batch_once(
    client: &reqwest::Client,
    wav_data: Vec<u8>,
    api_key: &str,
    language: &str,
) -> Result<(String, Vec<crate::srt::TimedSegment>), ApiError> {
    let lang = normalize_language(language);
    // diarize=true adds a per-word `speaker` index to words[]; it does not
    // change the transcript text, so it's safe to send on every batch request
    // (single-speaker audio simply reports one speaker → no SRT label).
    let url = format!(
        "https://api.deepgram.com/v1/listen?model=nova-3&language={}&smart_format=true&punctuate=true&paragraphs=true&diarize=true{}{}{}",
        lang,
        deepgram_common_params(lang),
        day_ordinal_replace_params(lang),
        crate::dictionary::keyterm_params()
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

    // words[] is present by default (no extra request param needed).
    // parse_deepgram_words converts fractional-second timings to ms and carries
    // each word's diarization `speaker` (populated by &diarize=true, above).
    let words = parse_deepgram_words(alt);
    let segments = crate::srt::chunk_words_to_cues(
        &words,
        crate::srt::SRT_MAX_WORDS_PER_CUE,
        crate::srt::SRT_MAX_MS_PER_CUE,
    );

    Ok((transcript, segments))
}

/// Parse Deepgram's `words[]` array (from an `alternatives[]` entry) into
/// `TimedWord`s: fractional-second timings become milliseconds, and — when
/// `diarize=true` populated it — the per-word `speaker` index is carried
/// through. A word with no `speaker` field yields `speaker: None`, so the
/// non-diarized path is unaffected.
pub(crate) fn parse_deepgram_words(alt: &serde_json::Value) -> Vec<crate::srt::TimedWord> {
    alt["words"]
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
                        // Deepgram's diarize=true adds a per-word `speaker`
                        // integer; absent (no diarization) → None.
                        speaker: w["speaker"].as_u64().map(|n| n as u32),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Build the Deepgram `/listen` URL for Call-mode multichannel transcription:
/// the same nova-3 base as `transcribe_deepgram_batch` but WITH
/// `multichannel=true` and deliberately WITHOUT `diarize`/`paragraphs`. Each
/// channel is transcribed independently and the labeled text is assembled from
/// the merged segments (`build_multichannel_result`), never from Deepgram's
/// per-channel flat transcript. `auto` maps to multilingual, matching the batch route.
fn multichannel_url(language: &str) -> String {
    let lang = normalize_language(language);
    format!(
        "https://api.deepgram.com/v1/listen?model=nova-3&language={}&smart_format=true&punctuate=true&multichannel=true{}{}",
        lang,
        deepgram_common_params(lang),
        day_ordinal_replace_params(lang)
    )
}

/// Turn a Deepgram *multichannel* response into `(labeled_text, merged_segments)`,
/// mirroring `transcribe_deepgram_batch`'s return shape. `diarize` is off for
/// multichannel, so `parse_deepgram_words` yields `speaker: None`; we therefore
/// stamp each channel's index as the speaker (channel 0 = "me", channel 1 =
/// "them") on every word BEFORE chunking. Both channels' cues are merged by
/// `start_ms` (they share the one stereo-file clock) and the `text` is built
/// from those merged cues — never from Deepgram's per-channel flat transcript,
/// which defaults to channel 0 and would silently drop "them".
fn build_multichannel_result(
    body: &serde_json::Value,
) -> (String, Vec<crate::srt::TimedSegment>) {
    let channels: Vec<Vec<crate::srt::TimedWord>> = (0usize..2)
        .map(|i| parse_deepgram_words(&body["results"]["channels"][i]["alternatives"][0]))
        .collect();
    merge_call_channels(channels)
}

/// Shared by both providers' call mode: stamp each channel's index as the
/// speaker (0 = "me", 1 = "them"), chunk per channel, merge by start time and
/// build the labeled text from the merged cues.
fn merge_call_channels(
    channels: Vec<Vec<crate::srt::TimedWord>>,
) -> (String, Vec<crate::srt::TimedSegment>) {
    let mut segments: Vec<crate::srt::TimedSegment> = Vec::new();
    for (channel_idx, mut words) in channels.into_iter().enumerate() {
        // diarize is off → the parser returns None; stamp the channel index
        // explicitly on each word before chunking (spec §4.4).
        for w in &mut words {
            w.speaker = Some(channel_idx as u32);
        }
        segments.extend(crate::srt::chunk_words_to_cues(
            &words,
            crate::srt::SRT_MAX_WORDS_PER_CUE,
            crate::srt::SRT_MAX_MS_PER_CUE,
        ));
    }
    // Both channels share the single stereo-file clock, so start_ms merges them
    // chronologically. Stable sort keeps channel 0 before channel 1 on ties.
    segments.sort_by_key(|s| s.start_ms);

    let text = segments
        .iter()
        .map(|seg| {
            // Single source of truth (srt::call_side_label), shared with
            // render_srt's Call arm so the injected text and the SRT prefix
            // can never drift apart.
            let label = crate::srt::call_side_label(seg.speaker.unwrap_or(1));
            format!("{}: {}", label, seg.text)
        })
        .collect::<Vec<_>>()
        .join("\n");

    (text, segments)
}

/// Multichannel (stereo) Deepgram batch request — the Call-mode core. `stereo_wav_bytes`
/// is a 2-channel / 16kHz WAV body (channel 0 = mic → "me", channel 1 = system
/// → "them"), built by `samples_to_wav_stereo` upstream. Sends `&multichannel=true`
/// and — unlike `transcribe_deepgram_batch` — NO `diarize`/`paragraphs`: each
/// channel is transcribed on its own, the channel index is stamped as the
/// speaker, and the labeled `text` is built from the merged segments (see
/// `build_multichannel_result`). The caller supplies a client with a long
/// timeout; this fn sets none of its own (parity with `transcribe_deepgram_batch`).
pub(crate) async fn transcribe_deepgram_multichannel(
    client: &reqwest::Client,
    stereo_wav_bytes: Vec<u8>,
    api_key: &str,
    language: &str,
) -> Result<(String, Vec<crate::srt::TimedSegment>), ApiError> {
    let response = client
        .post(multichannel_url(language))
        .header("Authorization", format!("Token {}", api_key))
        .header("Content-Type", "audio/wav")
        .body(stereo_wav_bytes)
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

    Ok(build_multichannel_result(&body))
}

// ── ElevenLabs Scribe ──
//
// Measured 2026-11-19 (FLEURS Hebrew dev, 40 clips / 648 words, plus 12
// Hebrew-English code-switched sentences): Scribe v3 batch 11.3% WER and 6.6%
// on the mixed set, against Deepgram multi 30.7% / 30.7% and Deepgram he
// 20.1% / 32.1%. Pinning `he` still writes English terms in Latin letters and
// transcribed pure English perfectly, so "auto" maps to `he` here; adding
// `en` as a secondary language made the mixed set worse (10.9%).

const ELEVENLABS_STT_URL: &str = "https://api.elevenlabs.io/v1/speech-to-text";
pub(crate) const ELEVENLABS_BATCH_MODEL: &str = "scribe_v3";

/// Scribe language code for an app language setting.
pub(crate) fn elevenlabs_language(lang: &str) -> &str {
    match lang {
        "auto" | "multi" | "he" => "he",
        other => other,
    }
}

/// ElevenLabs reports a bad key as 400 `authentication_error` (e.g. a key
/// without the `sk_` prefix) and an empty balance as `quota_exceeded`, so the
/// body decides before the status does.
pub(crate) fn classify_elevenlabs(status: reqwest::StatusCode, body: &str) -> ApiError {
    if body.contains("authentication_error") || body.contains("invalid_api_key") {
        ApiError::Unauthorized
    } else if body.contains("quota_exceeded") || body.contains("insufficient") {
        ApiError::InsufficientCredit
    } else {
        classify_status(status, body)
    }
}

/// `speaker_3` → 3.
pub(crate) fn elevenlabs_speaker(v: &serde_json::Value) -> Option<u32> {
    v.as_str()?.rsplit('_').next()?.parse().ok()
}

/// Scribe `words[]` (word / spacing / audio_event entries) → `TimedWord`s.
pub(crate) fn parse_elevenlabs_words(words: &serde_json::Value) -> Vec<crate::srt::TimedWord> {
    words
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|w| w["type"].as_str().unwrap_or("word") == "word")
                .filter_map(|w| {
                    let text = w["text"].as_str()?.trim();
                    if text.is_empty() {
                        return None;
                    }
                    Some(crate::srt::TimedWord {
                        text: text.to_string(),
                        start_ms: (w["start"].as_f64()? * 1000.0).round() as u64,
                        end_ms: (w["end"].as_f64()? * 1000.0).round() as u64,
                        speaker: elevenlabs_speaker(&w["speaker_id"]),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn elevenlabs_form(audio: Vec<u8>, language: &str) -> Result<multipart::Form, ApiError> {
    let file_part = multipart::Part::bytes(audio)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| ApiError::Other(format!("Failed to create multipart: {}", e)))?;
    let mut form = multipart::Form::new()
        .text("model_id", ELEVENLABS_BATCH_MODEL)
        .text("language_code", elevenlabs_language(language).to_string())
        .text("tag_audio_events", "false");
    for t in crate::dictionary::elevenlabs_keyterms() {
        form = form.text("keyterms", t);
    }
    Ok(form.part("file", file_part))
}

async fn elevenlabs_post(
    client: &reqwest::Client,
    form: multipart::Form,
    api_key: &str,
    timeout: Option<Duration>,
) -> Result<serde_json::Value, ApiError> {
    let mut req = client
        .post(ELEVENLABS_STT_URL)
        .header("xi-api-key", api_key)
        .multipart(form);
    if let Some(t) = timeout {
        req = req.timeout(t);
    }
    let response = req.send().await.map_err(|e| classify_request_error(&e))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(classify_elevenlabs(status, &body));
    }
    response
        .json()
        .await
        .map_err(|e| ApiError::Other(format!("Failed to parse ElevenLabs response: {}", e)))
}

async fn transcribe_elevenlabs_inner(
    samples: &[f32],
    api_key: &str,
    language: &str,
) -> Result<String, ApiError> {
    let form = elevenlabs_form(samples_to_wav(samples, 16000), language)?;
    let body = elevenlabs_post(&reqwest::Client::new(), form, api_key, Some(Duration::from_secs(30))).await?;
    Ok(body["text"].as_str().unwrap_or("").trim().to_string())
}

/// Long-file ElevenLabs request, same contract as `transcribe_deepgram_batch`
/// (the caller's client carries the long timeout). Diarized, word timestamps.
pub(crate) async fn transcribe_elevenlabs_batch(
    client: &reqwest::Client,
    samples: &[f32],
    api_key: &str,
    language: &str,
) -> Result<(String, Vec<crate::srt::TimedSegment>), ApiError> {
    let form = elevenlabs_form(samples_to_wav(samples, 16000), language)?
        .text("diarize", "true")
        .text("timestamps_granularity", "word");
    let body = elevenlabs_post(client, form, api_key, None).await?;
    Ok(build_elevenlabs_batch_result(&body))
}

fn build_elevenlabs_batch_result(body: &serde_json::Value) -> (String, Vec<crate::srt::TimedSegment>) {
    let words = parse_elevenlabs_words(&body["words"]);
    let segments = crate::srt::chunk_words_to_cues(
        &words,
        crate::srt::SRT_MAX_WORDS_PER_CUE,
        crate::srt::SRT_MAX_MS_PER_CUE,
    );
    (body["text"].as_str().unwrap_or("").trim().to_string(), segments)
}

/// Call mode on ElevenLabs: the same stereo WAV (channel 0 = me, 1 = them)
/// with `use_multi_channel=true`. The response carries `transcripts[]`, one
/// per channel with its own `channel_index` and words on the shared clock
/// (verified live 2026-11-19).
pub(crate) async fn transcribe_elevenlabs_multichannel(
    client: &reqwest::Client,
    stereo_wav_bytes: Vec<u8>,
    api_key: &str,
    language: &str,
) -> Result<(String, Vec<crate::srt::TimedSegment>), ApiError> {
    let form = elevenlabs_form(stereo_wav_bytes, language)?
        .text("use_multi_channel", "true")
        .text("timestamps_granularity", "word");
    let body = elevenlabs_post(client, form, api_key, None).await?;
    Ok(build_elevenlabs_multichannel_result(&body))
}

fn build_elevenlabs_multichannel_result(
    body: &serde_json::Value,
) -> (String, Vec<crate::srt::TimedSegment>) {
    let mut channels: Vec<Vec<crate::srt::TimedWord>> = vec![Vec::new(), Vec::new()];
    if let Some(ts) = body["transcripts"].as_array() {
        for (i, t) in ts.iter().enumerate() {
            let idx = t["channel_index"].as_u64().map(|n| n as usize).unwrap_or(i);
            if idx < 2 {
                channels[idx] = parse_elevenlabs_words(&t["words"]);
            }
        }
    }
    merge_call_channels(channels)
}

/// File / call transcription on whichever provider `settings::file_provider` picked.
pub(crate) async fn transcribe_file_batch(
    provider: &ApiProvider,
    client: &reqwest::Client,
    samples: &[f32],
    api_key: &str,
    language: &str,
) -> Result<(String, Vec<crate::srt::TimedSegment>), ApiError> {
    match provider {
        ApiProvider::ElevenLabs => transcribe_elevenlabs_batch(client, samples, api_key, language).await,
        _ => transcribe_deepgram_batch(client, samples, api_key, language).await,
    }
}

pub(crate) async fn transcribe_call_multichannel(
    provider: &ApiProvider,
    client: &reqwest::Client,
    stereo_wav_bytes: Vec<u8>,
    api_key: &str,
    language: &str,
) -> Result<(String, Vec<crate::srt::TimedSegment>), ApiError> {
    match provider {
        ApiProvider::ElevenLabs => {
            transcribe_elevenlabs_multichannel(client, stereo_wav_bytes, api_key, language).await
        }
        _ => transcribe_deepgram_multichannel(client, stereo_wav_bytes, api_key, language).await,
    }
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
        ApiProvider::ElevenLabs => transcribe_elevenlabs_inner(samples, api_key, lang).await,
        ApiProvider::Groq => {
            let text = transcribe_groq_inner(samples, api_key, lang).await?;
            // Whisper's own language detection reads unclear Hebrew as Arabic
            // or another language. "auto" promises Hebrew or English, so a
            // result in any other script is run again as Hebrew.
            if is_auto_language(lang) && has_foreign_letter(&text) {
                return transcribe_groq_inner(samples, api_key, "he").await;
            }
            Ok(text)
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_words_reads_speaker_field() {
        // A diarized Deepgram `alternatives[0]`: each word's `speaker` index
        // must land on the corresponding TimedWord, alongside text and timing.
        let alt = serde_json::json!({
            "words": [
                { "word": "שלום", "punctuated_word": "שלום", "start": 0.0, "end": 0.5, "speaker": 0 },
                { "word": "עולם", "punctuated_word": "עולם", "start": 0.5, "end": 1.0, "speaker": 0 },
                { "word": "היי", "punctuated_word": "היי", "start": 1.0, "end": 1.4, "speaker": 1 }
            ]
        });
        let words = parse_deepgram_words(&alt);
        assert_eq!(words.len(), 3);
        assert_eq!(words[0].text, "שלום");
        assert_eq!(words[0].start_ms, 0);
        assert_eq!(words[0].end_ms, 500);
        assert_eq!(words[0].speaker, Some(0));
        assert_eq!(words[1].speaker, Some(0));
        assert_eq!(words[2].text, "היי");
        assert_eq!(words[2].speaker, Some(1));
    }

    #[test]
    fn elevenlabs_language_pins_hebrew_for_auto() {
        assert_eq!(elevenlabs_language("auto"), "he");
        assert_eq!(elevenlabs_language("multi"), "he");
        assert_eq!(elevenlabs_language("he"), "he");
        assert_eq!(elevenlabs_language("en"), "en");
        assert_eq!(elevenlabs_language("fr"), "fr");
    }

    #[test]
    fn elevenlabs_errors_classify_by_body() {
        // Real body for a bad key (2026-11-19): HTTP 400, not 401.
        let bad = r#"{"detail":{"type":"authentication_error","code":"invalid_api_key","message":"API key must start with 'sk_'."}}"#;
        assert!(classify_elevenlabs(reqwest::StatusCode::BAD_REQUEST, bad).is_key_problem());
        assert!(matches!(
            classify_elevenlabs(reqwest::StatusCode::UNAUTHORIZED, r#"{"detail":{"status":"quota_exceeded"}}"#),
            ApiError::InsufficientCredit
        ));
        assert!(matches!(
            classify_elevenlabs(reqwest::StatusCode::BAD_REQUEST, "audio too short"),
            ApiError::BadRequest(_)
        ));
    }

    #[test]
    fn elevenlabs_words_skip_spacing_and_read_speakers() {
        let words = serde_json::json!([
            {"text":"כעת","start":0.72,"end":0.96,"type":"word","speaker_id":"speaker_0"},
            {"text":" ","start":0.96,"end":1.0,"type":"spacing","speaker_id":"speaker_0"},
            {"text":"זמין,","start":1.0,"end":1.26,"type":"word","speaker_id":"speaker_12"},
            {"text":"(laughs)","start":1.3,"end":1.5,"type":"audio_event"}
        ]);
        let w = parse_elevenlabs_words(&words);
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].text.as_str(), w[0].start_ms, w[0].end_ms, w[0].speaker), ("כעת", 720, 960, Some(0)));
        assert_eq!((w[1].text.as_str(), w[1].speaker), ("זמין,", Some(12)));
        assert_eq!(elevenlabs_speaker(&serde_json::Value::Null), None);
    }

    #[test]
    fn elevenlabs_multichannel_labels_each_side_and_merges_by_time() {
        // Shape verified live 2026-11-19: transcripts[] with channel_index.
        let body = serde_json::json!({"transcripts":[
            {"channel_index":1,"text":"היי","words":[{"text":"היי","start":2.4,"end":2.7,"type":"word","speaker_id":"speaker_1"}]},
            {"channel_index":0,"text":"שלום","words":[{"text":"שלום","start":0.68,"end":0.96,"type":"word","speaker_id":"speaker_0"}]}
        ]});
        let (text, segs) = build_elevenlabs_multichannel_result(&body);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].speaker, Some(0));
        assert_eq!(segs[1].speaker, Some(1));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("שלום"));
        assert!(lines[1].ends_with("היי"));
    }

    #[test]
    fn elevenlabs_batch_result_uses_service_text_and_word_cues() {
        let body = serde_json::json!({"text":" שלום עולם ","words":[
            {"text":"שלום","start":0.0,"end":0.4,"type":"word","speaker_id":"speaker_0"},
            {"text":" ","start":0.4,"end":0.5,"type":"spacing"},
            {"text":"עולם","start":0.5,"end":0.9,"type":"word","speaker_id":"speaker_0"}
        ]});
        let (text, segs) = build_elevenlabs_batch_result(&body);
        assert_eq!(text, "שלום עולם");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].start_ms, 0);
        assert_eq!(segs[0].end_ms, 900);
    }

    #[test]
    fn parse_words_absent_speaker_is_none() {
        // No diarization → no `speaker` key → speaker stays None. The whisper
        // and non-diarized cloud paths both rely on this.
        let alt = serde_json::json!({
            "words": [
                { "word": "בדיקה", "start": 0.0, "end": 0.3 }
            ]
        });
        let words = parse_deepgram_words(&alt);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].speaker, None);
    }

    #[test]
    fn samples_to_wav_stereo_header_is_two_channel() {
        // One stereo frame (L=0, R=0) → 2 samples → 4 data bytes, 48-byte file.
        let wav = samples_to_wav_stereo(&[0.0f32, 0.0], 16000);
        assert_eq!(wav.len(), 48);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        // audio format = PCM (1)
        assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 1);
        // num_channels = 2 (the whole point of the stereo variant)
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2);
        // sample_rate = 16000
        assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), 16000);
        // byte_rate = 16000 * 2ch * 2bytes = 64000
        assert_eq!(u32::from_le_bytes([wav[28], wav[29], wav[30], wav[31]]), 64000);
        // block_align = 2ch * 2bytes = 4
        assert_eq!(u16::from_le_bytes([wav[32], wav[33]]), 4);
        // bits per sample = 16
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16);
        assert_eq!(&wav[36..40], b"data");
        // data_size = 4 bytes
        assert_eq!(u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]), 4);
    }

    // Real transcripts from the 2026-11-19 measurement.
    #[test]
    fn prefer_multi_keeps_hebrew_for_hebrew_speech() {
        assert!(!prefer_multi(
            "הלקוח ביקש שנעבור לזום במקום טימס בשבוע הבא",
            "הלקוח ביקש שנעבור לזום במקום Teams בשבוע הבא."
        ));
        // multi's mishearing in Arabic script is not English
        assert!(!prefer_multi(
            "מטרתו העיקרית של המדע היא לגלות כיצד העולם פועל",
            "مطرتوها اיקريت شيل حماده هي لجلوت kidsa علم بعلم"
        ));
        assert!(!prefer_multi("שלום", ""));
    }

    #[test]
    fn prefer_multi_takes_english_speech() {
        // he deleted the English sentence
        assert!(prefer_multi("", "Please send me the final version by Thursday."));
        // mostly Latin words
        assert!(prefer_multi(
            "אז תגיע קצת לפני",
            "The meeting is at five, as taguya קצת left nay."
        ));
        // he spelled English with niqqud, multi heard Latin words
        assert!(prefer_multi(
            "ל ט ס דוּ אִיט טוּמּוּרוֹ, וְעַז נָאֵלָם.",
            "הוא אמר לי' לet's do it tomorrow' ואז נעלם."
        ));
        // a whole English sentence inside a long Hebrew transcript
        let he = "שלום לכולם היום נדבר על התוכנית לשבוע הבא ועל התקציב של הפרויקט";
        let multi = "שלום לכולם היום נדבר על התוכנית לשבוע הבא and then we will review the budget ועל התקציב של הפרויקט";
        assert!(prefer_multi(he, multi));
    }

    /// Live check of the "auto" pick against Deepgram. Needs DEEPGRAM_API_KEY and
    /// LIVE_WAVS = ;-separated paths of 16 kHz mono PCM16 WAV files. Prints each transcript.
    #[test]
    #[ignore]
    fn live_deepgram_auto_pick() {
        let key = std::env::var("DEEPGRAM_API_KEY").expect("DEEPGRAM_API_KEY");
        let wavs = std::env::var("LIVE_WAVS").expect("LIVE_WAVS");
        let rt = tokio::runtime::Runtime::new().unwrap();
        for path in wavs.split(';') {
            let bytes = std::fs::read(path).unwrap();
            let samples: Vec<f32> = bytes[44..]
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                .collect();
            let text = rt
                .block_on(transcribe_api_inner(&ApiProvider::Deepgram, &samples, &key, "auto"))
                .unwrap();
            assert!(!text.trim().is_empty(), "{path}");
            println!("{path}\n  {text}");
        }
    }

    #[test]
    fn groq_prompt_adds_dictionary_words_within_the_cap() {
        assert_eq!(groq_prompt(&[]), GROQ_PROMPT_STYLE);
        let p = groq_prompt(&["BinTech".to_string(), "הנרי".to_string()]);
        assert!(p.ends_with(" BinTech, הנרי"));
        let many: Vec<String> = (0..100).map(|i| format!("מילה{}", i)).collect();
        assert!(groq_prompt(&many).chars().count() <= GROQ_PROMPT_MAX_CHARS);
    }

    #[test]
    fn samples_to_wav_stereo_encodes_and_clamps_samples() {
        // Full-scale L=+1.0 clamps to i16 32767; R=-1.0 maps to -32768.
        let wav = samples_to_wav_stereo(&[1.0f32, -1.0], 16000);
        assert_eq!(wav.len(), 48);
        // First sample (L): 32767 = 0x7FFF little-endian.
        assert_eq!(&wav[44..46], &32767i16.to_le_bytes());
        // Second sample (R): -32768 = 0x8000 little-endian.
        assert_eq!(&wav[46..48], &(-32768i16).to_le_bytes());
    }

    #[test]
    fn multichannel_url_has_multichannel_but_no_diarize_or_paragraphs() {
        // Same nova-3 base as the batch route, "auto" resolved to multilingual…
        let url = multichannel_url("auto");
        assert!(url.contains("model=nova-3"));
        assert!(url.contains("language=multi"));
        assert!(url.contains("smart_format=true"));
        assert!(url.contains("punctuate=true"));
        // …but Call mode is per-channel: multichannel ON, and NO diarize/paragraphs
        // (the labeled text is built from segments, not the flat transcript).
        assert!(url.contains("multichannel=true"));
        assert!(!url.contains("diarize"));
        assert!(!url.contains("paragraphs"));
        // Hebrew also carries the day-ordinal replace fix (see day_ordinal_replace_params).
        assert!(url.contains("replace="));

        // A non-Hebrew language: base params unchanged, no replace params appended
        // (the º-for-day-name bug is Hebrew-specific).
        assert_eq!(
            multichannel_url("en"),
            "https://api.deepgram.com/v1/listen?model=nova-3&language=en&smart_format=true&punctuate=true&multichannel=true&mip_opt_out=true"
        );
        assert!(!multichannel_url("en").contains("language_hint"));
        assert!(url.contains("&language_hint=he&language_hint=en"));
    }

    #[test]
    fn language_hints_only_ride_with_the_multilingual_model() {
        // Deepgram answers HTTP 400 to a hint on a single-language request.
        assert_eq!(deepgram_common_params("he"), "&mip_opt_out=true");
        assert_eq!(deepgram_common_params("en"), "&mip_opt_out=true");
        assert!(deepgram_common_params("multi").contains("language_hint=he"));
        assert!(deepgram_common_params("multi").contains("mip_opt_out=true"));
    }

    fn alt(words: &[(&str, Option<&str>)]) -> serde_json::Value {
        let words: Vec<_> = words
            .iter()
            .map(|(w, l)| match l {
                Some(l) => serde_json::json!({"word": w, "punctuated_word": w, "language": l}),
                None => serde_json::json!({"word": w, "punctuated_word": w}),
            })
            .collect();
        serde_json::json!({ "words": words })
    }

    #[test]
    fn words_heard_as_a_third_language_are_dropped() {
        // The real segments from the 2026-11-17 streaming test.
        let persian = alt(&[("اشرح", Some("fa")), ("انخوامیل", Some("fa")), ("בסדר", Some("he"))]);
        assert_eq!(without_foreign_words(&persian).as_deref(), Some("בסדר"));
        let spanish = alt(&[("¿Qué?", Some("es"))]);
        assert_eq!(without_foreign_words(&spanish).as_deref(), Some(""));
        let croatian = alt(&[("Čomatoti?", Some("hr"))]);
        assert_eq!(without_foreign_words(&croatian).as_deref(), Some(""));
    }

    #[test]
    fn hebrew_and_english_segments_are_left_untouched() {
        let mixed = alt(&[("פתחתי", Some("he")), ("pull", Some("en")), ("request", Some("en"))]);
        assert_eq!(without_foreign_words(&mixed), None);
        // Locked "he"/"en" sessions carry no language tags at all.
        let untagged = alt(&[("שלום", None), ("עולם", None)]);
        assert_eq!(without_foreign_words(&untagged), None);
        assert_eq!(without_foreign_words(&serde_json::json!({"transcript": "x"})), None);
    }

    #[test]
    fn a_plain_name_with_a_foreign_tag_survives_inside_a_known_segment() {
        let name = alt(&[("קוראים", Some("he")), ("לי", Some("he")), ("Henry", Some("fr"))]);
        assert_eq!(without_foreign_words(&name), None);
    }

    #[test]
    fn day_ordinal_replace_params_only_where_hebrew_can_appear() {
        assert_eq!(day_ordinal_replace_params("en"), "");
        assert!(!day_ordinal_replace_params("multi").is_empty());
        assert!(!day_ordinal_replace_params("he").is_empty());
    }

    #[test]
    fn auto_and_unknown_languages_go_multilingual() {
        assert_eq!(normalize_language("auto"), "multi");
        assert_eq!(normalize_language("multi"), "multi");
        assert_eq!(normalize_language("fr"), "multi");
        assert_eq!(normalize_language("he"), "he");
        assert_eq!(normalize_language("en"), "en");
        // Deepgram's spoken punctuation is English-only; it must not ride along
        // with the multilingual model.
        assert_eq!(dictation_params("multi"), "");
    }

    #[test]
    fn day_ordinal_replace_params_covers_all_seven_days_percent_encoded() {
        let params = day_ordinal_replace_params("he");
        // Deepgram's own encoding: º is U+00BA → UTF-8 C2 BA → %C2%BA.
        assert!(params.contains("&replace=1%C2%BA:%D7%A8%D7%90%D7%A9%D7%95%D7%9F"));
        assert!(params.contains("&replace=2%C2%BA:%D7%A9%D7%A0%D7%99"));
        assert_eq!(params.matches("&replace=").count(), 7, "one pair per day of the week");
    }

    #[test]
    fn multichannel_stamps_channel_speaker_and_labels_text() {
        // Deepgram multichannel response with diarize OFF: words carry NO `speaker`
        // field, so the parser returns None and the channel index must be stamped
        // explicitly. channels[0] = mic ("me"), channels[1] = system ("them").
        // "them" speaks first in time, so the chronological merge must reorder it
        // ahead of "me" — proving the merge is by start_ms, not append order.
        let body = serde_json::json!({
            "results": {
                "channels": [
                    { "alternatives": [{ "words": [
                        { "word": "הכול", "punctuated_word": "הכול", "start": 1.0, "end": 1.3 },
                        { "word": "טוב", "punctuated_word": "טוב", "start": 1.3, "end": 1.6 }
                    ]}]},
                    { "alternatives": [{ "words": [
                        { "word": "מה", "punctuated_word": "מה", "start": 0.0, "end": 0.3 },
                        { "word": "נשמע", "punctuated_word": "נשמע", "start": 0.3, "end": 0.6 }
                    ]}]}
                ]
            }
        });

        let (text, segments) = build_multichannel_result(&body);

        // Channel index stamped explicitly (parser returned None — diarize off).
        assert_eq!(segments.len(), 2);
        // Merged chronologically: channel 1 ("them", start 0) before channel 0
        // ("me", start 1000).
        assert_eq!(segments[0].speaker, Some(1));
        assert_eq!(segments[0].text, "מה נשמע");
        assert_eq!(segments[1].speaker, Some(0));
        assert_eq!(segments[1].text, "הכול טוב");

        // Labeled text in chronological order, with BOTH sides present.
        assert_eq!(text, "הצד השני: מה נשמע\nאני: הכול טוב");
        assert!(text.contains("אני:"));
        assert!(text.contains("הצד השני:"));
    }

    #[test]
    fn multichannel_wrapper_has_expected_signature() {
        // Compile-time signature guard only — never polls the future, never hits
        // the network. `_reference` compiles ONLY if transcribe_deepgram_multichannel
        // takes (&reqwest::Client, Vec<u8>, &str, &str) and returns a future whose
        // Output is Result<(String, Vec<TimedSegment>), ApiError>.
        #[allow(dead_code)]
        async fn _reference(
            client: &reqwest::Client,
            bytes: Vec<u8>,
            key: &str,
            lang: &str,
        ) -> Result<(String, Vec<crate::srt::TimedSegment>), ApiError> {
            transcribe_deepgram_multichannel(client, bytes, key, lang).await
        }
        let _ = _reference; // silence unused warning; body is never executed
    }
}
