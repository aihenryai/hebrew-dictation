//! Opt-in local-only HTTP API — exposes the last completed transcript on
//! `127.0.0.1` so other local tools/scripts (agents, pipelines) can read it
//! programmatically, without going through the UI. Usually that's the most
//! recent dictation, but the user can also paste an older transcript from the
//! UI — see `LastTranscript` for what the transcript and its `seq` mean.
//!
//! Off by default (`local_api_enabled` in settings, no UI yet — set it in
//! settings.json): this is a new network listener, so dictation must never
//! depend on it, and it must never appear silently for users who didn't ask
//! for it. Bind failures (e.g. port already taken by another instance) are
//! logged and non-fatal.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Response, Server};

/// Generates a fresh random token without pulling in the `rand` crate for a
/// couple of call sites: `RandomState::new()` re-seeds from the OS CSPRNG
/// (`BCryptGenRandom` on Windows) on every call, so four independent 64-bit
/// draws give 256 bits of OS-sourced entropy. `pub(crate)`: also used by
/// `narration_process` for the sidecar-adoption nonce (same shape of problem
/// — a local capability secret — different consumer).
pub(crate) fn generate_token() -> String {
    let mut token = String::with_capacity(64);
    for i in 0u64..4 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(i);
        token.push_str(&format!("{:016x}", hasher.finish()));
    }
    token
}

/// Where the current launch's token is written for the MCP server (or any
/// other local automation) to read. Regenerated every app start — no
/// rotation logic needed, a stale token from a previous launch just stops
/// matching once this one overwrites the file.
///
/// NTFS ACLs already restrict `%APPDATA%` to the owning user (+ Administrators)
/// by default — there is no POSIX chmod(0600) equivalent to additionally apply
/// on Windows, unlike on macOS/Linux where the same app-data tree is used.
fn token_path() -> std::path::PathBuf {
    let app_data = dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    app_data.join("hebrew-dictation").join("local_api_token")
}

/// Persist the token so a local client can discover it. Best-effort: a write
/// failure (e.g. a locked file) just means no local reader can authenticate
/// yet — the server still enforces the in-memory token either way, so this
/// never *weakens* the check, it only affects discoverability.
fn write_token_file(token: &str) {
    let path = token_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("local_api: failed to create token dir {}: {}", parent.display(), e);
            return;
        }
    }
    if let Err(e) = std::fs::write(&path, token) {
        eprintln!("local_api: failed to write token file {}: {}", path.display(), e);
    }
}

/// Checks a request's `Authorization: Bearer <token>` header against the
/// server's token. Constant-time-ish via a full-string compare is not
/// attempted here — timing side-channels over a loopback socket serving one
/// low-value local endpoint are not this feature's threat model; the header
/// is the actual barrier (see the module doc for what it defends against).
fn has_valid_token(headers: &[Header], expected: &str) -> bool {
    let Some(auth) = headers
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("authorization"))
    else {
        return false;
    };
    auth.value
        .as_str()
        .strip_prefix("Bearer ")
        .is_some_and(|got| got == expected)
}

/// Checks a request's `Host` header names this server specifically —
/// `127.0.0.1:<port>` or `localhost:<port>` — rather than trusting whatever a
/// DNS-rebound page's browser sent. Loopback binding alone does not block
/// this: a page on `evil.example` whose DNS TTL rebinds to 127.0.0.1 becomes
/// same-origin with the listener, and neither the OS nor the TCP layer
/// distinguishes that request from a legitimate local one — only the `Host`
/// header (which the browser sets from the ORIGINAL hostname, not the
/// resolved IP) can tell them apart.
fn has_valid_host(headers: &[Header], port: u16) -> bool {
    let Some(host) = headers
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("host"))
    else {
        // No Host header at all — not a browser request (curl/MCP client
        // without one). Loopback + a valid token is still required either
        // way, so this is not a bypass.
        return true;
    };
    let value = host.value.as_str();
    value == format!("127.0.0.1:{port}") || value == format!("localhost:{port}")
}

/// The last completed transcript, plus a monotonic freshness signal, so a
/// consumer can tell a NEW transcript from the one it already saw.
///
/// `seq` counts completed-transcript *events*, not spoken utterances: it covers
/// a whole non-streaming dictation, a whole streaming session, and any
/// transcript the user pastes from the UI (an edited transcript, or an older
/// batch-file result). A paste bumps `seq` too, and stamps `at_ms` with the
/// paste time, not the recording time — so `at_ms` is when this transcript was
/// recorded here, not when the audio was spoken.
///
/// In-memory only: `seq` resets to 0 on app restart, which is safe because
/// every waiter re-reads its baseline `seq` at call time (see the MCP server's
/// `wait_for_dictation`).
#[derive(Clone, Default)]
pub struct LastTranscript {
    pub text: String,
    pub seq: u64,
    pub at_ms: u64,
}

/// Record a completed transcript: replace text, bump seq, stamp time.
/// Private on purpose: `record_utterance` is the only way to mutate the state,
/// so the blank-text guard cannot be bypassed by a caller reaching past it.
fn bump_transcript(last: &mut LastTranscript, text: &str, now_ms: u64) {
    last.text = text.to_string();
    last.seq = last.seq.wrapping_add(1);
    last.at_ms = now_ms;
}

/// Record a completed transcript: a whole non-streaming dictation, a whole
/// streaming session, or a transcript pasted from the UI. Callers pass the
/// COMPLETE text — never a streaming segment, or `seq` would tick per fragment.
///
/// Blank text (empty or whitespace-only) is ignored so a no-op recording never
/// looks like a new dictation — the UI's paste button has no frontend guard, so
/// a cleared textarea reaches us as `" "`. The guard trims only to decide; the
/// text is recorded verbatim.
pub fn record_utterance(last: &Mutex<LastTranscript>, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    // Recover from a poisoned lock rather than skipping forever: poisoning is
    // permanent, and LastTranscript has no cross-field invariant a panic could
    // break. Matches this crate's existing `into_inner()` convention.
    let mut guard = last.lock().unwrap_or_else(|e| e.into_inner());
    bump_transcript(&mut guard, text, now_unix_ms());
}

/// Serialize the transcript state for `GET /transcript`.
/// `seq`/`at` are additive keys, so an existing consumer reading `.text` still
/// parses fine. Note `text`'s *meaning* did change for streaming dictation: it
/// now holds the whole session's transcript rather than the last segment.
/// `at` is `at_ms`: unix epoch milliseconds.
fn transcript_json(last: &LastTranscript) -> String {
    serde_json::json!({
        "text": last.text,
        "seq": last.seq,
        "at": last.at_ms,
    })
    .to_string()
}

/// Unix epoch millis, saturating to 0 if the clock is before the epoch.
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Spawn the server on its own blocking OS thread — no async runtime
/// integration needed for a single read-only endpoint.
///
/// Requires a bearer token on every request (generated fresh this launch,
/// written to `token_path()` for a local client to read) and validates the
/// `Host` header against DNS rebinding — see `has_valid_token`/`has_valid_host`.
/// Before this, loopback binding was the ONLY barrier: any other process
/// running as this user (or a rebound web page) could read every dictated
/// transcript with a plain unauthenticated GET.
pub fn start(port: u16, last_transcript: Arc<Mutex<LastTranscript>>) {
    std::thread::spawn(move || {
        let addr = format!("127.0.0.1:{}", port);
        let server = match Server::http(&addr) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("local_api: failed to bind {}: {}", addr, e);
                return;
            }
        };
        let token = generate_token();
        write_token_file(&token);
        eprintln!("local_api: listening on http://{}", addr);

        for request in server.incoming_requests() {
            let response = if request.method() != &Method::Get || request.url() != "/transcript" {
                Response::from_string("{\"error\":\"not found\"}").with_status_code(404)
            } else if !has_valid_host(request.headers(), port) {
                Response::from_string("{\"error\":\"invalid host\"}").with_status_code(400)
            } else if !has_valid_token(request.headers(), &token) {
                Response::from_string("{\"error\":\"unauthorized\"}").with_status_code(401)
            } else {
                // Recover from a poisoned lock (see `record_utterance`): reporting
                // an empty transcript forever would be indistinguishable from a
                // genuinely empty one, silently hanging every waiting consumer.
                let last = last_transcript.lock().unwrap_or_else(|e| e.into_inner());
                let body = transcript_json(&last);
                let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
                    .expect("static header is valid");
                Response::from_string(body).with_header(header)
            };
            let _ = request.respond(response);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(name: &str, value: &str) -> Header {
        Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
    }

    #[test]
    fn generate_token_returns_high_entropy_distinct_values() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b, "two calls must not collide");
        assert_eq!(a.len(), 64, "4 * 16 hex chars");
    }

    #[test]
    fn has_valid_token_accepts_matching_bearer_header() {
        let headers = [header("Authorization", "Bearer secret123")];
        assert!(has_valid_token(&headers, "secret123"));
    }

    #[test]
    fn has_valid_token_rejects_wrong_token() {
        let headers = [header("Authorization", "Bearer wrong")];
        assert!(!has_valid_token(&headers, "secret123"));
    }

    #[test]
    fn has_valid_token_rejects_missing_header() {
        let headers: [Header; 0] = [];
        assert!(!has_valid_token(&headers, "secret123"));
    }

    #[test]
    fn has_valid_token_rejects_missing_bearer_prefix() {
        let headers = [header("Authorization", "secret123")];
        assert!(!has_valid_token(&headers, "secret123"));
    }

    #[test]
    fn has_valid_token_header_name_is_case_insensitive() {
        let headers = [header("authorization", "Bearer secret123")];
        assert!(has_valid_token(&headers, "secret123"));
    }

    #[test]
    fn has_valid_host_accepts_127_0_0_1_with_matching_port() {
        let headers = [header("Host", "127.0.0.1:5757")];
        assert!(has_valid_host(&headers, 5757));
    }

    #[test]
    fn has_valid_host_accepts_localhost_with_matching_port() {
        let headers = [header("Host", "localhost:5757")];
        assert!(has_valid_host(&headers, 5757));
    }

    #[test]
    fn has_valid_host_rejects_wrong_port() {
        let headers = [header("Host", "127.0.0.1:9999")];
        assert!(!has_valid_host(&headers, 5757));
    }

    #[test]
    fn has_valid_host_rejects_a_rebound_external_hostname() {
        // The DNS-rebinding scenario this guards against: a browser that
        // resolved evil.example to 127.0.0.1 still sends the ORIGINAL
        // hostname in its Host header, not the resolved IP.
        let headers = [header("Host", "evil.example:5757")];
        assert!(!has_valid_host(&headers, 5757));
    }

    #[test]
    fn has_valid_host_allows_missing_header_not_a_bypass_alone() {
        // No Host header (e.g. a plain non-browser HTTP client) is allowed
        // through THIS check — the token check is the actual barrier for
        // such a client, this only stops browser-style rebinding.
        let headers: [Header; 0] = [];
        assert!(has_valid_host(&headers, 5757));
    }

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
    fn record_utterance_ignores_empty_text() {
        let last = Mutex::new(LastTranscript::default());
        record_utterance(&last, "");

        let guard = last.lock().unwrap();
        assert_eq!(guard.seq, 0, "an empty utterance must never bump seq");
        assert_eq!(guard.text, "");
        assert_eq!(guard.at_ms, 0, "an ignored utterance must not stamp the clock");
    }

    #[test]
    fn record_utterance_ignores_whitespace_only_text() {
        let last = Mutex::new(LastTranscript::default());
        // The UI's paste button has no frontend guard, so a whitespace-only
        // textarea reaches this helper. It's a no-op, not a new dictation.
        record_utterance(&last, "   \n\t ");

        let guard = last.lock().unwrap();
        assert_eq!(guard.seq, 0, "whitespace-only text must never bump seq");
        assert_eq!(guard.text, "");
    }

    #[test]
    fn record_utterance_records_text_untrimmed() {
        let last = Mutex::new(LastTranscript::default());
        record_utterance(&last, "  שלום  ");

        let guard = last.lock().unwrap();
        assert_eq!(guard.seq, 1, "surrounding whitespace does not make it a no-op");
        assert_eq!(
            guard.text, "  שלום  ",
            "the guard trims only to decide; the recorded text stays verbatim"
        );
    }

    #[test]
    fn record_utterance_bumps_once_on_non_empty_text() {
        let last = Mutex::new(LastTranscript::default());
        record_utterance(&last, "שלום עולם");

        let guard = last.lock().unwrap();
        assert_eq!(guard.seq, 1);
        assert_eq!(guard.text, "שלום עולם");
        assert!(guard.at_ms > 0, "a recorded utterance stamps the wall clock");
    }

    #[test]
    fn record_utterance_bumps_again_on_second_call() {
        let last = Mutex::new(LastTranscript::default());
        record_utterance(&last, "ראשון");
        record_utterance(&last, "שני");

        let guard = last.lock().unwrap();
        assert_eq!(guard.seq, 2, "each completed transcript bumps seq exactly once");
        assert_eq!(guard.text, "שני");
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
