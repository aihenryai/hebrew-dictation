use crate::settings::ApiProvider;
use reqwest::multipart;
use std::time::Duration;

/// Convert f32 samples (16kHz mono) to a WAV byte buffer (PCM16).
fn samples_to_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
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

fn api_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "API timeout — try a shorter recording or check your connection".to_string()
    } else if e.is_connect() {
        "Cannot connect to API — check your internet connection".to_string()
    } else {
        format!("API request failed: {}", e)
    }
}

fn status_error(status: reqwest::StatusCode, body: &str) -> String {
    match status.as_u16() {
        401 | 403 => "API key invalid — check your key in settings".to_string(),
        429 => "API rate limited — try again in a moment".to_string(),
        _ => format!("API error {}: {}", status.as_u16(), body),
    }
}

// ── OpenAI Whisper API ──

pub async fn transcribe_openai(
    samples: &[f32],
    api_key: &str,
    language: &str,
) -> Result<String, String> {
    let wav_data = samples_to_wav(samples, 16000);

    let file_part = multipart::Part::bytes(wav_data)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| format!("Failed to create multipart: {}", e))?;

    let mut form = multipart::Form::new()
        .part("file", file_part)
        .text("model", "whisper-1")
        .text("response_format", "text");

    if language != "auto" {
        form = form.text("language", language.to_string());
    }

    let response = reqwest::Client::new()
        .post("https://api.openai.com/v1/audio/transcriptions")
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| api_error(&e))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(status_error(status, &body));
    }

    let text = response.text().await
        .map_err(|e| format!("Failed to read API response: {}", e))?;
    Ok(text.trim().to_string())
}

// ── Deepgram Nova-3 API ──

pub async fn transcribe_deepgram(
    samples: &[f32],
    api_key: &str,
    language: &str,
) -> Result<String, String> {
    let wav_data = samples_to_wav(samples, 16000);

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
        .map_err(|e| api_error(&e))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(status_error(status, &body));
    }

    let body: serde_json::Value = response.json().await
        .map_err(|e| format!("Failed to parse Deepgram response: {}", e))?;

    let transcript = body["results"]["channels"][0]["alternatives"][0]["transcript"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();

    Ok(transcript)
}

// ── Unified entry point ──

pub async fn transcribe_api(
    provider: &ApiProvider,
    samples: &[f32],
    api_key: &str,
    language: &str,
) -> Result<String, String> {
    match provider {
        ApiProvider::OpenAI => transcribe_openai(samples, api_key, language).await,
        ApiProvider::Deepgram => transcribe_deepgram(samples, api_key, language).await,
    }
}

/// Verify that the API key works for the given provider.
pub async fn test_api_key(provider: &ApiProvider, api_key: &str) -> Result<(), String> {
    let silence = vec![0.0f32; 8000]; // 0.5s at 16kHz
    match transcribe_api(provider, &silence, api_key, "he").await {
        Ok(_) => Ok(()),
        Err(e) if e.contains("API key invalid") => Err(e),
        Err(e) if e.contains("Cannot connect") => Err(e),
        // API may reject silent audio — key is still valid
        Err(_) => Ok(()),
    }
}
