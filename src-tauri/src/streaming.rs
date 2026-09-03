use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsWriter = SplitSink<WsStream, Message>;

#[derive(Debug, Clone, Serialize)]
pub struct InterimPayload {
    pub text: String,
    pub is_final: bool,
}

/// Active Deepgram streaming session.
/// All mutable state lives behind Mutexes so the session can be shared via `Arc`
/// between the audio dispatch task and the Tauri command handler.
pub struct StreamingSession {
    writer: Arc<Mutex<Option<WsWriter>>>,
    final_text: Arc<Mutex<String>>,
    recv_task: Mutex<Option<JoinHandle<()>>>,
}

/// Per-session latch so a repeating per-segment injection failure (e.g. missing
/// macOS Accessibility permission) surfaces exactly once, not once per segment.
type InjectErrReported = Arc<std::sync::atomic::AtomicBool>;

/// Real request (עומרי רוזן, 2026-09-02): say "כתוב בעברית" / "כתוב באנגלית" mid-
/// dictation to switch language, since Deepgram's `multi` code-switching mode is
/// never used for Hebrew (see `transcribe_deepgram_batch`'s doc comment) — a
/// keyword trigger is the only way to dictate bilingually today.
///
/// Matches ONLY when the ENTIRE final segment is the trigger phrase (after
/// trimming whitespace and a trailing sentence-ending mark smart_format may add)
/// — never a substring — so a sentence that merely mentions writing Hebrew or
/// English ("אני אוהב לכתוב בעברית") is never swallowed as a command. A
/// streaming final segment is exactly what the user said between two pauses, so
/// requiring an exact match is not a burden: saying the trigger phrase alone,
/// which is how a command is naturally spoken, already produces this.
fn detect_language_switch(transcript: &str) -> Option<&'static str> {
    let trimmed = transcript
        .trim()
        .trim_end_matches(['.', '!', '?', '。', '׃'])
        .trim();
    match trimmed {
        "כתוב בעברית" | "תכתוב בעברית" => Some("he"),
        "כתוב באנגלית" | "תכתוב באנגלית" => Some("en"),
        _ => None,
    }
}

impl StreamingSession {
    /// Open a WebSocket connection to Deepgram streaming and start a receive task
    /// that emits `transcription-interim` events for each message.
    pub async fn start(
        api_key: &str,
        language: &str,
        language_switch_enabled: bool,
        app: AppHandle,
    ) -> Result<Arc<Self>, String> {
        // day_ordinal_replace_params: Deepgram's smart_format reformats Hebrew
        // day-names ("ביום שני") into a Spanish-style ordinal indicator
        // ("ביום 2º") — see its doc comment in api_transcribe.rs for the full
        // writeup. This is the default (streaming) path Henry dictates through,
        // so this call site is the one that matters most in practice.
        let url = format!(
            "wss://api.deepgram.com/v1/listen?model=nova-3&language={}&encoding=linear16&sample_rate=16000&channels=1&smart_format=true&punctuate=true&interim_results=true{}",
            language,
            crate::api_transcribe::day_ordinal_replace_params(language)
        );

        let mut request = url
            .into_client_request()
            .map_err(|e| format!("Invalid streaming URL: {}", e))?;
        request.headers_mut().insert(
            "Authorization",
            format!("Token {}", api_key)
                .parse()
                .map_err(|e| format!("Invalid auth header: {}", e))?,
        );

        let (ws_stream, _response) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| map_ws_error(&e))?;

        let (writer, mut reader) = ws_stream.split();
        let writer = Arc::new(Mutex::new(Some(writer)));
        let final_text = Arc::new(Mutex::new(String::new()));

        let final_text_rx = final_text.clone();
        let app_clone = app.clone();
        let inject_err_reported: InjectErrReported =
            Arc::new(std::sync::atomic::AtomicBool::new(false));
        let recv_task = tokio::spawn(async move {
            while let Some(msg) = reader.next().await {
                match msg {
                    Ok(Message::Text(txt)) => {
                        handle_message(
                            &txt,
                            &final_text_rx,
                            &app_clone,
                            &inject_err_reported,
                            language_switch_enabled,
                        )
                        .await;
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
        });

        Ok(Arc::new(Self {
            writer,
            final_text,
            recv_task: Mutex::new(Some(recv_task)),
        }))
    }

    /// Convert f32 samples (assumed 16kHz mono) to PCM16 LE bytes and send over the WS.
    pub async fn send_audio_pcm16(&self, samples: &[f32]) -> Result<(), String> {
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for &s in samples {
            let clamped = (s * 32768.0).clamp(-32768.0, 32767.0) as i16;
            bytes.extend_from_slice(&clamped.to_le_bytes());
        }

        let mut guard = self.writer.lock().await;
        if let Some(writer) = guard.as_mut() {
            writer
                .send(Message::Binary(bytes.into()))
                .await
                .map_err(|e| format!("WS send error: {}", e))?;
        }
        Ok(())
    }

    /// Send Deepgram's CloseStream message, close the WS, await the receive task,
    /// and return the accumulated final text.
    pub async fn stop(&self) -> Result<String, String> {
        {
            let mut guard = self.writer.lock().await;
            if let Some(mut writer) = guard.take() {
                // Deepgram accepts {"type": "CloseStream"} to flush remaining final results.
                let _ = writer
                    .send(Message::Text(r#"{"type":"CloseStream"}"#.to_string().into()))
                    .await;
                let _ = writer.close().await;
            }
        }

        let task = {
            let mut guard = self.recv_task.lock().await;
            guard.take()
        };
        if let Some(task) = task {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
        }

        let text = self.final_text.lock().await.clone();
        Ok(text.trim().to_string())
    }
}

async fn handle_message(
    raw: &str,
    final_text: &Arc<Mutex<String>>,
    app: &AppHandle,
    inject_err_reported: &InjectErrReported,
    language_switch_enabled: bool,
) {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) else {
        return;
    };

    // Skip non-transcript message types (Metadata, SpeechStarted, UtteranceEnd, etc.)
    if json.get("channel").is_none() {
        return;
    }

    let Some(transcript) = json
        .pointer("/channel/alternatives/0/transcript")
        .and_then(|t| t.as_str())
    else {
        return;
    };

    let is_final = json.get("is_final").and_then(|b| b.as_bool()).unwrap_or(false);

    if transcript.is_empty() {
        return;
    }

    if is_final && language_switch_enabled {
        if let Some(target_lang) = detect_language_switch(transcript) {
            // A command, not content: never inject it, never accumulate it into
            // the dictated text. The frontend restarts the streaming session
            // with the new language (a WS connection's language is fixed at
            // open time, so switching mid-stream means reconnecting).
            let _ = app.emit("language-switch-requested", target_lang);
            return;
        }
    }

    if is_final {
        // Inject this segment into the active text field immediately so the user
        // sees dictation appear in their target app as they speak (live streaming).
        // A trailing space separates consecutive segments. Goes through
        // `inject_text_defocused` (not the raw injector) — the floating
        // toolbar/idle-button window can hold OS focus for the entire
        // streaming session (e.g. after a mouse click started it), so every
        // segment needs the same defocus-before-typing treatment the
        // non-streaming `inject_text` command already gets.
        let to_inject = format!("{} ", transcript);
        let app_for_inject = app.clone();
        let app_for_err = app.clone();
        let reported = inject_err_reported.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if let Err(e) = crate::inject_text_defocused(&app_for_inject, &to_inject) {
                // Surface the failure to the UI ONCE per session — previously it
                // was discarded, so a Mac without Accessibility permission
                // streamed an entire dictation into nothing with zero feedback.
                if !reported.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    let _ = app_for_err.emit("injection-error", e);
                }
            }
        })
        .await;

        let mut acc = final_text.lock().await;
        if !acc.is_empty() {
            acc.push(' ');
        }
        acc.push_str(transcript);
    }

    let _ = app.emit(
        "transcription-interim",
        InterimPayload {
            text: transcript.to_string(),
            is_final,
        },
    );
}

fn map_ws_error(e: &tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error as WsErr;
    match e {
        WsErr::Http(resp) => match resp.status().as_u16() {
            401 | 403 => "מפתח Deepgram לא תקין — עדכן אותו בהגדרות".to_string(),
            402 => "נגמר הקרדיט ב-Deepgram — צור חשבון חדש או הוסף קרדיט בלוח הבקרה".to_string(),
            429 => "חרגת ממגבלת השימוש ב-Deepgram — נסה שוב בעוד רגע".to_string(),
            400 => "Deepgram דחה את הבקשה (400) — ייתכן שפת תמלול לא נתמכת במצב streaming".to_string(),
            code => format!("שגיאת Deepgram (HTTP {})", code),
        },
        WsErr::Io(io) => format!("אין חיבור ל-Deepgram — {}", io),
        _ => format!("שגיאת streaming: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_language_switch_matches_exact_hebrew_and_english_triggers() {
        assert_eq!(detect_language_switch("כתוב בעברית"), Some("he"));
        assert_eq!(detect_language_switch("תכתוב בעברית"), Some("he"));
        assert_eq!(detect_language_switch("כתוב באנגלית"), Some("en"));
        assert_eq!(detect_language_switch("תכתוב באנגלית"), Some("en"));
    }

    #[test]
    fn detect_language_switch_tolerates_smart_format_punctuation_and_whitespace() {
        assert_eq!(detect_language_switch("כתוב בעברית."), Some("he"));
        assert_eq!(detect_language_switch("  כתוב בעברית  "), Some("he"));
        assert_eq!(detect_language_switch("כתוב באנגלית!"), Some("en"));
    }

    /// The load-bearing guard: a sentence that merely mentions the trigger
    /// phrase must never be swallowed as a command — only an EXACT match on
    /// the whole final segment counts.
    #[test]
    fn detect_language_switch_never_matches_a_substring_of_a_real_sentence() {
        assert_eq!(detect_language_switch("אני אוהב לכתוב בעברית כל יום"), None);
        assert_eq!(detect_language_switch("הוא אמר לי כתוב בעברית ואני כתבתי"), None);
        assert_eq!(detect_language_switch("כתוב בעברית ותשלח לי"), None);
    }

    #[test]
    fn detect_language_switch_ignores_unrelated_text() {
        assert_eq!(detect_language_switch(""), None);
        assert_eq!(detect_language_switch("שלום עולם"), None);
        assert_eq!(detect_language_switch("write in hebrew"), None);
    }
}
