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
