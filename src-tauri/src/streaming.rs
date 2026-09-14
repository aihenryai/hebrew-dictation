use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
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
    recv_task: Mutex<Option<JoinHandle<Result<(), String>>>>,
    stop_requested: Arc<AtomicBool>,
    injection: Arc<SessionInjection>,
}

pub(crate) struct StreamingStop {
    pub text: String,
    pub warning: Option<String>,
}

/// A cancelled spawn_blocking future does not stop its OS thread. Closing this
/// gate skips queued work and waits for any injection already inside it, before
/// the UI can restore a window or start another dictation.
struct SessionInjection {
    enabled: AtomicBool,
    running: std::sync::Mutex<()>,
}

impl SessionInjection {
    fn new() -> Self {
        Self {
            enabled: AtomicBool::new(true),
            running: std::sync::Mutex::new(()),
        }
    }

    fn run(&self, inject: impl FnOnce()) {
        let _guard = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if self.enabled.load(Ordering::SeqCst) {
            inject();
        }
    }

    async fn close_and_drain(self: &Arc<Self>) {
        self.enabled.store(false, Ordering::SeqCst);
        let gate = self.clone();
        let _ = tokio::task::spawn_blocking(move || {
            drop(gate.running.lock().unwrap_or_else(|e| e.into_inner()));
        })
        .await;
    }
}

/// Timeout must abort AND join: dropping a JoinHandle merely detaches its task.
pub(crate) async fn finish_task(
    mut task: JoinHandle<Result<(), String>>,
    timeout: Duration,
) -> Result<(), String> {
    match tokio::time::timeout(timeout, &mut task).await {
        Ok(result) => result.map_err(|e| format!("משימת התמלול נקטעה: {e}"))?,
        Err(_) => {
            task.abort();
            let _ = task.await;
            Err("שירות התמלול לא סיים בזמן; ייתכן שחסר סוף ההכתבה. הטקסט שכבר התקבל נשמר.".into())
        }
    }
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
/// Strip Hebrew niqud/cantillation marks (U+0591–U+05C7 — points, dagesh,
/// rafe, shin/sin dots, cantillation; base letters are U+05D0–U+05EA, a
/// disjoint range, so this never touches them). Confirmed necessary live
/// (Henry, 2026-09-09): a short, isolated utterance like "כתוב באנגלית" can
/// come back from Deepgram FULLY NIQQUD ("כְּתוֹב בַּאֲנָלִית") even though
/// no other transcript this session ever showed niqud — smart_format seems to
/// reach for a more "dictionary pronunciation" rendering specifically when it
/// has little surrounding context. Byte-for-byte exact matching against plain
/// text silently failed on every such segment.
fn strip_niqud(s: &str) -> String {
    s.chars()
        .filter(|c| !('\u{0591}'..='\u{05C7}').contains(c))
        .collect()
}

fn detect_language_switch(transcript: &str) -> Option<&'static str> {
    let stripped = strip_niqud(transcript);
    let trimmed = stripped
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
        let stop_requested = Arc::new(AtomicBool::new(false));
        let stopping = stop_requested.clone();
        let injection = Arc::new(SessionInjection::new());
        let injection_rx = injection.clone();

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
                            &injection_rx,
                        )
                        .await;
                    }
                    Ok(Message::Close(_)) => break,
                    Err(e) => {
                        let error = map_ws_error(&e);
                        let _ = app_clone.emit("audio-stream-error", &error);
                        return Err(error);
                    }
                    _ => {}
                }
            }
            if stopping.load(Ordering::SeqCst) {
                Ok(())
            } else {
                let error =
                    "החיבור לשירות התמלול נסגר במהלך ההכתבה. הטקסט שכבר התקבל נשמר.".to_string();
                let _ = app_clone.emit("audio-stream-error", &error);
                Err(error)
            }
        });

        Ok(Arc::new(Self {
            writer,
            final_text,
            recv_task: Mutex::new(Some(recv_task)),
            stop_requested,
            injection,
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

    /// Ask Deepgram to flush and let the SERVER close the WebSocket afterwards.
    /// Never discard received text because shutdown failed or timed out.
    pub async fn stop(&self) -> StreamingStop {
        self.stop_with_timeout(Duration::from_secs(5)).await
    }

    async fn stop_with_timeout(&self, timeout: Duration) -> StreamingStop {
        // Holding this guard also serializes concurrent stop requests.
        let mut receiver = self.recv_task.lock().await;
        self.stop_requested.store(true, Ordering::SeqCst);
        let deadline = tokio::time::Instant::now() + timeout;
        let close_result = tokio::time::timeout_at(deadline, async {
            let mut guard = self.writer.lock().await;
            if let Some(mut writer) = guard.take() {
                writer
                    .send(Message::Text(
                        r#"{"type":"CloseStream"}"#.to_string().into(),
                    ))
                    .await
                    .map_err(|e| map_ws_error(&e))?;
                // Do not call writer.close(): after sending a WebSocket close
                // frame tungstenite will reject late transcript data frames.
            }
            Ok::<(), String>(())
        })
        .await;
        let mut warning = match close_result {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e),
            Err(_) => Some("בקשת סיום ההכתבה התעכבה; ייתכן שחסר סוף התמלול.".into()),
        };
        if let Some(task) = receiver.take() {
            if let Err(error) = finish_task(
                task,
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            {
                warning.get_or_insert(error);
            }
        }
        self.injection.close_and_drain().await;
        let text = self.final_text.lock().await.clone();
        StreamingStop {
            text: text.trim().to_string(),
            warning,
        }
    }
}

async fn handle_message(
    raw: &str,
    final_text: &Arc<Mutex<String>>,
    app: &AppHandle,
    inject_err_reported: &InjectErrReported,
    language_switch_enabled: bool,
    injection: &Arc<SessionInjection>,
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

    let is_final = json
        .get("is_final")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);

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
        // Commit received words before awaiting OS input. If stop cancels the
        // receive task during injection, history still contains these words.
        {
            let mut acc = final_text.lock().await;
            if !acc.is_empty() {
                acc.push(' ');
            }
            acc.push_str(transcript);
        }
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
        let gate = injection.clone();
        let _ = tokio::task::spawn_blocking(move || {
            gate.run(|| {
                if let Err(e) = crate::inject_text_defocused(&app_for_inject, &to_inject) {
                    // Surface the failure to the UI ONCE per session — previously it
                    // was discarded, so a Mac without Accessibility permission
                    // streamed an entire dictation into nothing with zero feedback.
                    if !reported.swap(true, std::sync::atomic::Ordering::SeqCst) {
                        let _ = app_for_err.emit("injection-error", e);
                    }
                }
            });
        })
        .await;
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
            400 => {
                "Deepgram דחה את הבקשה (400) — ייתכן שפת תמלול לא נתמכת במצב streaming".to_string()
            }
            code => format!("שגיאת Deepgram (HTTP {})", code),
        },
        WsErr::Io(io) => format!("אין חיבור ל-Deepgram — {}", io),
        _ => format!("שגיאת streaming: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Loopback-only fake service: no API key, microphone, or real text field.
    async fn local_session() -> (Arc<StreamingSession>, WebSocketStream<TcpStream>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            tokio_tungstenite::accept_async(socket).await.unwrap()
        });
        let (client, _) = tokio_tungstenite::connect_async(format!("ws://{address}"))
            .await
            .unwrap();
        let server = accept.await.unwrap();
        let (writer, mut reader) = client.split();
        let final_text = Arc::new(Mutex::new(String::new()));
        let received = final_text.clone();
        let receiver = tokio::spawn(async move {
            while let Some(message) = reader.next().await {
                match message.map_err(|e| e.to_string())? {
                    Message::Text(raw) => {
                        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
                        if let Some(text) = json
                            .pointer("/channel/alternatives/0/transcript")
                            .and_then(|v| v.as_str())
                        {
                            received.lock().await.push_str(text);
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            Ok(())
        });
        (
            Arc::new(StreamingSession {
                writer: Arc::new(Mutex::new(Some(writer))),
                final_text,
                recv_task: Mutex::new(Some(receiver)),
                stop_requested: Arc::new(AtomicBool::new(false)),
                injection: Arc::new(SessionInjection::new()),
            }),
            server,
        )
    }

    #[tokio::test]
    async fn stop_receives_final_words_before_the_server_closes() {
        let (session, mut server) = local_session().await;
        let service = tokio::spawn(async move {
            let message = server.next().await.unwrap().unwrap();
            assert_eq!(message, Message::Text(r#"{"type":"CloseStream"}"#.into()));
            // The old writer.close() sent a second, protocol-level close here,
            // making late final transcript frames impossible to receive.
            assert!(
                tokio::time::timeout(Duration::from_millis(30), server.next())
                    .await
                    .is_err()
            );
            server.send(Message::Text(r#"{"is_final":true,"channel":{"alternatives":[{"transcript":"המילים האחרונות"}]}}"#.into())).await.unwrap();
            server.close(None).await.unwrap();
        });
        let result = session.stop().await;
        service.await.unwrap();
        assert_eq!(result.text, "המילים האחרונות");
        assert!(result.warning.is_none(), "{:?}", result.warning);
        assert!(!session.injection.enabled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn stalled_shutdown_preserves_received_text_and_closes_injection() {
        let (session, mut server) = local_session().await;
        *session.final_text.lock().await = "טקסט שכבר התקבל".into();
        let service = tokio::spawn(async move {
            let _ = server.next().await;
            std::future::pending::<()>().await;
            drop(server);
        });
        let result = session.stop_with_timeout(Duration::from_millis(50)).await;
        assert_eq!(result.text, "טקסט שכבר התקבל");
        assert!(result.warning.is_some());
        assert!(session.recv_task.lock().await.is_none());
        session
            .injection
            .run(|| panic!("timed-out session must never inject again"));
        service.abort();
        let _ = service.await;
    }

    #[tokio::test]
    async fn timed_out_receive_or_dispatch_task_is_aborted_not_detached() {
        struct OnDrop(Arc<AtomicBool>);
        impl Drop for OnDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = dropped.clone();
        let (started, running) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _guard = OnDrop(flag);
            let _ = started.send(());
            std::future::pending::<()>().await;
            Ok(())
        });
        running.await.unwrap();
        assert!(finish_task(task, Duration::from_millis(10)).await.is_err());
        assert!(
            dropped.load(Ordering::SeqCst),
            "task must have been destroyed before returning"
        );
    }

    #[tokio::test]
    async fn shutdown_drains_in_flight_input_and_skips_queued_input() {
        let gate = Arc::new(SessionInjection::new());
        let worker_gate = gate.clone();
        let (entered, entry) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let worker = tokio::task::spawn_blocking(move || {
            worker_gate.run(|| {
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(2)).unwrap();
            });
        });
        entry.await.unwrap();
        let stop_gate = gate.clone();
        let mut stop = tokio::spawn(async move {
            stop_gate.close_and_drain().await;
        });
        assert!(tokio::time::timeout(Duration::from_millis(20), &mut stop)
            .await
            .is_err());
        let queued_gate = gate.clone();
        let queued = tokio::task::spawn_blocking(move || {
            queued_gate.run(|| panic!("queued input from a closed session must be skipped"));
        });
        release.send(()).unwrap();
        worker.await.unwrap();
        stop.await.unwrap();
        queued.await.unwrap();
    }

    #[test]
    fn detect_language_switch_matches_exact_hebrew_and_english_triggers() {
        assert_eq!(detect_language_switch("כתוב בעברית"), Some("he"));
        assert_eq!(detect_language_switch("תכתוב בעברית"), Some("he"));
        assert_eq!(detect_language_switch("כתוב באנגלית"), Some("en"));
        assert_eq!(detect_language_switch("תכתוב באנגלית"), Some("en"));
    }

    /// Real live failure (Henry, 2026-09-09): a short isolated trigger phrase
    /// came back from Deepgram FULLY NIQQUD — "כְּתוֹב בַּאֲנָלִית" — while every
    /// other transcript that session was plain text. Byte-exact matching
    /// silently never fired. (That real capture also mis-heard the base word
    /// as "אנלית" — a separate ASR error niqud-stripping can't fix; tested
    /// here with the correctly-heard word to isolate the niqud mechanism.)
    #[test]
    fn detect_language_switch_strips_niqud_deepgram_sometimes_adds() {
        assert_eq!(detect_language_switch("כְּתוֹב בַּאֲנְגְּלִית."), Some("en"));
        assert_eq!(detect_language_switch("כְּתוֹב בְּעִבְרִית"), Some("he"));
    }

    #[test]
    fn strip_niqud_removes_points_but_keeps_every_base_letter() {
        assert_eq!(strip_niqud("כְּתוֹב בַּאֲנְגְּלִית"), "כתוב באנגלית");
        assert_eq!(
            strip_niqud("שלום"),
            "שלום",
            "plain text must pass through untouched"
        );
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
        assert_eq!(
            detect_language_switch("הוא אמר לי כתוב בעברית ואני כתבתי"),
            None
        );
        assert_eq!(detect_language_switch("כתוב בעברית ותשלח לי"), None);
    }

    #[test]
    fn detect_language_switch_ignores_unrelated_text() {
        assert_eq!(detect_language_switch(""), None);
        assert_eq!(detect_language_switch("שלום עולם"), None);
        assert_eq!(detect_language_switch("write in hebrew"), None);
    }
}
