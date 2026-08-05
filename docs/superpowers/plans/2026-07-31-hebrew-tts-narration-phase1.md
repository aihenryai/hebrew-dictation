# Hebrew TTS Narration Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a local, CPU-only, generic-Hebrew-voice text-to-speech ("הקראה") feature to hebrew-dictation, mirroring the zero-friction local UX `whisper-rs` already provides for speech-to-text.

**Architecture:** The app manages an isolated Python runtime (provisioned on-demand via the `uv` package manager, hash-verified, invisible to the user) that runs `piper1-gpl`'s built-in HTTP server as a warm background sidecar on `127.0.0.1`. Rust owns the sidecar's full lifecycle (spawn, health-check, crash-recovery, guaranteed teardown including a Windows Job Object for crash cases and a startup sweep for orphans) and talks to it over local HTTP. The frontend gets a new "הקראה" screen: paste Hebrew text, get a WAV back, play it, optionally save it.

**Tech Stack:** Rust (Tauri v2, `tokio::process`, `reqwest`), `piper1-gpl` (Python, GPL-3.0, run as a subprocess — never linked into the app), `uv` (Python env manager, MIT), the `he_IL-saspeech-medium` Piper voice (SASPEECH corpus), React/TypeScript frontend.

**Spec:** `docs/superpowers/specs/2026-07-31-hebrew-tts-narration-phase1-design.md` (✅ Approved, commit `130664f`)

---

## File Structure

| File | Responsibility |
|---|---|
| `src-tauri/src/narration.rs` (NEW) | Talking to an *already-running* sidecar: pure args-builder, pure WAV sanity check, async `synthesize`/`health_check` over HTTP. No process management, no provisioning. |
| `src-tauri/src/narration_process.rs` (NEW) | Sidecar **process lifecycle**: spawn, Windows Job Object assignment, teardown, startup stale-sweep. Depends on `narration::health_check` for readiness: calls it, doesn't reimplement it. |
| `src-tauri/src/narration_provision.rs` (NEW) | **Provisioning state machine**: is the engine installed? If not, download `uv`, create a venv, install `piper-tts`, download the voice, write the atomic completion marker. Idempotent — safe to re-run after a partial failure. |
| `src-tauri/src/model.rs` (MODIFY) | Extract a generic hash-verified download helper out of `download_model` so `narration_provision.rs` reuses it for the `uv` binary and the voice file, instead of duplicating download/verify/progress logic (spec §2.4, §2.6 — DRY). |
| `src-tauri/src/settings.rs` (MODIFY) | Add `narration_port: u16` (default `5758`), `#[serde(default)]` for back-compat. |
| `src-tauri/src/lib.rs` (MODIFY) | `AppState` field for the narration server handle; 3 new Tauri commands; register them in `invoke_handler!`; call the stale-sweep from `setup()`. |
| `src/App.tsx` (MODIFY) | New `"narration"` `AppView` variant, the הקראה screen, and an entry-point button from the home screen. |

Each Rust file has exactly one responsibility, mirroring the existing split between `local_api.rs` (server/state), `enhance.rs` (LLM call), and `batch.rs` (orchestration + pure guards) — no file here does two of "talk to the sidecar", "manage its process", and "install it" at once, so each is independently testable and none of them should grow past a few hundred lines (comparable to `local_api.rs`'s 217 or `enhance.rs`'s 309).

---

## Chunk 1: Spike & Fact-Finding Gate

**No app code in this chunk.** Per spec §6, several facts are load-bearing but currently unverified, and the Hebrew voice's actual quality has never been listened to or measured. This chunk is a hard gate: Chunk 2 (which hardcodes exact versions/hashes/URLs) cannot start until this chunk's findings are written down.

**Files:**
- Create: `docs/superpowers/plans/2026-07-31-hebrew-tts-narration-phase1-spike-findings.md`

This is investigation work done by hand in a scratch directory (NOT inside the app repo's tracked source — use e.g. a temp folder), with results recorded into the findings doc. There is no automated test for "does the voice sound acceptable" — the gate is a human/ASR judgment call, not a green checkmark.

- [ ] **Step 1: Set up a throwaway Python environment and install piper-tts**

```powershell
mkdir C:\Users\אורח\scratch\piper-spike
cd C:\Users\אורח\scratch\piper-spike
python -m venv venv
.\venv\Scripts\Activate.ps1
pip install "piper-tts>=1.6.0"
pip show piper-tts
python -m piper.http_server --help
```

Record in the findings doc:
- The exact installed version (e.g. `1.6.0`, `1.6.1`) — this becomes the pinned version in Chunk 3.
- Whether the plain `pip install piper-tts` was sufficient for `python -m piper.http_server --help` to succeed, or whether it failed on a missing import (e.g. `flask`) — if it failed, `pip install "piper-tts[http]"` and re-run, and record which extra was actually required.

- [ ] **Step 2: Download the Hebrew voice and inspect its license**

```powershell
python -m piper.download_voices he_IL-saspeech-medium
```

If that command doesn't exist in the installed version, download manually from `https://huggingface.co/rhasspy/piper-voices/tree/main/he/he_IL/saspeech/medium` (the `.onnx` and `.onnx.json` files).

Record in the findings doc:
- Exact download URLs for both files.
- File sizes in bytes (for later hash-verification, mirroring `model.rs`'s `(name, url, expected_size, sha256_hex)` table).
- SHA-256 of the `.onnx` file (`certutil -hashfile <file> SHA256` on Windows, or `sha256sum` in Git Bash).
- The voice's MODEL_CARD / README text (usually alongside the voice files or linked from the `piper-voices` repo) — specifically any license or attribution requirement beyond the repo's blanket MIT, since this voice is one real person's (Shaul Amsterdamski's) recorded speech from the SASPEECH/Roboshaul corpus. Quote the relevant license text verbatim into the findings doc.

- [ ] **Step 3: Start the HTTP server and confirm the API shape**

```powershell
python -m piper.http_server -m he_IL-saspeech-medium --host 127.0.0.1 --port 5758
```

In a second PowerShell window:
```powershell
Invoke-RestMethod http://127.0.0.1:5758/info

Measure-Command {
    Invoke-WebRequest -Method Post -Uri http://127.0.0.1:5758/synthesize `
        -ContentType "application/json" `
        -Body (@{text="שלום עולם"} | ConvertTo-Json) `
        -OutFile test1.wav
}
```

`Measure-Command` prints a `TotalSeconds` field — that is the per-request latency measurement once the server is warm (run it a second time after the first request to exclude any one-time JIT/cache warmup inside the server itself). For cold-start time, wrap the server launch similarly or just note the wall-clock gap between starting `python -m piper.http_server ...` in the first window and `Invoke-RestMethod .../info` first succeeding in the second.

Record in the findings doc:
- Exact JSON shape of `/info`'s response.
- Confirm `/synthesize` returns a valid WAV body — open `test1.wav` in any player to sanity-check it's audible at all before moving to quality judgment.
- Cold-start time: seconds from launching the server process to `/info` responding successfully.
- Per-request latency: the `TotalSeconds` from `Measure-Command` above, run against a ~50-word paragraph once the server is warm.
- Warm-idle RAM: `Get-Process python | Select-Object WorkingSet64` a minute after the last request, server otherwise idle.

- [ ] **Step 4: Objective quality verification — the actual gate**

Generate WAV for each of these test sentences via `/synthesize`, saving each output:

1. A short, simple sentence: `"אני בודק את המערכת."`
2. A multi-clause sentence: `"אחרי שסיימתי את הישיבה, יצאתי לקנות קפה, ופגשתי חבר ותיק ברחוב."`
3. The known Hebrew-TTS trap word (documented in `HANDOFF-TTS-VOICE-GENERATION.md` as failing on every model tested so far — expected to fail here too, record it, don't chase a fix): `"בדקתי את כל המתגים בלוח החשמל."`
4. A mixed-content sentence with a brand name and digits: `"הורדתי את ChatGPT 5 אתמול."`
5. A longer paragraph (~80-100 words) of realistic narration text, to sanity-check nothing degrades over a longer generation.

For each output WAV, run it back through Whisper ASR (reuse this very app's own transcription — batch-transcribe the WAV file via `hebrew-dictation`'s own file-upload batch mode with a downloaded Hebrew whisper model, OR any other available Whisper access) and compare the transcription against the original input text. **Do not judge by ear alone — the transcription comparison is the actual measurement.** Record, for each sentence: the ASR transcription, and a word-level diff against the original.

- [ ] **Step 5: Write the go/no-go verdict**

In the findings doc, write a `## Verdict` section stating explicitly: **GO** (quality acceptable, proceed to Chunk 2) or **NO-GO** (quality too poor — stop, do not proceed to any further chunk, report back with the recordings and ASR transcripts for a fresh decision per spec §6.3). A GO verdict must be based on the ASR comparisons from Step 4, not a subjective impression.

- [ ] **Step 6: Consolidate all pinned facts**

At the top of the findings doc, add a `## Pinned Facts (for Chunk 2+)` section listing, in one place:
- `piper-tts` exact version to pin
- Whether an extra (e.g. `[http]`) is required in the pip install
- Voice `.onnx` and `.onnx.json` download URLs, sizes, and SHA-256 hashes
- Nakdimon model source (bundled in the `piper-tts` wheel, or a separate first-use download — if the latter, its URL/size/hash too)
- License/attribution text to surface in the app's credits screen
- Cold-start seconds, per-paragraph latency, warm-idle RSS in MB

- [ ] **Step 7: Commit the findings doc**

```bash
cd "C:\Users\אורח\claude-dev\AI-Tools\MCP-Dev\hebrew-dictation"
git add docs/superpowers/plans/2026-07-31-hebrew-tts-narration-phase1-spike-findings.md
git commit -m "docs(spike): Hebrew TTS Phase 1 fact-finding + quality verdict"
```

**Gate: do not start Chunk 2 until this chunk's Step 5 verdict is GO.**

---

## Chunk 2: `narration.rs` — talking to an already-running sidecar

This chunk assumes a piper HTTP server is already running somewhere on localhost — it does not spawn one (that's Chunk 3). Everything here is either a pure function or an HTTP client tested against a lightweight mock server, so none of it depends on Chunk 1's spike having produced a real installed `piper-tts`.

### Task 1: Pure functions — server args builder and WAV sanity check

**Files:**
- Create: `src-tauri/src/narration.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod narration;`)
- Test: `src-tauri/src/narration.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Find where modules are declared**

Run: `grep -n "^mod " src-tauri/src/lib.rs`
Expected: a list like `mod audio;`, `mod batch;`, `mod enhance;`, etc. Note the exact location (they're alphabetically grouped in this codebase) so `mod narration;` goes in the right place.

- [ ] **Step 2: Write the failing tests**

Create `src-tauri/src/narration.rs`:

```rust
//! Talks to an already-running `piper1-gpl` HTTP sidecar over localhost.
//! Process lifecycle (spawning that sidecar) lives in `narration_process.rs`;
//! this module only knows how to build its launch args and call its API.

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
}
```

- [ ] **Step 3: Declare the module**

In `src-tauri/src/lib.rs`, add `mod narration;` alongside the other `mod` declarations (alphabetical order, per Step 1's finding).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml narration::tests`
Expected: `test result: ok. 5 passed`

(These pass immediately since implementation and tests are written together for functions this small — unlike later tasks, there's no meaningful "red" state to observe. If you want to verify the tests are real, temporarily change one assertion's expected value, confirm it fails, then revert.)

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/narration.rs src-tauri/src/lib.rs
git commit -m "feat(narration): pure server-args builder + WAV sanity check"
```

### Task 2: Async HTTP client — `synthesize` and `health_check`

**Files:**
- Modify: `src-tauri/src/narration.rs`
- Test: `src-tauri/src/narration.rs` (inline, extends the `tests` module)

Talks to the sidecar's two endpoints (`POST /synthesize`, `GET /info`, per spec §2.3) using `reqwest` (already a dependency — see `model.rs`). Tests run against a **real local HTTP server on an OS-assigned port**, using `tiny_http` (already a dependency, used for the app's own local API in `local_api.rs`) as a lightweight mock — not a new test-only dependency.

- [ ] **Step 1: Write the failing tests**

Add these new top-level items to `src-tauri/src/narration.rs` (above the `#[cfg(test)]` block):

```rust
use serde::Serialize;
use std::time::Duration;

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
        .send()
        .await
        .map_err(|e| NarrationError::Unreachable(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(NarrationError::BadResponse(format!("HTTP {}", resp.status())));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| NarrationError::BadResponse(e.to_string()))?
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
```

And add these tests inside the existing `mod tests` block:

```rust
    // Spin up a real tiny_http server on an OS-assigned port as a fake
    // sidecar, so these tests exercise actual HTTP behavior rather than
    // mocking reqwest itself — mirrors this crate's pattern of testing
    // HTTP-facing code against a real local server (see local_api.rs).

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

        let port = spawn_fake_sidecar(move |_req| (200, wav_for_server.clone()));

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
        let port = spawn_fake_sidecar(|_req| (200, b"{}".to_vec()));

        let client = reqwest::Client::new();
        assert!(health_check(&client, port).await);
    }

    #[tokio::test]
    async fn health_check_false_when_unreachable() {
        let client = reqwest::Client::new();
        assert!(!health_check(&client, 1).await);
    }
```

- [ ] **Step 2: Run tests to verify they fail first**

Run: `cargo test --manifest-path src-tauri/Cargo.toml narration::tests::synthesize_returns_wav_bytes_on_success`
Expected: FAIL — compile error (`synthesize` doesn't exist yet) if the tests are pasted before the implementation. If pasted together, instead temporarily comment out the `synthesize` function body, confirm the test fails, then restore it. Either way, confirm red before green.

- [ ] **Step 3: Run the full narration test suite**

Run: `cargo test --manifest-path src-tauri/Cargo.toml narration::tests`
Expected: `test result: ok. 11 passed` (5 from Task 1 + 6 from this task).

- [ ] **Step 4: Run clippy**

Run: `cargo clippy --manifest-path src-tauri/Cargo.toml`
Expected: no new warnings attributable to `narration.rs` (pre-existing warnings in other untouched files are out of scope per this repo's established scope rule).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/narration.rs
git commit -m "feat(narration): async HTTP client for the piper sidecar (synthesize, health_check)"
```

---
