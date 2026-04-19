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

impl StreamingSession {
    /// Open a WebSocket connection to Deepgram streaming and start a receive task
    /// that emits `transcription-interim` events for each message.
    pub async fn start(
        api_key: &str,
        language: &str,
        app: AppHandle,
    ) -> Result<Arc<Self>, String> {
        let url = format!(
            "wss://api.deepgram.com/v1/listen?model=nova-3&language={}&encoding=linear16&sample_rate=16000&channels=1&smart_format=true&punctuate=true&interim_results=true",
            language
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
        let recv_task = tokio::spawn(async move {
            while let Some(msg) = reader.next().await {
                match msg {
                    Ok(Message::Text(txt)) => {
                        handle_message(&txt, &final_text_rx, &app_clone).await;
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

async fn handle_message(raw: &str, final_text: &Arc<Mutex<String>>, app: &AppHandle) {
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

    if is_final {
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
        WsErr::Http(resp) if resp.status().as_u16() == 401 => {
            "API key invalid — check your Deepgram key in settings".to_string()
        }
        WsErr::Http(resp) => format!("Deepgram streaming error: HTTP {}", resp.status()),
        WsErr::Io(io) => format!("Cannot connect to Deepgram — {}", io),
        _ => format!("Streaming error: {}", e),
    }
}
