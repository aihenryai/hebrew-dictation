//! Talks to an already-running `piper1-gpl` HTTP sidecar over localhost.
//! Process lifecycle (spawning that sidecar) lives in `narration_process.rs`;
//! this module only knows how to build its launch args and call its API.

use serde::Serialize;
use std::time::Duration;

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
            NarrationError::Unreachable(e) => write!(f, "מנוע ההקראה לא זמין: {e}"),
            NarrationError::BadResponse(e) => write!(f, "מנוע ההקראה החזיר שגיאה: {e}"),
            NarrationError::InvalidAudio => write!(f, "מנוע ההקראה החזיר תשובה לא תקינה"),
        }
    }
}

impl std::error::Error for NarrationError {}

#[derive(Serialize)]
struct SynthesizeRequest<'a> {
    text: &'a str,
}

/// POST /synthesize on the sidecar, returning raw WAV bytes.
/// Validates the response looks like real audio before returning it —
/// callers never receive a partial/corrupt buffer silently (spec §5).
pub async fn synthesize(
    client: &reqwest::Client,
    port: u16,
    text: &str,
) -> Result<Vec<u8>, NarrationError> {
    let url = format!("http://127.0.0.1:{port}/synthesize");
    let resp = client
        .post(&url)
        .json(&SynthesizeRequest { text })
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
        assert_eq!(
            args,
            vec![
                "-m", "piper.http_server",
                "-m", "he_IL-saspeech-medium",
                "--host", "127.0.0.1",
                "--port", "5758",
            ]
        );
        let host_idx = args.iter().position(|a| a == "--host").unwrap();
        assert_eq!(args[host_idx + 1], "127.0.0.1");
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
        let result = synthesize(&client, port, "שלום עולם").await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), fake_wav);
    }

    #[tokio::test]
    async fn synthesize_rejects_non_200_as_bad_response() {
        let port = spawn_fake_sidecar(|_req| (500, b"error".to_vec()));

        let client = reqwest::Client::new();
        let result = synthesize(&client, port, "טקסט").await;

        assert!(matches!(result, Err(NarrationError::BadResponse(_))));
    }

    #[tokio::test]
    async fn synthesize_rejects_non_wav_body_as_invalid_audio() {
        let port = spawn_fake_sidecar(|_req| (200, b"<html>not audio</html>".to_vec()));

        let client = reqwest::Client::new();
        let result = synthesize(&client, port, "טקסט").await;

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
        let result = synthesize(&client, 1, "טקסט").await;

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
