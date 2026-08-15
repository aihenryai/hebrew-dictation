//! Talks to an already-running narration sidecar over localhost.
//! Process lifecycle (spawning that sidecar) lives in `narration_process.rs`;
//! this module only knows how to build its launch args and call its API.

use serde::Serialize;
use std::time::Duration;

/// Default seconds of silence between sentences. Piper's server defaulted
/// this to 0.0 — no pause at punctuation at all — and could only read it at
/// startup. Our own sidecar reads it per request, so it is now a live
/// control; this is just the starting value, chosen by listening to real
/// Hebrew paragraphs at 0.0 / 0.3 / 0.45 / 0.6.
pub const SENTENCE_SILENCE: f32 = 0.45;

/// Default speech rate. `length_scale` is *phoneme duration*, so higher =
/// slower. 1.0 (the voice's own default) was judged too fast for Hebrew
/// narration; 1.18 is the chosen default. Overridable per request.
pub const DEFAULT_LENGTH_SCALE: f32 = 1.18;

/// Generator noise and phoneme-width noise, taken from the voice config's
/// own inference defaults. Exposed so the user can flatten or liven the
/// delivery, or sharpen how distinctly syllables separate.
pub const DEFAULT_NOISE_SCALE: f32 = 0.667;
pub const DEFAULT_NOISE_W: f32 = 0.8;

/// Paths the narration sidecar needs on its command line. Grouped into a
/// struct because there are four of them and positional args of the same type
/// are easy to transpose silently.
pub struct ServerPaths<'a> {
    /// Our own server script, written to app-data during provisioning.
    pub script: &'a str,
    /// The VITS voice `.onnx`.
    pub voice: &'a str,
    /// The voice's `.onnx.json` config (phoneme map, sample rate).
    pub config: &'a str,
    /// The Phonikud diacritizer `.onnx`.
    pub phonikud: &'a str,
    /// Local tokenizer for the diacritizer, so it never reaches the network.
    pub tokenizer: &'a str,
}

/// Build the argv for our narration server script.
///
/// This runs our own script rather than `python -m piper.http_server`: the
/// voices declare `phoneme_type: "raw"` and expect stress-marked IPA, which
/// piper's server cannot produce — it phonemizes from nikud, which encodes
/// vowels but not stress.
///
/// `--host` is security-mandatory: binding all interfaces would expose the
/// synthesis endpoint to the LAN.
pub fn build_server_args(paths: &ServerPaths<'_>, host: &str, port: u16) -> Vec<String> {
    vec![
        paths.script.to_string(),
        "--model".to_string(),
        paths.voice.to_string(),
        "--config".to_string(),
        paths.config.to_string(),
        "--phonikud".to_string(),
        paths.phonikud.to_string(),
        "--tokenizer".to_string(),
        paths.tokenizer.to_string(),
        "--host".to_string(),
        host.to_string(),
        "--port".to_string(),
        port.to_string(),
        // Startup fallbacks only: every real request carries its own values,
        // so these just keep the sidecar sane if one ever omits them.
        "--sentence-silence".to_string(),
        SENTENCE_SILENCE.to_string(),
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

/// User-tunable synthesis knobs. All are sent per request; the sidecar falls
/// back to its startup defaults for anything omitted.
#[derive(Clone, Copy, Debug, serde::Deserialize)]
pub struct NarrationParams {
    /// Phoneme duration — higher is SLOWER.
    pub length_scale: f32,
    /// Seconds of silence between sentences.
    pub sentence_silence: f32,
    /// Generator noise: higher is more expressive/variable, lower is flatter.
    pub noise_scale: f32,
    /// Phoneme-width noise: affects how distinctly syllables separate.
    pub noise_w: f32,
}

impl Default for NarrationParams {
    fn default() -> Self {
        Self {
            length_scale: DEFAULT_LENGTH_SCALE,
            sentence_silence: SENTENCE_SILENCE,
            noise_scale: DEFAULT_NOISE_SCALE,
            noise_w: DEFAULT_NOISE_W,
        }
    }
}

impl NarrationParams {
    /// Clamp every field into a range that still produces speech. The sidecar
    /// accepts any float, so a bad value would yield garbage audio rather than
    /// an error — clamping here keeps that impossible from the UI or a caller.
    pub fn clamped(self) -> Self {
        fn c(v: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                fallback
            }
        }
        Self {
            length_scale: c(self.length_scale, 0.8, 1.8, DEFAULT_LENGTH_SCALE),
            sentence_silence: c(self.sentence_silence, 0.0, 2.0, SENTENCE_SILENCE),
            noise_scale: c(self.noise_scale, 0.0, 1.2, DEFAULT_NOISE_SCALE),
            noise_w: c(self.noise_w, 0.0, 1.2, DEFAULT_NOISE_W),
        }
    }
}

#[derive(Serialize)]
struct SynthesizeRequest<'a> {
    text: &'a str,
    length_scale: f32,
    sentence_silence: f32,
    noise_scale: f32,
    noise_w: f32,
}

/// POST /synthesize on the sidecar, returning raw WAV bytes.
/// Validates the response looks like real audio before returning it —
/// callers never receive a partial/corrupt buffer silently (spec §5).
///
/// Every knob in `params` is sent per request, which is what lets the UI
/// controls take effect without restarting the sidecar. Values are clamped
/// here rather than trusted.
pub async fn synthesize(
    client: &reqwest::Client,
    port: u16,
    text: &str,
    params: NarrationParams,
) -> Result<Vec<u8>, NarrationError> {
    let p = params.clamped();
    let url = format!("http://127.0.0.1:{port}/synthesize");
    let resp = client
        .post(&url)
        .json(&SynthesizeRequest {
            text,
            length_scale: p.length_scale,
            sentence_silence: p.sentence_silence,
            noise_scale: p.noise_scale,
            noise_w: p.noise_w,
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

    fn test_paths() -> ServerPaths<'static> {
        ServerPaths {
            script: "C:\\app\\narration_server.py",
            voice: "C:\\app\\michael.onnx",
            config: "C:\\app\\model.config.json",
            phonikud: "C:\\app\\phonikud-1.0.int8.onnx",
            tokenizer: "C:\\app\\tokenizer.json",
        }
    }

    #[test]
    fn build_server_args_includes_host_flag_explicitly() {
        let args = build_server_args(&test_paths(), "127.0.0.1", 5758);
        let host_idx = args.iter().position(|a| a == "--host").unwrap();
        assert_eq!(args[host_idx + 1], "127.0.0.1");
    }

    #[test]
    fn build_server_args_passes_every_required_path_and_the_port() {
        // All five paths are mandatory and are easy to transpose; a missing
        // one surfaces as a confusing Python argparse error at spawn time
        // rather than here.
        let args = build_server_args(&test_paths(), "127.0.0.1", 5758);
        assert_eq!(args[0], "C:\\app\\narration_server.py", "script must be argv[0]");
        for (flag, expected) in [
            ("--model", "C:\\app\\michael.onnx"),
            ("--config", "C:\\app\\model.config.json"),
            ("--phonikud", "C:\\app\\phonikud-1.0.int8.onnx"),
            ("--tokenizer", "C:\\app\\tokenizer.json"),
            ("--port", "5758"),
        ] {
            let idx = args
                .iter()
                .position(|a| a == flag)
                .unwrap_or_else(|| panic!("{flag} must be passed"));
            assert_eq!(args[idx + 1], expected, "wrong value after {flag}");
        }
    }

    #[test]
    fn build_server_args_sets_a_nonzero_sentence_silence() {
        // The pause between sentences is startup-only — unlike length_scale
        // there is no per-request fallback, so losing this flag silently
        // reintroduces the run-together delivery.
        let args = build_server_args(&test_paths(), "127.0.0.1", 5758);
        let idx = args
            .iter()
            .position(|a| a == "--sentence-silence")
            .expect("sentence-silence must be passed at startup");
        let value: f32 = args[idx + 1].parse().unwrap();
        assert!(value > 0.0, "a 0 pause reintroduces the run-together bug");
    }

    #[test]
    fn clamped_rejects_nonsense_values_on_every_field() {
        // The sidecar accepts any float, so out-of-range or non-finite values
        // would produce garbage audio rather than an error.
        let wild = NarrationParams {
            length_scale: 99.0,
            sentence_silence: -3.0,
            noise_scale: 50.0,
            noise_w: f32::NAN,
        }
        .clamped();
        assert_eq!(wild.length_scale, 1.8);
        assert_eq!(wild.sentence_silence, 0.0);
        assert_eq!(wild.noise_scale, 1.2);
        assert_eq!(wild.noise_w, DEFAULT_NOISE_W, "NaN must fall back, not clamp");
    }

    #[test]
    fn clamped_maps_every_non_finite_to_its_default() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let p = NarrationParams {
                length_scale: bad,
                sentence_silence: bad,
                noise_scale: bad,
                noise_w: bad,
            }
            .clamped();
            assert_eq!(p.length_scale, DEFAULT_LENGTH_SCALE);
            assert_eq!(p.sentence_silence, SENTENCE_SILENCE);
            assert_eq!(p.noise_scale, DEFAULT_NOISE_SCALE);
            assert_eq!(p.noise_w, DEFAULT_NOISE_W);
        }
    }

    #[test]
    fn clamped_passes_through_reasonable_values() {
        let p = NarrationParams::default().clamped();
        assert_eq!(p.length_scale, DEFAULT_LENGTH_SCALE);
        assert_eq!(p.sentence_silence, SENTENCE_SILENCE);
        assert_eq!(p.noise_scale, DEFAULT_NOISE_SCALE);
        assert_eq!(p.noise_w, DEFAULT_NOISE_W);
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
        let result = synthesize(&client, port, "שלום עולם", NarrationParams::default()).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), fake_wav);
    }

    #[tokio::test]
    async fn synthesize_rejects_non_200_as_bad_response() {
        let port = spawn_fake_sidecar(|_req| (500, b"error".to_vec()));

        let client = reqwest::Client::new();
        let result = synthesize(&client, port, "טקסט", NarrationParams::default()).await;

        assert!(matches!(result, Err(NarrationError::BadResponse(_))));
    }

    #[tokio::test]
    async fn synthesize_rejects_non_wav_body_as_invalid_audio() {
        let port = spawn_fake_sidecar(|_req| (200, b"<html>not audio</html>".to_vec()));

        let client = reqwest::Client::new();
        let result = synthesize(&client, port, "טקסט", NarrationParams::default()).await;

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
        let result = synthesize(&client, 1, "טקסט", NarrationParams::default()).await;

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
