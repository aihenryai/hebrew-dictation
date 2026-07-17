//! Opt-in local-only HTTP API — exposes the last dictated transcript on
//! `127.0.0.1` so other local tools/scripts (agents, pipelines) can read the
//! most recent dictation programmatically, without going through the UI.
//!
//! Off by default (`local_api_enabled` in settings, no UI yet — set it in
//! settings.json): this is a new network listener, so dictation must never
//! depend on it, and it must never appear silently for users who didn't ask
//! for it. Bind failures (e.g. port already taken by another instance) are
//! logged and non-fatal.

use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Response, Server};

/// The last completed transcript, plus a monotonic freshness signal. `seq`
/// increments once per completed utterance so a consumer can tell a NEW
/// dictation from the one it already saw. In-memory only: `seq` resets to 0 on
/// app restart, which is safe because every waiter re-reads its baseline `seq`
/// at call time (see the MCP server's `wait_for_dictation`).
#[derive(Clone, Default)]
pub struct LastTranscript {
    pub text: String,
    pub seq: u64,
    pub at_ms: u64,
}

/// Record a freshly injected transcript: replace text, bump seq, stamp time.
pub fn bump_transcript(last: &mut LastTranscript, text: &str, now_ms: u64) {
    last.text = text.to_string();
    last.seq = last.seq.wrapping_add(1);
    last.at_ms = now_ms;
}

/// Serialize the transcript state for `GET /transcript`.
/// Additive: `text` keeps its existing meaning, `seq`/`at` are new keys, so
/// any existing consumer reading `.text` keeps working unchanged.
pub fn transcript_json(last: &LastTranscript) -> String {
    serde_json::json!({
        "text": last.text,
        "seq": last.seq,
        "at": last.at_ms,
    })
    .to_string()
}

/// Unix epoch millis, saturating to 0 if the clock is before the epoch.
pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Spawn the server on its own blocking OS thread — no async runtime
/// integration needed for a single read-only endpoint.
pub fn start(port: u16, last_transcript: Arc<Mutex<String>>) {
    std::thread::spawn(move || {
        let addr = format!("127.0.0.1:{}", port);
        let server = match Server::http(&addr) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("local_api: failed to bind {}: {}", addr, e);
                return;
            }
        };
        eprintln!("local_api: listening on http://{}", addr);

        for request in server.incoming_requests() {
            let response = if request.method() == &Method::Get && request.url() == "/transcript" {
                let text = last_transcript
                    .lock()
                    .map(|t| t.clone())
                    .unwrap_or_default();
                let body = serde_json::json!({ "text": text }).to_string();
                let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
                    .expect("static header is valid");
                Response::from_string(body).with_header(header)
            } else {
                Response::from_string("{\"error\":\"not found\"}").with_status_code(404)
            };
            let _ = request.respond(response);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_transcript_increments_seq_and_replaces_text() {
        let mut last = LastTranscript::default();
        assert_eq!(last.seq, 0, "fresh state starts at seq 0");
        assert_eq!(last.text, "");
        assert_eq!(last.at_ms, 0);

        bump_transcript(&mut last, "שלום", 1_000);
        assert_eq!(last.seq, 1);
        assert_eq!(last.text, "שלום");
        assert_eq!(last.at_ms, 1_000);

        bump_transcript(&mut last, "עולם", 2_000);
        assert_eq!(last.seq, 2, "each injection bumps seq exactly once");
        assert_eq!(last.text, "עולם");
        assert_eq!(last.at_ms, 2_000);
    }

    #[test]
    fn transcript_json_includes_text_seq_and_at() {
        let mut last = LastTranscript::default();
        bump_transcript(&mut last, "בדיקה", 1_700_000_000_000);

        let v: serde_json::Value = serde_json::from_str(&transcript_json(&last)).unwrap();
        assert_eq!(v["text"], "בדיקה");
        assert_eq!(v["seq"], 1);
        assert_eq!(v["at"], 1_700_000_000_000u64);
    }

    #[test]
    fn transcript_json_on_empty_state_is_valid_json() {
        let last = LastTranscript::default();
        let v: serde_json::Value = serde_json::from_str(&transcript_json(&last)).unwrap();
        assert_eq!(v["text"], "");
        assert_eq!(v["seq"], 0);
        assert_eq!(v["at"], 0);
    }
}
