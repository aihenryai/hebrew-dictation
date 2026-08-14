//! Talks to an already-running `piper1-gpl` HTTP sidecar over localhost.
//! Process lifecycle (spawning that sidecar) lives in `narration_process.rs`;
//! this module only knows how to build its launch args and call its API.

use serde::Serialize;
use std::time::Duration;

/// Seconds of silence piper inserts between sentences. The server's own
/// default is 0.0 — literally no pause at punctuation — which is why the
/// narration ran together and sounded like it ignored the punctuation.
/// This is a **startup-only** flag: unlike `length_scale`, piper's
/// `/synthesize` does not read it per request, so it cannot be exposed as a
/// live control without restarting the sidecar. 0.45s is the value chosen by
/// listening to real Hebrew paragraphs at 0.0 / 0.3 / 0.45 / 0.6.
pub const SENTENCE_SILENCE: f32 = 0.45;

/// Default speech rate. Piper's `length_scale` is *phoneme duration*, so
/// higher = slower. 1.0 (the voice's own default) was judged too fast for
/// Hebrew narration; 1.18 is the chosen default. Users can override it per
/// request via the speed control — see `synthesize`.
pub const DEFAULT_LENGTH_SCALE: f32 = 1.18;

/// Build the argv for `python -m piper.http_server`, given the voice model
/// name, bind host, and port. Pure. `--host` is security-mandatory (spec §3):
/// piper1-gpl's HTTP server defaults to `0.0.0.0` (all network interfaces),
/// so omitting this flag would expose the synthesis endpoint to the LAN.
pub fn build_server_args(model_name: &str, host: &str, port: u16) -> Vec<String> {
    vec![
        "-m".to_string(),
        "piper.http_server".to_string(),
        "-m".to_string(),
        model_name.to_string(),
        "--host".to_string(),
        host.to_string(),
        "--port".to_string(),
        port.to_string(),
        "--sentence-silence".to_string(),
        SENTENCE_SILENCE.to_string(),
        // Server-side fallback only: every real request sends its own
        // length_scale, so this just keeps a sane rate if one ever omits it.
        "--length-scale".to_string(),
        DEFAULT_LENGTH_SCALE.to_string(),
    ]
}

/// Minimal structural check that `bytes` looks like a WAV file: starts with
/// the "RIFF" and "WAVE" magic bytes and has more than just the 44-byte
/// canonical header. NOT full WAV validation — this exists to reject "the
/// sidecar returned an HTML error page" or an empty body, not to validate
/// audio content or codec details.
pub fn looks_like_valid_wav(bytes: &[u8]) -> bool {
    bytes.len() > 44 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

#[derive(Debug)]
pub enum NarrationError {
    /// Couldn't reach the sidecar at all (connection refused, DNS, timeout).
    Unreachable(String),
    /// Sidecar responded, but with a non-2xx status.
    BadResponse(String),
    /// Sidecar responded 2xx, but the body doesn't look like a WAV file.
    InvalidAudio,
}

impl std::fmt::Display for NarrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NarrationError::Unreachable(e) => write!(f, "מנוע הקריינות לא זמין: {e}"),
            NarrationError::BadResponse(e) => write!(f, "מנוע הקריינות החזיר שגיאה: {e}"),
            NarrationError::InvalidAudio => write!(f, "מנוע הקריינות החזיר תשובה לא תקינה"),
        }
    }
}

impl std::error::Error for NarrationError {}

#[derive(Serialize)]
struct SynthesizeRequest<'a> {
    text: &'a str,
    length_scale: f32,
}

/// Clamp a user-supplied speech rate to a range that still produces sane
/// audio. Piper accepts any positive float, but values outside roughly this
/// band stop sounding like speech, and a 0 or negative value from a bad
/// caller would produce garbage rather than an error.
pub fn clamp_length_scale(requested: f32) -> f32 {
    if !requested.is_finite() {
        return DEFAULT_LENGTH_SCALE;
    }
    requested.clamp(0.8, 1.8)
}

/// POST /synthesize on the sidecar, returning raw WAV bytes.
/// Validates the response looks like real audio before returning it —
/// callers never receive a partial/corrupt buffer silently (spec §5).
///
/// `length_scale` is phoneme duration, so higher = slower speech. It is sent
/// per request (piper reads it from the request body), which is what lets the
/// speed control take effect without restarting the sidecar.
pub async fn synthesize(
    client: &reqwest::Client,
    port: u16,
    text: &str,
    length_scale: f32,
) -> Result<Vec<u8>, NarrationError> {
    let url = format!("http://127.0.0.1:{port}/synthesize");
    let resp = client
        .post(&url)
        .json(&SynthesizeRequest {
            text,
            length_scale: clamp_length_scale(length_scale),
        })
        // Synthesis time scales with text length, unlike the fast /info
        // probe — 120s is a generous ceiling for worst-case long-form text,
        // just high enough to guarantee the caller never hangs forever on a
        // wedged sidecar.
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| NarrationError::Unreachable(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(NarrationError::BadResponse(format!("HTTP {}", resp.status())));
    }

    let bytes = resp
        .bytes()
        .await
        // A failure here means the connection dropped mid-transfer after a
        // 2xx status — a transport-level failure, not a documented error
        // response, so it belongs with Unreachable rather than BadResponse.
        .map_err(|e| NarrationError::Unreachable(e.to_string()))?
        .to_vec();

    if !looks_like_valid_wav(&bytes) {
        return Err(NarrationError::InvalidAudio);
    }

    Ok(bytes)
}

/// GET /info on the sidecar. Returns true only on a reachable 2xx response —
/// used both as a readiness probe (Chunk 3) and for user-facing diagnostics.
pub async fn health_check(client: &reqwest::Client, port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/info");
    match client.get(&url).timeout(Duration::from_secs(2)).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_server_args_includes_host_flag_explicitly() {
        let args = build_server_args("he_IL-saspeech-medium", "127.0.0.1", 5758);
        let host_idx = args.iter().position(|a| a == "--host").unwrap();
        assert_eq!(args[host_idx + 1], "127.0.0.1");
    }

    #[test]
    fn build_server_args_passes_the_module_voice_and_port() {
        // Covers the mandatory args that aren't flag-tuning: dropping any of
        // them silently launches the wrong thing (or nothing) rather than
        // failing a narrower flag assertion.
        let args = build_server_args("he_IL-saspeech-medium", "127.0.0.1", 5758);
        assert_eq!(args[0], "-m");
        assert_eq!(args[1], "piper.http_server");
        let voice_idx = args
            .iter()
            .position(|a| a == "he_IL-saspeech-medium")
            .expect("the voice name must be passed");
        assert_eq!(args[voice_idx - 1], "-m");
        let port_idx = args.iter().position(|a| a == "--port").unwrap();
        assert_eq!(args[port_idx + 1], "5758");
    }

    #[test]
    fn build_server_args_sets_a_nonzero_sentence_silence() {
        // Piper's own default is 0.0 — no pause at all between sentences,
        // which is what made narration sound like it ignored punctuation.
        // This flag is startup-only, so if it ever stops being passed there
        // is no per-request fallback to save it.
        let args = build_server_args("he_IL-saspeech-medium", "127.0.0.1", 5758);
        let idx = args
            .iter()
            .position(|a| a == "--sentence-silence")
            .expect("sentence-silence must be passed at startup");
        let value: f32 = args[idx + 1].parse().unwrap();
        assert!(value > 0.0, "a 0 pause reintroduces the run-together bug");
    }

    #[test]
    fn clamp_length_scale_rejects_nonsense_rates() {
        // Guards the speed control: piper accepts any float, so a 0/negative
        // or NaN value would produce garbage audio rather than an error.
        assert_eq!(clamp_length_scale(0.0), 0.8);
        assert_eq!(clamp_length_scale(-5.0), 0.8);
        assert_eq!(clamp_length_scale(99.0), 1.8);
        // All three non-finite values, not just NaN: `clamp` on an infinity
        // would otherwise silently return a range bound instead of the default.
        assert_eq!(clamp_length_scale(f32::NAN), DEFAULT_LENGTH_SCALE);
        assert_eq!(clamp_length_scale(f32::INFINITY), DEFAULT_LENGTH_SCALE);
        assert_eq!(clamp_length_scale(f32::NEG_INFINITY), DEFAULT_LENGTH_SCALE);
    }

    #[test]
    fn clamp_length_scale_passes_through_reasonable_rates() {
        assert_eq!(clamp_length_scale(DEFAULT_LENGTH_SCALE), DEFAULT_LENGTH_SCALE);
        assert_eq!(clamp_length_scale(1.0), 1.0);
    }

    #[test]
    fn looks_like_valid_wav_accepts_real_header() {
        let mut fake_wav = b"RIFF".to_vec();
        fake_wav.extend_from_slice(&[0u8; 4]); // chunk size, don't care
        fake_wav.extend_from_slice(b"WAVE");
        fake_wav.extend_from_slice(&[0u8; 100]); // pretend audio data
        assert!(looks_like_valid_wav(&fake_wav));
    }

    #[test]
    fn looks_like_valid_wav_rejects_empty_body() {
        assert!(!looks_like_valid_wav(&[]));
    }

    #[test]
    fn looks_like_valid_wav_rejects_html_error_page() {
        let html = b"<html><body>500 Internal Server Error</body></html>";
        assert!(!looks_like_valid_wav(html));
    }

    #[test]
    fn looks_like_valid_wav_rejects_truncated_header() {
        assert!(!looks_like_valid_wav(b"RIFF\x00\x00\x00\x00"));
    }

    // The listener thread's `incoming_requests()` loop runs for the life of
    // the test binary (it's never explicitly stopped) — intentional, not a
    // leak to "fix": each test gets its own OS-assigned port, and the thread
    // dies with the process. Don't chase this as a phantom hang.
    fn spawn_fake_sidecar(
        respond: impl Fn(&tiny_http::Request) -> (u16, Vec<u8>) + Send + 'static,
    ) -> u16 {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let (status, body) = respond(&request);
                let response = tiny_http::Response::from_data(body)
                    .with_status_code(tiny_http::StatusCode(status));
                let _ = request.respond(response);
            }
        });
        port
    }

    #[tokio::test]
    async fn synthesize_returns_wav_bytes_on_success() {
        let mut fake_wav = b"RIFF".to_vec();
        fake_wav.extend_from_slice(&[0u8; 4]);
        fake_wav.extend_from_slice(b"WAVE");
        fake_wav.extend_from_slice(&[0u8; 50]);
        let wav_for_server = fake_wav.clone();

        let port = spawn_fake_sidecar(move |req| {
            assert_eq!(req.method(), &tiny_http::Method::Post);
            assert_eq!(req.url(), "/synthesize");
            (200, wav_for_server.clone())
        });

        let client = reqwest::Client::new();
        let result = synthesize(&client, port, "שלום עולם", DEFAULT_LENGTH_SCALE).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), fake_wav);
    }

    #[tokio::test]
    async fn synthesize_rejects_non_200_as_bad_response() {
        let port = spawn_fake_sidecar(|_req| (500, b"error".to_vec()));

        let client = reqwest::Client::new();
        let result = synthesize(&client, port, "טקסט", DEFAULT_LENGTH_SCALE).await;

        assert!(matches!(result, Err(NarrationError::BadResponse(_))));
    }

    #[tokio::test]
    async fn synthesize_rejects_non_wav_body_as_invalid_audio() {
        let port = spawn_fake_sidecar(|_req| (200, b"<html>not audio</html>".to_vec()));

        let client = reqwest::Client::new();
        let result = synthesize(&client, port, "טקסט", DEFAULT_LENGTH_SCALE).await;

        assert!(matches!(result, Err(NarrationError::InvalidAudio)));
    }

    #[tokio::test]
    async fn synthesize_reports_unreachable_on_connection_refused() {
        // Port 1 is a reserved/unassignable port — nothing will ever listen
        // there, so this reliably reproduces "connection refused" without
        // racing a real bind/unbind cycle. A short client-side timeout is
        // cheap insurance against a hang if some local firewall/AV/VPN ever
        // intercepts loopback traffic instead of refusing the connection.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let result = synthesize(&client, 1, "טקסט", DEFAULT_LENGTH_SCALE).await;

        assert!(matches!(result, Err(NarrationError::Unreachable(_))));
    }

    #[tokio::test]
    async fn health_check_true_on_reachable_200() {
        let port = spawn_fake_sidecar(|req| {
            assert_eq!(req.method(), &tiny_http::Method::Get);
            assert_eq!(req.url(), "/info");
            (200, b"{}".to_vec())
        });

        let client = reqwest::Client::new();
        assert!(health_check(&client, port).await);
    }

    #[tokio::test]
    async fn health_check_false_when_unreachable() {
        let client = reqwest::Client::new();
        assert!(!health_check(&client, 1).await);
    }
}
