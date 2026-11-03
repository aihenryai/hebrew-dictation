use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
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
type WsReader = SplitStream<WsStream>;

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
    recv_task: Mutex<Option<JoinHandle<Result<(), String>>>>,
    ctx: Arc<ReceiveContext>,
    endpoint: Endpoint,
}

/// Where to (re)connect. Kept so a dropped connection can be reopened.
struct Endpoint {
    url: String,
    auth: Option<String>,
}

/// State shared by every connection of one session: a reconnect replaces the
/// socket, not the dictation.
struct ReceiveContext {
    final_text: Arc<Mutex<String>>,
    app: Option<AppHandle>,
    inject_err_reported: InjectErrReported,
    language_switch_enabled: bool,
    live_injection: bool,
    injection: Arc<SessionInjection>,
    activity: Activity,
    stopping: AtomicBool,
    /// The service dropped the connection before CloseStream.
    dead: AtomicBool,
    unacked: std::sync::Mutex<Unacked>,
    /// Reconnects since the service last sent anything; reset on every message.
    failed_reconnects: AtomicU32,
}

impl ReceiveContext {
    fn unacked(&self) -> std::sync::MutexGuard<'_, Unacked> {
        self.unacked.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Audio sent but not yet covered by a final result, so a reconnect can
/// resend it and the words spoken just before a drop are not lost.
/// Positions count samples from the start of the session.
#[derive(Default)]
struct Unacked {
    start: u64,
    samples: VecDeque<i16>,
}

/// 60 seconds. Finals arrive every few seconds, so this is never reached on
/// a working connection; it only bounds memory on a broken one.
const MAX_UNACKED_SAMPLES: usize = 16_000 * 60;

impl Unacked {
    fn push(&mut self, pcm: &[i16]) {
        self.samples.extend(pcm.iter().copied());
        let over = self.samples.len().saturating_sub(MAX_UNACKED_SAMPLES);
        if over > 0 {
            self.samples.drain(..over);
            self.start += over as u64;
        }
    }

    fn ack(&mut self, upto: u64) {
        let n = upto.saturating_sub(self.start).min(self.samples.len() as u64) as usize;
        self.samples.drain(..n);
        self.start += n as u64;
    }
}

/// Real failure (2026-11-03, a user's computer): "WS send error: ... forcibly
/// closed by the remote host (os error 10054)" ended the whole dictation. A
/// dropped connection is now reopened and the untranscribed audio resent.
const MAX_RECONNECTS: u32 = 3;
const CONNECT_ATTEMPTS: u64 = 3;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const RECONNECT_FAILED: &str =
    "החיבור לשירות התמלול נפל ולא הצלחתי לחבר אותו מחדש. כדאי לבדוק את החיבור לאינטרנט. הטקסט שכבר התקבל נשמר.";

fn to_pcm16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| (s * 32768.0).clamp(-32768.0, 32767.0) as i16)
        .collect()
}

fn pcm_bytes(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// Where a final result's audio ends, in samples from the connection's start.
/// Empty finals (silence) count too: they acknowledge audio as well.
fn final_audio_end(raw: &str) -> Option<u64> {
    let json: serde_json::Value = serde_json::from_str(raw).ok()?;
    if !json.get("is_final")?.as_bool()? {
        return None;
    }
    let end = json.get("start")?.as_f64()? + json.get("duration")?.as_f64()?;
    Some((end * 16_000.0).round().max(0.0) as u64)
}

async fn open(endpoint: &Endpoint) -> Result<WsStream, String> {
    let mut request = endpoint
        .url
        .as_str()
        .into_client_request()
        .map_err(|e| format!("Invalid streaming URL: {}", e))?;
    if let Some(auth) = &endpoint.auth {
        request.headers_mut().insert(
            "Authorization",
            auth.parse()
                .map_err(|e| format!("Invalid auth header: {}", e))?,
        );
    }
    let (ws_stream, _response) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| map_ws_error(&e))?;
    Ok(ws_stream)
}

/// `conn_base` is the session position of this connection's first sample:
/// Deepgram's timestamps restart at zero on every connection.
fn spawn_receiver(
    ctx: Arc<ReceiveContext>,
    mut reader: WsReader,
    conn_base: u64,
) -> JoinHandle<Result<(), String>> {
    tokio::spawn(async move {
        while let Some(msg) = reader.next().await {
            ctx.activity.fetch_add(1, Ordering::SeqCst);
            match msg {
                Ok(Message::Text(txt)) => {
                    ctx.failed_reconnects.store(0, Ordering::SeqCst);
                    handle_message(&txt, &ctx).await;
                    // Only after the text is committed, so audio is never
                    // forgotten before its words are kept.
                    if let Some(end) = final_audio_end(&txt) {
                        ctx.unacked().ack(conn_base + end);
                    }
                    // Typing a long segment on a slow machine is progress too.
                    ctx.activity.fetch_add(1, Ordering::SeqCst);
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
        // After CloseStream this is the normal end; Deepgram may also drop the
        // socket without a close handshake then. Before it, the next audio
        // chunk (or stop) reconnects, so a drop is not an error by itself.
        if !ctx.stopping.load(Ordering::SeqCst) {
            ctx.dead.store(true, Ordering::SeqCst);
        }
        Ok(())
    })
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

/// Bumped on every audio chunk sent, transcript message received and segment
/// typed. Shutdown waits as long as this keeps moving, so a slow connection or
/// a slow machine that is still delivering the last words is never cut off.
pub(crate) type Activity = Arc<AtomicU64>;

/// How long shutdown waits with no activity at all before giving up, and the
/// hard ceiling however much activity there is.
#[derive(Clone, Copy)]
pub(crate) struct WaitLimits {
    pub idle: Duration,
    pub cap: Duration,
}

/// Real failure (2026-11-03, a user's slower computer): a flat 5 seconds for
/// Deepgram to finish AND for the last segments to be typed was not enough;
/// the dictation ended with "שירות התמלול לא סיים בזמן" while text was still
/// arriving. Idle-based waiting gives a working connection all the time it
/// needs and still stops a dead one after `idle`.
pub(crate) const RECEIVE_LIMITS: WaitLimits = WaitLimits {
    idle: Duration::from_secs(10),
    cap: Duration::from_secs(90),
};
pub(crate) const DISPATCH_LIMITS: WaitLimits = WaitLimits {
    idle: Duration::from_secs(8),
    cap: Duration::from_secs(90),
};

/// Timeout must abort AND join: dropping a JoinHandle merely detaches its task.
pub(crate) async fn finish_task(
    mut task: JoinHandle<Result<(), String>>,
    activity: &AtomicU64,
    limits: WaitLimits,
    timeout_error: &str,
) -> Result<(), String> {
    let started = tokio::time::Instant::now();
    let mut seen = activity.load(Ordering::SeqCst);
    loop {
        let left = limits.cap.saturating_sub(started.elapsed());
        match tokio::time::timeout(limits.idle.min(left), &mut task).await {
            Ok(result) => return result.map_err(|e| format!("משימת התמלול נקטעה: {e}"))?,
            Err(_) => {
                let now = activity.load(Ordering::SeqCst);
                if now != seen && started.elapsed() < limits.cap {
                    seen = now;
                    continue;
                }
                task.abort();
                let _ = task.await;
                return Err(timeout_error.into());
            }
        }
    }
}

const RECEIVE_TIMEOUT: &str =
    "שירות התמלול לא החזיר את סוף ההכתבה. ייתכן שהחיבור לאינטרנט איטי. הטקסט שכבר התקבל נשמר.";
pub(crate) const DISPATCH_TIMEOUT: &str =
    "סוף ההקלטה לא נשלח לשירות התמלול, כנראה בגלל חיבור איטי. הטקסט שכבר התקבל נשמר.";

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
    ///
    /// `live_injection = false` only collects the text: hold-to-talk types the
    /// whole dictation once the keys are released, because typing while the
    /// user still holds Ctrl+Win would turn every character into a shortcut.
    pub async fn start(
        api_key: &str,
        language: &str,
        language_switch_enabled: bool,
        live_injection: bool,
        app: AppHandle,
    ) -> Result<Arc<Self>, String> {
        // day_ordinal_replace_params: Deepgram's smart_format reformats Hebrew
        // day-names ("ביום שני") into a Spanish-style ordinal indicator
        // ("ביום 2º") — see its doc comment in api_transcribe.rs for the full
        // writeup. This is the default (streaming) path Henry dictates through,
        // so this call site is the one that matters most in practice.
        let url = format!(
            "wss://api.deepgram.com/v1/listen?model=nova-3&language={}&encoding=linear16&sample_rate=16000&channels=1&smart_format=true&punctuate=true&interim_results=true{}{}",
            language,
            crate::api_transcribe::day_ordinal_replace_params(language),
            crate::dictionary::keyterm_params()
        );

        let endpoint = Endpoint {
            url,
            auth: Some(format!("Token {}", api_key)),
        };
        Self::open_session(endpoint, Some(app), language_switch_enabled, live_injection).await
    }

    async fn open_session(
        endpoint: Endpoint,
        app: Option<AppHandle>,
        language_switch_enabled: bool,
        live_injection: bool,
    ) -> Result<Arc<Self>, String> {
        let (writer, reader) = open(&endpoint).await?.split();
        let ctx = Arc::new(ReceiveContext {
            final_text: Arc::new(Mutex::new(String::new())),
            app,
            inject_err_reported: Arc::new(AtomicBool::new(false)),
            language_switch_enabled,
            live_injection,
            injection: Arc::new(SessionInjection::new()),
            activity: Arc::new(AtomicU64::new(0)),
            stopping: AtomicBool::new(false),
            dead: AtomicBool::new(false),
            unacked: std::sync::Mutex::new(Unacked::default()),
            failed_reconnects: AtomicU32::new(0),
        });
        let recv_task = spawn_receiver(ctx.clone(), reader, 0);
        let writer = Arc::new(Mutex::new(Some(writer)));
        spawn_keep_alive(Arc::downgrade(&writer), KEEP_ALIVE_EVERY);
        Ok(Arc::new(Self {
            writer,
            recv_task: Mutex::new(Some(recv_task)),
            ctx,
            endpoint,
        }))
    }

    pub(crate) fn activity(&self) -> &AtomicU64 {
        &self.ctx.activity
    }

    /// Convert f32 samples (assumed 16kHz mono) to PCM16 LE bytes and send over
    /// the WS. A dropped connection is reopened here, before the error would
    /// reach the user; only a connection that cannot be reopened fails.
    pub async fn send_audio_pcm16(&self, samples: &[f32]) -> Result<(), String> {
        let pcm = to_pcm16(samples);
        self.ctx.unacked().push(&pcm);
        if self.ctx.dead.load(Ordering::SeqCst) {
            // Resends everything unacknowledged, this chunk included.
            return self.reconnect().await;
        }
        let sent = {
            let mut guard = self.writer.lock().await;
            let Some(writer) = guard.as_mut() else {
                return Ok(());
            };
            writer.send(Message::Binary(pcm_bytes(&pcm).into())).await.is_ok()
        };
        if sent {
            self.ctx.activity.fetch_add(1, Ordering::SeqCst);
            Ok(())
        } else {
            self.reconnect().await
        }
    }

    async fn reconnect(&self) -> Result<(), String> {
        let mut receiver = self.recv_task.lock().await;
        self.reconnect_locked(&mut receiver).await
    }

    /// Lock order is receiver, then writer, the same as `stop`. Holding the
    /// writer for the whole swap keeps KeepAlive and audio off the old socket.
    async fn reconnect_locked(
        &self,
        receiver: &mut Option<JoinHandle<Result<(), String>>>,
    ) -> Result<(), String> {
        let mut writer = self.writer.lock().await;
        if writer.is_none() {
            return Ok(());
        }
        if self.ctx.failed_reconnects.fetch_add(1, Ordering::SeqCst) >= MAX_RECONNECTS {
            return Err(RECONNECT_FAILED.into());
        }
        // Let the old receiver commit what it already has; a dead socket ends
        // it at once, a half-open one is cut after a moment.
        if let Some(mut old) = receiver.take() {
            if tokio::time::timeout(Duration::from_secs(2), &mut old).await.is_err() {
                old.abort();
                let _ = old.await;
            }
        }
        let mut last_error = String::new();
        for attempt in 0..CONNECT_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(attempt)).await;
            }
            let ws = match tokio::time::timeout(CONNECT_TIMEOUT, open(&self.endpoint)).await {
                Ok(Ok(ws)) => ws,
                Ok(Err(e)) => {
                    last_error = e;
                    continue;
                }
                Err(_) => {
                    last_error = "timeout".into();
                    continue;
                }
            };
            let (mut new_writer, reader) = ws.split();
            let (base, pending) = {
                let unacked = self.ctx.unacked();
                (unacked.start, unacked.samples.iter().copied().collect::<Vec<i16>>())
            };
            let mut resent = true;
            for chunk in pending.chunks(16_000) {
                if new_writer
                    .send(Message::Binary(pcm_bytes(chunk).into()))
                    .await
                    .is_err()
                {
                    resent = false;
                    break;
                }
            }
            if !resent {
                last_error = "resend failed".into();
                continue;
            }
            // Cleared before the new receiver exists, so its own drop sticks.
            self.ctx.dead.store(false, Ordering::SeqCst);
            *receiver = Some(spawn_receiver(self.ctx.clone(), reader, base));
            *writer = Some(new_writer);
            self.ctx.activity.fetch_add(1, Ordering::SeqCst);
            return Ok(());
        }
        Err(format!("{RECONNECT_FAILED} ({last_error})"))
    }

    /// Ask Deepgram to flush and let the SERVER close the WebSocket afterwards.
    /// Never discard received text because shutdown failed or timed out.
    pub async fn stop(&self) -> StreamingStop {
        self.stop_with_limits(RECEIVE_LIMITS).await
    }

    async fn stop_with_limits(&self, limits: WaitLimits) -> StreamingStop {
        // Holding this guard also serializes concurrent stop requests.
        let mut receiver = self.recv_task.lock().await;
        let mut reconnect_warning = None;
        // Dropped while the user was quiet or right before stop: words not yet
        // transcribed are still in the buffer, so reopen and resend them.
        if self.ctx.dead.load(Ordering::SeqCst) && !self.ctx.unacked().samples.is_empty() {
            reconnect_warning = self.reconnect_locked(&mut receiver).await.err();
        }
        self.ctx.stopping.store(true, Ordering::SeqCst);
        let dead = self.ctx.dead.load(Ordering::SeqCst);
        let close_result = tokio::time::timeout(limits.idle, async {
            let mut guard = self.writer.lock().await;
            if let Some(mut writer) = guard.take().filter(|_| !dead) {
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
        let mut warning = reconnect_warning.or(match close_result {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e),
            Err(_) => Some("בקשת סיום ההכתבה התעכבה; ייתכן שחסר סוף התמלול.".into()),
        });
        if let Some(task) = receiver.take() {
            if let Err(error) = finish_task(task, &self.ctx.activity, limits, RECEIVE_TIMEOUT).await {
                warning.get_or_insert(error);
            }
        }
        self.ctx.injection.close_and_drain().await;
        let text = self.ctx.final_text.lock().await.clone();
        StreamingStop {
            text: text.trim().to_string(),
            warning,
        }
    }
}

/// Deepgram closes a stream that gets no audio for about 10 seconds. The
/// microphone sends nothing while dictation is paused, and a language switch
/// holds the new connection's audio back while the old one finishes; without
/// this the connection died there and every later word was lost.
const KEEP_ALIVE_EVERY: Duration = Duration::from_secs(4);

/// Weak, so a session that is dropped without `stop` does not stay alive.
/// Stops by itself once `stop` takes the writer, before CloseStream goes out
/// on the same lock, so no KeepAlive can follow it. A failed send does not
/// stop it: a reconnect puts a new socket into the same slot.
fn spawn_keep_alive(writer: std::sync::Weak<Mutex<Option<WsWriter>>>, every: Duration) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(every).await;
            let Some(writer) = writer.upgrade() else { return };
            let mut guard = writer.lock().await;
            let Some(w) = guard.as_mut() else { return };
            let _ = w
                .send(Message::Text(r#"{"type":"KeepAlive"}"#.to_string().into()))
                .await;
        }
    });
}

async fn handle_message(raw: &str, ctx: &ReceiveContext) {
    let final_text = &ctx.final_text;
    let inject_err_reported = &ctx.inject_err_reported;
    let injection = &ctx.injection;
    // None only in tests: no window to type into or notify, the text is kept.
    let app = ctx.app.as_ref();
    let language_switch_enabled = ctx.language_switch_enabled;
    let live_injection = ctx.live_injection;
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
    let transcript_owned = crate::dictionary::apply(transcript);
    let transcript = transcript_owned.as_str();

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
            // the dictated text. The frontend asks `switch_streaming_language`
            // to swap this connection for one in the new language (a WS
            // connection's language is fixed at open time); the microphone and
            // the dictation itself keep running.
            if let Some(app) = app {
                let _ = app.emit("language-switch-requested", target_lang);
            }
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
    }

    let Some(app) = app else { return };

    if is_final && live_injection {
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

    const SHORT: WaitLimits = WaitLimits {
        idle: Duration::from_millis(50),
        cap: Duration::from_millis(200),
    };

    // Loopback-only fake service: no API key, microphone, or real text field.
    async fn local_session() -> (Arc<StreamingSession>, WebSocketStream<TcpStream>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            tokio_tungstenite::accept_async(socket).await.unwrap()
        });
        let session = StreamingSession::open_session(endpoint(address), None, false, false)
            .await
            .unwrap();
        (session, accept.await.unwrap())
    }

    fn endpoint(address: std::net::SocketAddr) -> Endpoint {
        Endpoint {
            url: format!("ws://{address}"),
            auth: None,
        }
    }

    fn final_msg(text: &str, start: f64, duration: f64) -> Message {
        Message::Text(
            format!(
                r#"{{"is_final":true,"start":{start},"duration":{duration},"channel":{{"alternatives":[{{"transcript":"{text}"}}]}}}}"#
            )
            .into(),
        )
    }

    async fn wait_until_dead(session: &StreamingSession) {
        for _ in 0..200 {
            if session.ctx.dead.load(Ordering::SeqCst) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the dropped connection was never noticed");
    }

    /// The 2026-11-03 "os error 10054" case: the service drops the socket in
    /// the middle. The dictation goes on, and audio not yet transcribed is
    /// sent again on the new connection, so no word is lost.
    #[tokio::test]
    async fn dropped_connection_reconnects_and_resends_untranscribed_audio() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (first_tx, first_rx) = tokio::sync::oneshot::channel();
        let service = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut first = tokio_tungstenite::accept_async(socket).await.unwrap();
            // One second of audio, transcribed; then 100 samples, then a drop.
            let mut got = 0;
            while got < 16_100 * 2 {
                if let Message::Binary(b) = first.next().await.unwrap().unwrap() {
                    got += b.len();
                    if got == 16_000 * 2 {
                        first.send(final_msg("לפני", 0.0, 1.0)).await.unwrap();
                    }
                }
            }
            drop(first);
            let _ = first_tx.send(());
            let (socket, _) = listener.accept().await.unwrap();
            let mut second = tokio_tungstenite::accept_async(socket).await.unwrap();
            let resent = loop {
                if let Message::Binary(b) = second.next().await.unwrap().unwrap() {
                    break b.len();
                }
            };
            second.send(final_msg("אחרי", 0.0, 0.01)).await.unwrap();
            loop {
                if second.next().await.unwrap().unwrap()
                    == Message::Text(r#"{"type":"CloseStream"}"#.into())
                {
                    break;
                }
            }
            second.close(None).await.unwrap();
            resent
        });
        let session = StreamingSession::open_session(endpoint(address), None, false, false)
            .await
            .unwrap();
        session.send_audio_pcm16(&[0.1; 16_000]).await.unwrap();
        // Wait for the first final before sending more, as real speech would.
        while session.ctx.unacked().start < 16_000 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        session.send_audio_pcm16(&[0.2; 100]).await.unwrap();
        first_rx.await.unwrap();
        wait_until_dead(&session).await;
        session.send_audio_pcm16(&[0.3; 50]).await.unwrap();
        let result = session.stop().await;
        assert_eq!(service.await.unwrap(), 150 * 2, "exactly the untranscribed audio");
        assert_eq!(result.text, "לפני אחרי");
        assert!(result.warning.is_none(), "{:?}", result.warning);
    }

    #[tokio::test]
    async fn drop_with_everything_transcribed_stops_quietly() {
        let (session, mut server) = local_session().await;
        let service = tokio::spawn(async move {
            let _ = server.next().await;
            server.send(final_msg("הכל הגיע", 0.0, 0.01)).await.unwrap();
            drop(server);
        });
        session.send_audio_pcm16(&[0.1; 160]).await.unwrap();
        service.await.unwrap();
        wait_until_dead(&session).await;
        let result = session.stop().await;
        assert_eq!(result.text, "הכל הגיע");
        assert!(result.warning.is_none(), "{:?}", result.warning);
    }

    #[tokio::test]
    async fn a_service_that_keeps_dropping_gives_up_instead_of_looping() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let service = tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                drop(tokio_tungstenite::accept_async(socket).await);
            }
        });
        let session = StreamingSession::open_session(endpoint(address), None, false, false)
            .await
            .unwrap();
        let mut failure = None;
        for _ in 0..200 {
            if let Err(e) = session.send_audio_pcm16(&[0.1; 160]).await {
                failure = Some(e);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        service.abort();
        assert!(failure.unwrap().starts_with(RECONNECT_FAILED));
    }

    #[test]
    fn unacked_audio_is_trimmed_by_finals_and_bounded() {
        let mut u = Unacked::default();
        u.push(&[1, 2, 3, 4]);
        u.ack(3);
        assert_eq!((u.start, u.samples.len()), (3, 1));
        u.ack(99);
        assert_eq!((u.start, u.samples.len()), (4, 0));
        u.push(&vec![0; MAX_UNACKED_SAMPLES + 10]);
        assert_eq!(u.samples.len(), MAX_UNACKED_SAMPLES);
        assert_eq!(u.start, 14);
        assert_eq!(
            final_audio_end(r#"{"is_final":true,"start":1.5,"duration":0.5}"#),
            Some(32_000)
        );
        assert_eq!(final_audio_end(r#"{"is_final":false,"start":1,"duration":1}"#), None);
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
        assert!(!session.ctx.injection.enabled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn keep_alive_holds_a_silent_stream_open_and_never_follows_close_stream() {
        let (session, mut server) = local_session().await;
        spawn_keep_alive(Arc::downgrade(&session.writer), Duration::from_millis(15));
        let first = tokio::time::timeout(Duration::from_secs(2), server.next())
            .await
            .expect("a silent stream must get a KeepAlive")
            .unwrap()
            .unwrap();
        assert_eq!(first, Message::Text(r#"{"type":"KeepAlive"}"#.into()));
        let service = tokio::spawn(async move {
            // Skip KeepAlives already in flight, up to CloseStream.
            loop {
                let m = server.next().await.unwrap().unwrap();
                if m == Message::Text(r#"{"type":"CloseStream"}"#.into()) {
                    break;
                }
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(60), server.next())
                    .await
                    .is_err(),
                "nothing may be sent after CloseStream"
            );
            server.close(None).await.unwrap();
        });
        let result = session.stop().await;
        service.await.unwrap();
        assert!(result.warning.is_none(), "{:?}", result.warning);
    }

    #[tokio::test]
    async fn stalled_shutdown_preserves_received_text_and_closes_injection() {
        let (session, mut server) = local_session().await;
        *session.ctx.final_text.lock().await = "טקסט שכבר התקבל".into();
        let service = tokio::spawn(async move {
            let _ = server.next().await;
            std::future::pending::<()>().await;
            drop(server);
        });
        let result = session.stop_with_limits(SHORT).await;
        assert_eq!(result.text, "טקסט שכבר התקבל");
        assert!(result.warning.is_some());
        assert!(session.recv_task.lock().await.is_none());
        session
            .ctx
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
        let idle = AtomicU64::new(0);
        assert!(finish_task(task, &idle, SHORT, "x").await.is_err());
        assert!(
            dropped.load(Ordering::SeqCst),
            "task must have been destroyed before returning"
        );
    }

    /// The 2026-11-03 failure: the end of a dictation took longer than the old
    /// flat timeout while text was still arriving. Activity keeps it waiting.
    #[tokio::test]
    async fn shutdown_that_keeps_making_progress_is_not_cut_off() {
        let activity: Activity = Arc::new(AtomicU64::new(0));
        let ticking = activity.clone();
        let task = tokio::spawn(async move {
            // 6 x 30ms = well past the 50ms idle limit, but never idle for 50ms.
            for _ in 0..6 {
                tokio::time::sleep(Duration::from_millis(30)).await;
                ticking.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        });
        let limits = WaitLimits {
            idle: Duration::from_millis(50),
            cap: Duration::from_secs(5),
        };
        assert!(finish_task(task, &activity, limits, "x").await.is_ok());
    }

    #[tokio::test]
    async fn endless_activity_still_stops_at_the_cap() {
        let activity: Activity = Arc::new(AtomicU64::new(0));
        let ticking = activity.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
                ticking.fetch_add(1, Ordering::SeqCst);
            }
        });
        let limits = WaitLimits {
            idle: Duration::from_millis(40),
            cap: Duration::from_millis(150),
        };
        let started = std::time::Instant::now();
        assert_eq!(
            finish_task(task, &activity, limits, "cap").await,
            Err("cap".to_string())
        );
        assert!(started.elapsed() < Duration::from_secs(1));
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
