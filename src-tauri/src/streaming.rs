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

/// Wire protocol of the streaming service.
///
/// Deepgram: binary PCM frames, `{"type":"CloseStream"}` to finish, results
/// with `is_final` + `start`/`duration`.
///
/// ElevenLabs Scribe v3 Live (verified live 2026-11-19): JSON text frames
/// `{"message_type":"input_audio_chunk","audio_base_64":...,"commit":bool}`;
/// `session_started` first, then `partial_transcript` and, per VAD commit,
/// `committed_transcript` followed by `committed_transcript_with_timestamps`
/// (same text, word times in seconds from the connection's first sample).
/// A bad key gets `auth_error` and a close. There is no CloseStream: the end is
/// a commit, and the connection closes by itself after ~40 s without audio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Protocol {
    Deepgram,
    ElevenLabs,
}

/// Where to (re)connect. Kept so a dropped connection can be reopened.
struct Endpoint {
    url: String,
    /// Header name and value.
    auth: Option<(&'static str, String)>,
    protocol: Protocol,
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
    protocol: Protocol,
    /// An error no reconnect can fix (bad key, no credit). Ends the dictation
    /// with this message instead of reconnecting in a loop.
    fatal: std::sync::Mutex<Option<String>>,
    /// ElevenLabs: the final commit went out; the receiver drains and ends.
    stop_commit_sent: AtomicBool,
    /// Milliseconds since `epoch` when audio was last written to the socket.
    last_audio_ms: AtomicU64,
    /// Session level for `live_gain` (see `PeakTracker`).
    peaks: std::sync::Mutex<PeakTracker>,
    epoch: std::time::Instant,
}

impl ReceiveContext {
    fn unacked(&self) -> std::sync::MutexGuard<'_, Unacked> {
        self.unacked.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn fatal(&self) -> Option<String> {
        self.fatal.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_fatal(&self, msg: String) {
        let mut f = self.fatal.lock().unwrap_or_else(|e| e.into_inner());
        if f.is_none() {
            *f = Some(msg);
        }
    }

    fn mark_audio(&self) {
        self.last_audio_ms
            .store(self.epoch.elapsed().as_millis() as u64, Ordering::SeqCst);
    }

    fn ms_since_audio(&self) -> u64 {
        (self.epoch.elapsed().as_millis() as u64).saturating_sub(self.last_audio_ms.load(Ordering::SeqCst))
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

/// Gain for live audio from a quiet microphone (see `api_transcribe::upload_gain`
/// for the measurement). `peak` is the session level from `PeakTracker`.
/// It only grows, so the gain only falls: pauses never raise it, and a single
/// click does not lower it (the tracker ignores the loudest few chunks). Below `LIVE_MIN_PEAK` the
/// session has heard only near-silence and nothing is boosted; from there to
/// twice that the gain ramps up, so it has no jump. The cap is low (8) because
/// a strong boost of a near-silent room made "multi" hear English words
/// ("Okay.") in Hebrew dictation. The target matches `upload_gain`, so audio
/// boosted here is not boosted again on upload.
const LIVE_MIN_PEAK: f32 = 0.01;
const LIVE_TARGET_PEAK: f32 = 0.5;
const LIVE_MAX_GAIN: f32 = 8.0;

fn live_gain(peak: f32) -> f32 {
    if peak < LIVE_MIN_PEAK {
        return 1.0;
    }
    let gain = (LIVE_TARGET_PEAK / peak).clamp(1.0, LIVE_MAX_GAIN);
    if peak < 2.0 * LIVE_MIN_PEAK {
        return 1.0 + (gain - 1.0) * (peak - LIVE_MIN_PEAK) / LIVE_MIN_PEAK;
    }
    gain
}

/// How many of the loudest chunks a session's level ignores. A key click or a
/// tap on the laptop (the dictation hotkey itself, on a laptop mic) fills a
/// few ~10 ms chunks; speech fills dozens. Taking the session maximum let one
/// click set the gain for the whole dictation, the reason `upload_gain` uses a
/// percentile.
const PEAK_RANK: usize = 8;

/// The session's level for `live_gain`: the `PEAK_RANK`-th loudest chunk peak
/// so far (the loudest, until that many chunks were heard). Like the old
/// maximum it never falls, so pauses never raise the gain.
#[derive(Default)]
struct PeakTracker {
    /// Loudest chunk peaks, descending, at most `PEAK_RANK`.
    top: Vec<f32>,
}

impl PeakTracker {
    fn push(&mut self, peak: f32) -> f32 {
        let at = self.top.partition_point(|&p| p >= peak);
        if at < PEAK_RANK {
            self.top.insert(at, peak);
            self.top.truncate(PEAK_RANK);
        }
        if self.top.len() < PEAK_RANK {
            self.top[0]
        } else {
            self.top[PEAK_RANK - 1]
        }
    }
}

fn to_pcm16_with_gain(samples: &[f32], gain: f32) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| (s * gain * 32768.0).clamp(-32768.0, 32767.0) as i16)
        .collect()
}


fn pcm_bytes(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// Standard base64 with padding (ElevenLabs audio frames).
fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// One audio frame in the protocol's wire format.
fn audio_frame(protocol: Protocol, pcm: &[i16], commit: bool) -> Message {
    match protocol {
        Protocol::Deepgram => Message::Binary(pcm_bytes(pcm).into()),
        Protocol::ElevenLabs => Message::Text(
            format!(
                r#"{{"message_type":"input_audio_chunk","audio_base_64":"{}","commit":{},"sample_rate":16000}}"#,
                base64_encode(&pcm_bytes(pcm)),
                commit
            )
            .into(),
        ),
    }
}

/// Resend chunk size after a reconnect. ElevenLabs gets half-second frames
/// (base64 makes them a third bigger) to stay clear of its chunk limit.
fn resend_chunk(protocol: Protocol) -> usize {
    match protocol {
        Protocol::Deepgram => 16_000,
        Protocol::ElevenLabs => 8_000,
    }
}

/// ElevenLabs: where a committed segment's audio ends, from the last word's
/// end in `committed_transcript_with_timestamps`.
fn elevenlabs_audio_end(json: &serde_json::Value) -> Option<u64> {
    if json.get("message_type")?.as_str()? != "committed_transcript_with_timestamps" {
        return None;
    }
    let end = json
        .get("words")?
        .as_array()?
        .iter()
        .filter_map(|w| w.get("end").and_then(|e| e.as_f64()))
        .fold(f64::NAN, f64::max);
    end.is_finite().then(|| (end * 16_000.0).round().max(0.0) as u64)
}

/// ElevenLabs error messages. Returns the user-facing text and whether a
/// reconnect could help (`false` = stop the dictation with this message).
fn elevenlabs_error(json: &serde_json::Value) -> Option<(String, bool)> {
    let kind = json.get("message_type")?.as_str()?;
    let fatal = match kind {
        "auth_error" => return Some((ELEVENLABS_BAD_KEY.into(), false)),
        "quota_exceeded" => return Some((ELEVENLABS_NO_CREDIT.into(), false)),
        "unaccepted_terms" => true,
        k if k.ends_with("error") || k.ends_with("exceeded") || k.ends_with("throttled")
            || k == "rate_limited" || k == "queue_overflow" || k == "resource_exhausted" =>
        {
            false
        }
        _ => return None,
    };
    let detail = json
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap_or("")
        .chars()
        .take(160)
        .collect::<String>();
    Some((format!("שגיאה בשירות התמלול של ElevenLabs ({kind}) {detail}").trim().to_string(), !fatal))
}

const ELEVENLABS_BAD_KEY: &str = "מפתח ElevenLabs לא תקין. אפשר לעדכן אותו בהגדרות.";
const ELEVENLABS_NO_CREDIT: &str =
    "נגמר הקרדיט ב-ElevenLabs. אפשר להוסיף קרדיט בחשבון שלהם או להחליף ספק בהגדרות.";

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
    if let Some((name, value)) = &endpoint.auth {
        request.headers_mut().insert(
            *name,
            value
                .parse()
                .map_err(|e| format!("Invalid auth header: {}", e))?,
        );
    }
    let (mut ws_stream, _response) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| map_ws_error(&e))?;
    if endpoint.protocol == Protocol::ElevenLabs {
        // ElevenLabs accepts the socket before checking the key, then answers
        // with `session_started` or an error. Waiting for it here turns a bad
        // key into a start failure the user sees at once.
        await_session_started(&mut ws_stream).await?;
    }
    Ok(ws_stream)
}

async fn await_session_started(ws: &mut WsStream) -> Result<(), String> {
    let first = tokio::time::timeout(CONNECT_TIMEOUT, ws.next())
        .await
        .map_err(|_| "שירות התמלול לא ענה בזמן. כדאי לבדוק את החיבור לאינטרנט.".to_string())?;
    match first {
        Some(Ok(Message::Text(txt))) => {
            let json: serde_json::Value = serde_json::from_str(&txt).unwrap_or_default();
            if json["message_type"] == "session_started" {
                Ok(())
            } else if let Some((msg, _)) = elevenlabs_error(&json) {
                Err(msg)
            } else {
                Err(format!("תשובה לא צפויה משירות התמלול: {}", txt.chars().take(120).collect::<String>()))
            }
        }
        Some(Err(e)) => Err(map_ws_error(&e)),
        _ => Err("שירות התמלול סגר את החיבור מיד. ייתכן שהמפתח לא תקין.".into()),
    }
}

/// After the final commit, how long the receiver keeps listening once
/// nothing new arrives. Commit answers came within ~0.1 s in the 2026-11-19
/// probes; partials of a long last segment keep resetting it. 0.8 s: with the
/// refine pass running, this wait was the whole stop delay (2.26 s of 2.63 s in
/// Henry's 2026-11-23 timing log), and the refine text replaces what it hears.
const ELEVENLABS_DRAIN: Duration = Duration::from_millis(800);

/// `conn_base` is the session position of this connection's first sample:
/// Deepgram's timestamps restart at zero on every connection.
fn spawn_receiver(
    ctx: Arc<ReceiveContext>,
    mut reader: WsReader,
    conn_base: u64,
) -> JoinHandle<Result<(), String>> {
    tokio::spawn(async move {
        let mut commit_after_stop = false;
        loop {
            let next = if commit_after_stop {
                match tokio::time::timeout(ELEVENLABS_DRAIN, reader.next()).await {
                    Ok(m) => m,
                    // The final commit is answered and nothing more is coming.
                    Err(_) => break,
                }
            } else {
                reader.next().await
            };
            let Some(msg) = next else { break };
            ctx.activity.fetch_add(1, Ordering::SeqCst);
            match msg {
                Ok(Message::Text(txt)) => {
                    let json: serde_json::Value = serde_json::from_str(&txt).unwrap_or_default();
                    if ctx.protocol == Protocol::ElevenLabs {
                        if let Some((msg, recoverable)) = elevenlabs_error(&json) {
                            // An error is not progress: failed_reconnects stays,
                            // so a service that keeps refusing ends the loop.
                            eprintln!("[streaming] ElevenLabs error: {}", msg);
                            if !recoverable {
                                ctx.set_fatal(msg);
                                break;
                            }
                            continue;
                        }
                    }
                    ctx.failed_reconnects.store(0, Ordering::SeqCst);
                    handle_message(&txt, &ctx).await;
                    // Only after the text is committed, so audio is never
                    // forgotten before its words are kept.
                    let end = match ctx.protocol {
                        Protocol::Deepgram => final_audio_end(&txt),
                        Protocol::ElevenLabs => elevenlabs_audio_end(&json),
                    };
                    if let Some(end) = end {
                        ctx.unacked().ack(conn_base + end);
                    }
                    if ctx.protocol == Protocol::ElevenLabs
                        && ctx.stop_commit_sent.load(Ordering::SeqCst)
                        && json["message_type"] == "committed_transcript_with_timestamps"
                    {
                        commit_after_stop = true;
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
    /// 16 kHz mono audio the service never transcribed because the connection
    /// could not be reopened (up to the last 60 seconds). The caller sends it
    /// once more as a plain upload, so those words are not lost.
    pub leftover: Vec<f32>,
}

/// Audio still unacknowledged when the session ends on a dead connection.
/// Empty when the connection was alive: then every word reached the service.
fn leftover_audio(dead: bool, unacked: &Unacked) -> Vec<f32> {
    if !dead {
        return Vec::new();
    }
    unacked.samples.iter().map(|&s| s as f32 / 32768.0).collect()
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
        provider: &crate::settings::ApiProvider,
        api_key: &str,
        language: &str,
        language_switch_enabled: bool,
        live_injection: bool,
        app: AppHandle,
    ) -> Result<Arc<Self>, String> {
        if *provider == crate::settings::ApiProvider::ElevenLabs {
            let endpoint = Endpoint {
                url: elevenlabs_url(language),
                auth: Some(("xi-api-key", api_key.to_string())),
                protocol: Protocol::ElevenLabs,
            };
            return Self::open_session(endpoint, Some(app), language_switch_enabled, live_injection).await;
        }
        // day_ordinal_replace_params: Deepgram's smart_format reformats Hebrew
        // day-names ("ביום שני") into a Spanish-style ordinal indicator
        // ("ביום 2º") — see its doc comment in api_transcribe.rs for the full
        // writeup. This is the default (streaming) path Henry dictates through,
        // so this call site is the one that matters most in practice.
        // "auto" → Deepgram's multilingual model (see normalize_language).
        let language = crate::api_transcribe::normalize_language(language);
        let url = format!(
            "wss://api.deepgram.com/v1/listen?model=nova-3&language={}&encoding=linear16&sample_rate=16000&channels=1&smart_format=true&punctuate=true&interim_results=true{}{}{}",
            language,
            crate::api_transcribe::deepgram_common_params(language),
            crate::api_transcribe::day_ordinal_replace_params(language),
            crate::dictionary::keyterm_params()
        );

        let endpoint = Endpoint {
            url,
            auth: Some(("Authorization", format!("Token {}", api_key))),
            protocol: Protocol::Deepgram,
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
            protocol: endpoint.protocol,
            fatal: std::sync::Mutex::new(None),
            stop_commit_sent: AtomicBool::new(false),
            last_audio_ms: AtomicU64::new(0),
            peaks: std::sync::Mutex::new(PeakTracker::default()),
            epoch: std::time::Instant::now(),
        });
        let recv_task = spawn_receiver(ctx.clone(), reader, 0);
        let writer = Arc::new(Mutex::new(Some(writer)));
        spawn_keep_alive(Arc::downgrade(&writer), Arc::downgrade(&ctx), KEEP_ALIVE_EVERY);
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
        if let Some(fatal) = self.ctx.fatal() {
            return Err(fatal);
        }
        let chunk_peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let level = match self.ctx.peaks.lock() {
            Ok(mut t) => t.push(chunk_peak),
            Err(_) => chunk_peak,
        };
        let pcm = to_pcm16_with_gain(samples, live_gain(level));
        if self.ctx.dead.load(Ordering::SeqCst) {
            self.ctx.unacked().push(&pcm);
            // Resends everything unacknowledged, this chunk included.
            return self.reconnect().await;
        }
        let sent = {
            let mut guard = self.writer.lock().await;
            // Pushed under the writer lock, so the ElevenLabs keep-alive
            // silence lands in the buffer in the same order as on the wire
            // and the service's timestamps still match buffer positions.
            self.ctx.unacked().push(&pcm);
            let Some(writer) = guard.as_mut() else {
                return Ok(());
            };
            let ok = writer.send(audio_frame(self.ctx.protocol, &pcm, false)).await.is_ok();
            self.ctx.mark_audio();
            ok
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
        if let Some(fatal) = self.ctx.fatal() {
            return Err(fatal);
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
                    // A refused key will not be accepted on the next try.
                    if e == ELEVENLABS_BAD_KEY || e == ELEVENLABS_NO_CREDIT {
                        self.ctx.set_fatal(e.clone());
                        return Err(e);
                    }
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
            for chunk in pending.chunks(resend_chunk(self.ctx.protocol)) {
                if new_writer
                    .send(audio_frame(self.ctx.protocol, chunk, false))
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
                let finish = match self.ctx.protocol {
                    Protocol::Deepgram => Message::Text(r#"{"type":"CloseStream"}"#.to_string().into()),
                    Protocol::ElevenLabs => {
                        // A commit needs audio behind it (an empty one is
                        // throttled and closes the session), and trailing
                        // silence lets VAD close the last word cleanly.
                        let tail = vec![0i16; STOP_TAIL_SAMPLES];
                        self.ctx.unacked().push(&tail);
                        self.ctx.stop_commit_sent.store(true, Ordering::SeqCst);
                        audio_frame(Protocol::ElevenLabs, &tail, true)
                    }
                };
                writer.send(finish).await.map_err(|e| map_ws_error(&e))?;
                // Do not call writer.close(): after sending a WebSocket close
                // frame tungstenite will reject late transcript data frames.
            }
            Ok::<(), String>(())
        })
        .await;
        let mut warning = self.ctx.fatal().or(reconnect_warning).or(match close_result {
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
        // A fatal error that arrived while draining (e.g. credit ran out on the
        // last segment) still reaches the user.
        if warning.is_none() {
            warning = self.ctx.fatal();
        }
        let text = self.ctx.final_text.lock().await.clone();
        // `dead` was read before CloseStream: a connection that is still down
        // here never got the unacknowledged audio.
        let leftover = leftover_audio(dead, &self.ctx.unacked());
        StreamingStop {
            text: text.trim().to_string(),
            warning,
            leftover,
        }
    }
}

/// Deepgram closes a stream that gets no audio for about 10 seconds. The
/// microphone sends nothing while dictation is paused, and a language switch
/// holds the new connection's audio back while the old one finishes; without
/// this the connection died there and every later word was lost.
const KEEP_ALIVE_EVERY: Duration = Duration::from_secs(4);

/// 0.4 s of silence sent with the final ElevenLabs commit.
const STOP_TAIL_SAMPLES: usize = 6_400;
/// ElevenLabs has no KeepAlive message and closes a connection that gets no
/// audio for under 40 s (its `keepalive_interval_ms` did not prevent that in
/// the 2026-11-19 probe); 100 ms of silence every 4 s did. Only sent when the
/// microphone sent nothing for `SILENT_AFTER_MS`, so speech is never split.
const KEEP_ALIVE_SILENCE_SAMPLES: usize = 1_600;
const SILENT_AFTER_MS: u64 = 3_000;

/// Weak, so a session that is dropped without `stop` does not stay alive.
/// Stops by itself once `stop` takes the writer, before CloseStream goes out
/// on the same lock, so no KeepAlive can follow it. A failed send does not
/// stop it: a reconnect puts a new socket into the same slot.
fn spawn_keep_alive(
    writer: std::sync::Weak<Mutex<Option<WsWriter>>>,
    ctx: std::sync::Weak<ReceiveContext>,
    every: Duration,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(every).await;
            let Some(writer) = writer.upgrade() else { return };
            let Some(ctx) = ctx.upgrade() else { return };
            let mut guard = writer.lock().await;
            let Some(w) = guard.as_mut() else { return };
            match ctx.protocol {
                Protocol::Deepgram => {
                    let _ = w
                        .send(Message::Text(r#"{"type":"KeepAlive"}"#.to_string().into()))
                        .await;
                }
                Protocol::ElevenLabs => {
                    if ctx.dead.load(Ordering::SeqCst) || ctx.ms_since_audio() < SILENT_AFTER_MS {
                        continue;
                    }
                    // Into the buffer too: the service counts this audio in
                    // its timestamps, so acknowledgements stay aligned.
                    let silence = vec![0i16; KEEP_ALIVE_SILENCE_SAMPLES];
                    ctx.unacked().push(&silence);
                    let _ = w.send(audio_frame(Protocol::ElevenLabs, &silence, false)).await;
                    ctx.mark_audio();
                }
            }
        }
    });
}

/// The ElevenLabs realtime URL. "auto" pins Hebrew, with no secondary language.
///
/// `secondary_languages=en` was tried and removed (2026-11-19, Henry: Hebrew
/// dictation came out as English too often). Measured live on 102 recordings:
/// it turned short Hebrew into English ("כל שינוי" -> "Call Cynu", "אבל" ->
/// "A") and gained nothing elsewhere: Hebrew 10.8% both ways, code-switched
/// 18.0% without against 19.8% with, English 1.4% against 2.7%. Pinned Hebrew
/// still writes English words in Latin letters.
fn elevenlabs_url(language: &str) -> String {
    let lang = crate::api_transcribe::elevenlabs_language(language);
    format!(
        "wss://api.elevenlabs.io/v1/speech-to-text/realtime?model_id=scribe_v3_live&audio_format=pcm_16000&language_code={}&commit_strategy=vad&include_timestamps=true{}",
        lang,
        crate::dictionary::elevenlabs_keyterm_params()
    )
}

/// The transcript text and whether it is final, in either protocol.
/// ElevenLabs sends each commit twice (`committed_transcript`, then the same
/// text `_with_timestamps`); only the second counts, since it also carries
/// the audio position that acknowledges the buffer.
fn extract_transcript(protocol: Protocol, json: &serde_json::Value) -> Option<(String, bool)> {
    match protocol {
        Protocol::Deepgram => {
            json.get("channel")?;
            let transcript = json.pointer("/channel/alternatives/0/transcript")?.as_str()?;
            // Words the multilingual model heard as a third language are dropped (see
            // `without_foreign_words`). Words are only tagged in "multi" sessions, so
            // a session locked to Hebrew or English is left alone.
            let cleaned = json
                .pointer("/channel/alternatives/0")
                .and_then(crate::api_transcribe::without_foreign_words);
            let is_final = json.get("is_final").and_then(|b| b.as_bool()).unwrap_or(false);
            Some((cleaned.unwrap_or_else(|| transcript.to_string()), is_final))
        }
        Protocol::ElevenLabs => {
            let is_final = match json.get("message_type")?.as_str()? {
                "partial_transcript" => false,
                "committed_transcript_with_timestamps" => true,
                _ => return None,
            };
            Some((json.get("text")?.as_str()?.trim().to_string(), is_final))
        }
    }
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

    // Skip non-transcript message types (Metadata, SpeechStarted, session_started, etc.)
    let Some((transcript, is_final)) = extract_transcript(ctx.protocol, &json) else {
        return;
    };
    let transcript_owned = crate::dictionary::apply(&transcript);
    let transcript = transcript_owned.as_str();

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
            401 | 403 => "מפתח ה-API של שירות התמלול לא תקין. אפשר לעדכן אותו בהגדרות.".to_string(),
            402 => "נגמר הקרדיט בשירות התמלול. אפשר להוסיף קרדיט או להחליף ספק בהגדרות.".to_string(),
            429 => "חרגת ממגבלת השימוש בשירות התמלול. נסה שוב בעוד רגע.".to_string(),
            400 => "שירות התמלול דחה את הבקשה (400). ייתכן ששפת התמלול לא נתמכת בזמן אמת.".to_string(),
            code => format!("שגיאה בשירות התמלול (HTTP {})", code),
        },
        WsErr::Io(io) => format!("אין חיבור לשירות התמלול - {}", io),
        _ => format!("שגיאת streaming: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_gain_boosts_a_quiet_session_only() {
        assert_eq!(live_gain(0.0), 1.0);
        assert_eq!(live_gain(0.005), 1.0); // near-silence only
        assert_eq!(live_gain(0.01), 1.0); // the ramp starts at 1, no jump
        assert!((live_gain(0.015) - 4.5).abs() < 1e-4); // halfway up the ramp
        assert_eq!(live_gain(0.03), LIVE_MAX_GAIN);
        assert!((live_gain(0.1) - 5.0).abs() < 1e-4);
        assert_eq!(live_gain(0.6), 1.0); // normal mic
        let pcm = to_pcm16_with_gain(&[0.02, -0.02, 0.9], 30.0);
        assert_eq!(pcm, vec![19660, -19660, 32767]);
    }

    #[test]
    fn one_click_does_not_set_the_session_gain() {
        let mut t = PeakTracker::default();
        // The hotkey click, two chunks loud, then quiet speech.
        assert_eq!(t.push(0.6), 0.6);
        t.push(0.5);
        let mut level = 0.0;
        for i in 0..50 {
            level = t.push(0.04 + 0.0001 * i as f32);
        }
        assert!(level < 0.05, "level {level} still set by the click");
        assert!(live_gain(level) > 5.0);
        // Pauses never raise the gain: quieter chunks leave the level alone.
        assert_eq!(t.push(0.001), level);
        // Speech that really gets louder lowers it.
        for _ in 0..PEAK_RANK {
            t.push(0.4);
        }
        assert_eq!(t.push(0.0), 0.4);
    }

    #[test]
    fn leftover_audio_only_when_the_connection_stayed_dead() {
        let mut unacked = Unacked::default();
        unacked.push(&[16384, -16384, 0]);
        // Alive: the service got everything, nothing to upload again.
        assert!(leftover_audio(false, &unacked).is_empty());
        // Dead: the untranscribed audio comes back as f32 for an upload.
        assert_eq!(leftover_audio(true, &unacked), vec![0.5, -0.5, 0.0]);
        // Acknowledged audio is not returned.
        unacked.ack(2);
        assert_eq!(leftover_audio(true, &unacked), vec![0.0]);
    }

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
            protocol: Protocol::Deepgram,
        }
    }

    // ── ElevenLabs ──

    fn el_text(v: serde_json::Value) -> Message {
        Message::Text(v.to_string().into())
    }

    fn el_commit(text: &str, end: f64) -> Vec<Message> {
        vec![
            el_text(serde_json::json!({"message_type":"committed_transcript","text":text})),
            el_text(serde_json::json!({
                "message_type":"committed_transcript_with_timestamps","text":text,
                "words":[{"text":text,"start":0.0,"end":end,"type":"word"}]
            })),
        ]
    }

    /// Fake ElevenLabs: answers `session_started`, then runs `script`.
    async fn el_session<F, Fut>(script: F) -> (Arc<StreamingSession>, JoinHandle<()>)
    where
        F: FnOnce(WebSocketStream<TcpStream>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let service = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
            ws.send(el_text(serde_json::json!({"message_type":"session_started"}))).await.unwrap();
            script(ws).await;
        });
        let endpoint = Endpoint {
            url: format!("ws://{address}"),
            auth: None,
            protocol: Protocol::ElevenLabs,
        };
        let session = StreamingSession::open_session(endpoint, None, false, false).await.unwrap();
        (session, service)
    }

    fn chunk_of(m: &Message) -> serde_json::Value {
        match m {
            Message::Text(t) => serde_json::from_str(t).unwrap(),
            other => panic!("ElevenLabs audio must be JSON text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn elevenlabs_dictation_keeps_committed_text_once_and_ends_on_the_stop_commit() {
        let (session, service) = el_session(|mut ws| async move {
            let audio = chunk_of(&ws.next().await.unwrap().unwrap());
            assert_eq!(audio["message_type"], "input_audio_chunk");
            assert_eq!(audio["commit"], false);
            // 320 samples = 640 bytes = 856 base64 characters.
            assert_eq!(audio["audio_base_64"].as_str().unwrap().len(), 856);
            ws.send(el_text(serde_json::json!({"message_type":"partial_transcript","text":"שלו"}))).await.unwrap();
            for m in el_commit("שלום", 0.01) {
                ws.send(m).await.unwrap();
            }
            // The stop: 0.4 s of silence with commit=true, no CloseStream.
            let stop = chunk_of(&ws.next().await.unwrap().unwrap());
            assert_eq!(stop["commit"], true);
            for m in el_commit("עולם", 0.02) {
                ws.send(m).await.unwrap();
            }
            // Leave the socket open: the client must end on its own.
            std::future::pending::<()>().await;
        })
        .await;
        session.send_audio_pcm16(&[0.1; 320]).await.unwrap();
        while session.ctx.unacked().start == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(session.ctx.unacked().start, 160, "acknowledged up to the last word's end");
        let started = std::time::Instant::now();
        let result = session.stop().await;
        service.abort();
        assert_eq!(result.text, "שלום עולם", "each commit counted once");
        assert!(result.warning.is_none(), "{:?}", result.warning);
        assert!(started.elapsed() < Duration::from_secs(5), "drained, not timed out");
    }

    #[tokio::test]
    async fn elevenlabs_bad_key_fails_at_start() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
            ws.send(el_text(serde_json::json!({"message_type":"auth_error","error":"Invalid API key"})))
                .await
                .unwrap();
            let _ = ws.close(None).await;
        });
        let endpoint = Endpoint {
            url: format!("ws://{address}"),
            auth: None,
            protocol: Protocol::ElevenLabs,
        };
        let err = StreamingSession::open_session(endpoint, None, false, false).await.err().unwrap();
        assert_eq!(err, ELEVENLABS_BAD_KEY);
    }

    #[tokio::test]
    async fn elevenlabs_fatal_error_mid_dictation_stops_instead_of_reconnecting() {
        let (session, service) = el_session(|mut ws| async move {
            let _ = ws.next().await;
            ws.send(el_text(serde_json::json!({"message_type":"quota_exceeded","error":"no credit"})))
                .await
                .unwrap();
            let _ = ws.close(None).await;
        })
        .await;
        session.send_audio_pcm16(&[0.1; 160]).await.unwrap();
        service.await.unwrap();
        wait_until_dead(&session).await;
        let err = session.send_audio_pcm16(&[0.1; 160]).await.unwrap_err();
        assert_eq!(err, ELEVENLABS_NO_CREDIT);
        let result = session.stop().await;
        assert_eq!(result.warning.as_deref(), Some(ELEVENLABS_NO_CREDIT));
    }

    #[tokio::test]
    async fn elevenlabs_keep_alive_never_splits_live_speech() {
        let (session, service) = el_session(|_ws| async move {
            std::future::pending::<()>().await;
        })
        .await;
        session.send_audio_pcm16(&[0.1; 160]).await.unwrap();
        let before = session.ctx.unacked().samples.len();
        // Audio went out a moment ago (< SILENT_AFTER_MS): no silence may be inserted.
        spawn_keep_alive(Arc::downgrade(&session.writer), Arc::downgrade(&session.ctx), Duration::from_millis(10));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(session.ctx.unacked().samples.len(), before);
        service.abort();
    }

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(&[0xff, 0xfe, 0x00, 0x01]), "//4AAQ==");
    }

    #[test]
    fn elevenlabs_messages_parse() {
        let partial = serde_json::json!({"message_type":"partial_transcript","text":" היי "});
        assert_eq!(extract_transcript(Protocol::ElevenLabs, &partial), Some(("היי".into(), false)));
        let plain = serde_json::json!({"message_type":"committed_transcript","text":"היי"});
        assert_eq!(extract_transcript(Protocol::ElevenLabs, &plain), None, "counted via _with_timestamps");
        let ts = serde_json::json!({"message_type":"committed_transcript_with_timestamps","text":"היי",
            "words":[{"text":"היי","start":6.54,"end":6.9,"type":"word"},{"text":" ","start":6.9,"end":7.0,"type":"spacing"}]});
        assert_eq!(extract_transcript(Protocol::ElevenLabs, &ts), Some(("היי".into(), true)));
        assert_eq!(elevenlabs_audio_end(&ts), Some(112_000));
        let empty = serde_json::json!({"message_type":"committed_transcript_with_timestamps","text":"","words":null});
        assert_eq!(elevenlabs_audio_end(&empty), None);
        assert_eq!(elevenlabs_error(&ts), None);
        assert_eq!(
            elevenlabs_error(&serde_json::json!({"message_type":"commit_throttled","error":"x"})).map(|e| e.1),
            Some(true),
            "throttling is recoverable"
        );
        assert_eq!(
            elevenlabs_error(&serde_json::json!({"message_type":"auth_error"})),
            Some((ELEVENLABS_BAD_KEY.to_string(), false))
        );
    }

    #[test]
    fn elevenlabs_url_pins_hebrew_without_secondary_on_auto() {
        let auto = elevenlabs_url("auto");
        assert!(auto.contains("model_id=scribe_v3_live"));
        assert!(auto.contains("language_code=he&"));
        assert!(!auto.contains("secondary_languages"));
        assert!(auto.contains("commit_strategy=vad"));
        assert!(auto.contains("include_timestamps=true"));
        let en = elevenlabs_url("en");
        assert!(en.contains("language_code=en&"));
        assert!(!en.contains("secondary_languages"));
        assert!(!elevenlabs_url("he").contains("secondary_languages"));
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
        spawn_keep_alive(
            Arc::downgrade(&session.writer),
            Arc::downgrade(&session.ctx),
            Duration::from_millis(15),
        );
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
