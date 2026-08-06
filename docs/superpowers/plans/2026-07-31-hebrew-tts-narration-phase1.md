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

## Chunk 3: Provisioning — `uv`, the venv, `piper-tts`, and the voice

⚠️ **Amended by Chunk 7 (read there before implementing):** `download_and_verify`'s and `ensure_uv_available`'s signatures below were retroactively made generic over `R: tauri::Runtime` (instead of the concrete `AppHandle`) so Chunk 7 could write a real integration test against `tauri::test::mock_app()`. The code shown in this chunk already reflects that — if you're implementing chunks in order this is transparent and needs no extra action, this note exists only so someone reading Chunk 3 in isolation isn't confused about why a plain `AppHandle` param looks generic.

**Platform scope:** this whole feature is `#[cfg(target_os = "windows")]`-gated, matching the existing precedent of `system_audio.rs`'s Windows-only `System`/`Call` recording modes (spec's macOS non-goal, added during planning). Every new file in this chunk assumes Windows.

**Verified facts used below** (checked live during planning, not guessed):
- `uv` 0.12.1 Windows x64 release asset: `uv-x86_64-pc-windows-msvc.zip`, exactly **19,073,343 bytes**, SHA-256 `8fcb0cb46e1229065e344758980924e569bef5882ef45f46fada8fb24e06b74a` (from `https://github.com/astral-sh/uv/releases/download/0.12.1/uv-x86_64-pc-windows-msvc.zip` + its `.sha256` sibling).
- `uv venv <path> --python 3.11` downloads and installs a fully self-contained Python 3.11 build if none is found — **no system Python required** (confirmed against astral.sh's own docs). This is what makes the spec's "nothing for the user to install" claim actually true.
- `UV_PYTHON_INSTALL_DIR`, `UV_PYTHON_INSTALL_BIN=0`, `UV_PYTHON_NO_REGISTRY=1`, and `UV_CACHE_DIR` (all real `uv` environment variables, confirmed against astral.sh's reference docs) are what keep this fully contained under app-data — no PATH modification, no Windows registry entries. This directly satisfies the spec's guiding principle ("nothing touches PATH or the registry").
- `uv pip install --python <venv-path> piper-tts==<pinned>` installs into a venv by path, no activation needed.

⚠️ **The exact `piper-tts` version to pin, and whether a `[http]` extra is needed, come from Chunk 1's spike findings doc — do not guess them here.** Task 3 below uses a placeholder `<PIPER_TTS_VERSION>` that must be replaced with the spike's actual finding before this task is implemented.

### Task 1: Extract a generic hash-verified download helper from `model.rs`

**Files:**
- Modify: `src-tauri/src/model.rs`
- Test: `src-tauri/src/model.rs` (new `#[cfg(test)] mod tests`, this file has none today)

`model.rs`'s existing `download_model` (lines 136-235) has download+hash-verify+progress-emit logic hardcoded to whisper models. This task extracts that logic into a reusable function so Task 2 (the `uv` binary) and Task 3 (the voice file) can reuse it instead of duplicating it — the spec (§2.4, §2.6) explicitly calls for this reuse, since an unverified download that's about to be *executed* (`uv.exe`) is a higher-severity risk than an inert model file.

- [ ] **Step 1: Write the failing tests for the extracted core function**

Add to the bottom of `src-tauri/src/model.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn spawn_fake_download_server(body: Vec<u8>) -> u16 {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let response = tiny_http::Response::from_data(body.clone());
                let _ = request.respond(response);
            }
        });
        port
    }

    #[tokio::test]
    async fn download_and_verify_core_writes_file_on_hash_match() {
        let body = b"pretend this is a downloaded file".to_vec();
        let expected_hash = format!("{:x}", Sha256::digest(&body));
        let port = spawn_fake_download_server(body.clone());

        let tmp_dir = std::env::temp_dir().join(format!("narration-test-{}", port));
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let dest = tmp_dir.join("downloaded.bin");

        let mut progress_calls: Vec<(u64, u64)> = Vec::new();
        let result = download_and_verify_core(
            &format!("http://127.0.0.1:{port}/"),
            &dest,
            body.len() as u64,
            &expected_hash,
            "קובץ בדיקה",
            |downloaded, total| progress_calls.push((downloaded, total)),
        )
        .await;

        assert!(result.is_ok(), "expected success, got {result:?}");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!progress_calls.is_empty(), "progress callback should fire at least once");

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[tokio::test]
    async fn download_and_verify_core_rejects_hash_mismatch_and_cleans_up() {
        let body = b"some bytes".to_vec();
        let port = spawn_fake_download_server(body.clone());

        let tmp_dir = std::env::temp_dir().join(format!("narration-test-mismatch-{}", port));
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let dest = tmp_dir.join("downloaded.bin");

        let result = download_and_verify_core(
            &format!("http://127.0.0.1:{port}/"),
            &dest,
            body.len() as u64,
            "0000000000000000000000000000000000000000000000000000000000000000", // deliberately wrong
            "קובץ בדיקה",
            |_, _| {},
        )
        .await;

        assert!(result.is_err());
        assert!(!dest.exists(), "final file must not exist after a hash mismatch");
        let tmp_path = dest.with_file_name("downloaded.bin.tmp");
        assert!(!tmp_path.exists(), "temp file must be cleaned up after a hash mismatch");

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[tokio::test]
    async fn download_and_verify_core_aborts_when_response_exceeds_expected_size() {
        // Server sends far more than expected_size — the 10%-tolerance guard must abort early.
        let body = vec![0u8; 1000];
        let port = spawn_fake_download_server(body);

        let tmp_dir = std::env::temp_dir().join(format!("narration-test-oversize-{}", port));
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let dest = tmp_dir.join("downloaded.bin");

        let result = download_and_verify_core(
            &format!("http://127.0.0.1:{port}/"),
            &dest,
            10, // expected only 10 bytes — server sends 1000
            "irrelevant",
            "קובץ בדיקה",
            |_, _| {},
        )
        .await;

        assert!(result.is_err());
        assert!(!dest.exists());

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path src-tauri/Cargo.toml model::tests`
Expected: FAIL — compile error, `download_and_verify_core` doesn't exist yet.

- [ ] **Step 3: Write the extracted functions**

Add near the top of `src-tauri/src/model.rs` (after the existing `use` lines), and add `use std::path::Path;` to the existing imports:

```rust
/// Core download+verify: no Tauri dependency, so it's directly unit-testable.
/// Downloads `url` to `dest_path` via an atomic temp-file-then-rename, verifying
/// the result matches `expected_size`/`expected_sha256` before the rename.
/// `component_label` is the Hebrew noun used in error messages (e.g. "המודל",
/// "מנוע ההקראה") so one function serves whisper models, the `uv` binary, and
/// Piper voice files with wording that fits each. `on_progress(downloaded, total)`
/// fires as bytes arrive — callers decide what to do with that (Tauri event, or
/// nothing, in tests).
async fn download_and_verify_core(
    url: &str,
    dest_path: &Path,
    expected_size: u64,
    expected_sha256: &str,
    component_label: &str,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<(), String> {
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!("יצירת התיקייה עבור {component_label} נכשלה. בדוק שיש מקום פנוי בדיסק והרשאות כתיבה. (פרטים טכניים: {e})")
        })?;
    }

    let client = reqwest::Client::new();
    let response = client.get(url).send().await.map_err(|e| {
        format!("הורדת {component_label} נכשלה — בדוק שיש חיבור לאינטרנט ונסה שוב. (פרטים טכניים: {e})")
    })?;

    let total_size = response.content_length().unwrap_or(expected_size);

    // Append ".tmp" to the whole filename rather than replacing the extension
    // (the original whisper-only code did `.with_extension("bin.tmp")`, which
    // silently assumed a `.bin` destination — this version works for `.bin`,
    // `.zip`, `.onnx`, or anything else).
    let mut tmp_name = dest_path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp_path = PathBuf::from(tmp_name);

    let mut file = tokio::fs::File::create(&tmp_path).await.map_err(|e| {
        format!("יצירת קובץ זמני עבור {component_label} נכשלה. בדוק שיש מקום פנוי. (פרטים טכניים: {e})")
    })?;

    let mut downloaded: u64 = 0;
    let mut hasher = Sha256::new();
    let mut stream = response.bytes_stream();
    let max_size = expected_size + (expected_size / 10); // 10% tolerance, same as the original

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            format!("ההורדה של {component_label} נקטעה — בדוק את החיבור לאינטרנט ונסה שוב. (פרטים טכניים: {e})")
        })?;

        downloaded += chunk.len() as u64;

        if downloaded > max_size {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(format!(
                "ההורדה של {component_label} חרגה מהגודל הצפוי — בוטלה לצורך אבטחה. נסה שוב."
            ));
        }

        hasher.update(&chunk);
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|e| format!("כתיבה לדיסק נכשלה. בדוק שיש מקום פנוי. (פרטים טכניים: {e})"))?;

        on_progress(downloaded, total_size);
    }

    let hash_result = format!("{:x}", hasher.finalize());
    if hash_result != expected_sha256 {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(format!(
            "{component_label} שהורד לא תואם לחתימה הצפויה (ייתכן שההורדה נפגמה). הקובץ נמחק. נסה להוריד שוב. (צפוי: {}…, התקבל: {}…)",
            &expected_sha256[..expected_sha256.len().min(16)],
            &hash_result[..16]
        ));
    }

    std::fs::rename(&tmp_path, dest_path).map_err(|e| {
        format!("שמירת {component_label} הסופית נכשלה. נסה למחוק ולהוריד מחדש. (פרטים טכניים: {e})")
    })?;

    Ok(())
}

/// Tauri-aware wrapper around `download_and_verify_core`: same behavior, but
/// emits `progress_event` (`{downloaded, total, progress}`) via `app.emit` as
/// bytes arrive, for the settings UI's progress bar.
///
/// Generic over `R: tauri::Runtime` (not the concrete `AppHandle` = `AppHandle<Wry>`)
/// **specifically so this is testable against `tauri::test::mock_app()`'s
/// `AppHandle<MockRuntime>`** — see Chunk 7, which needs this to write a real
/// integration test without a live window. Every existing call site
/// (`download_model`, and `narration_provision::ensure_uv_available` below)
/// keeps calling this with a plain `&AppHandle` unchanged — `R` is inferred as
/// `Wry` automatically there, so this is not a breaking change to any
/// `#[tauri::command]` entry point.
pub async fn download_and_verify<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    url: &str,
    dest_path: &Path,
    expected_size: u64,
    expected_sha256: &str,
    progress_event: &str,
    component_label: &str,
) -> Result<(), String> {
    let app = app.clone();
    let event = progress_event.to_string();
    download_and_verify_core(
        url,
        dest_path,
        expected_size,
        expected_sha256,
        component_label,
        move |downloaded, total| {
            let progress = (downloaded as f64 / total as f64 * 100.0) as u32;
            let _ = app.emit(
                &event,
                serde_json::json!({ "downloaded": downloaded, "total": total, "progress": progress }),
            );
        },
    )
    .await
}
```

- [ ] **Step 4: Rewrite `download_model` to call the new helper**

Replace the body of `download_model` (the existing manual download loop, roughly lines 161-222 of the current file) with a call to the new helper, keeping everything else (the "already downloaded" short-circuit, the OS notification on success) unchanged:

```rust
pub async fn download_model(app: AppHandle, model_name: String) -> Result<String, String> {
    validate_model_name(&model_name)?;

    let (_, url, expected_size, expected_hash) = MODELS
        .iter()
        .find(|(name, _, _, _)| *name == model_name)
        .ok_or_else(|| format!("Unknown model: {}", model_name))?;

    let model_path = get_model_path(&model_name);

    // Check if already downloaded and valid — no notification, the user already has it.
    if model_path.exists() {
        let metadata = std::fs::metadata(&model_path).map_err(|e| e.to_string())?;
        if metadata.len() == *expected_size {
            return Ok(model_path.to_string_lossy().to_string());
        }
    }

    let label_for_notification = friendly_model_label(&model_name);

    download_and_verify(
        &app,
        url,
        &model_path,
        *expected_size,
        expected_hash,
        "model-download-progress",
        "המודל",
    )
    .await?;

    // OS-level notification — most users start a download and switch to other work
    // while it runs in the background. Surfacing completion via the system tray
    // means they don't need to keep the settings panel open.
    let _ = app
        .notification()
        .builder()
        .title("הכתבה בעברית — מודל מוכן")
        .body(format!("המודל \"{}\" הורד בהצלחה והוא מוכן לשימוש.", label_for_notification))
        .show();

    Ok(model_path.to_string_lossy().to_string())
}
```

⚠️ **Behavior note for the reviewer/implementer:** error message wording changes slightly (e.g. "יצירת תיקיית המודלים נכשלה" becomes the generalized "יצירת התיקייה עבור המודל נכשלה") since the helper is now shared across whisper models, `uv`, and voice files. This is a deliberate, expected diff, not a regression — the *behavior* (create dir, download, verify size+hash, atomic rename, clean up temp files on any failure) is identical to before.

- [ ] **Step 5: Run the model tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml model::tests`
Expected: `test result: ok. 3 passed`

- [ ] **Step 6: Manually smoke-test that whisper downloads still work**

There is no pre-existing automated coverage for `download_model` to regress against (this file had zero tests before this task), so this is a manual check: run the app in dev mode (`npm run tauri dev`), open Settings, download the smallest whisper model (`tiny`), and confirm it completes and the model becomes usable — exactly as before this refactor.

- [ ] **Step 7: Run clippy and commit**

```bash
cargo clippy --manifest-path src-tauri/Cargo.toml
git add src-tauri/src/model.rs
git commit -m "refactor(model): extract download_and_verify_core for reuse by narration provisioning"
```

### Task 2: Download and extract `uv`

**Files:**
- Create: `src-tauri/src/narration_provision.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod narration_provision;`)
- Test: `src-tauri/src/narration_provision.rs` (inline)

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/narration_provision.rs`:

```rust
//! Provisions the Hebrew narration engine: an isolated `uv`-managed Python
//! venv running `piper-tts`, plus the Hebrew voice model. Everything lives
//! under app-data — no system Python, no PATH/registry changes (spec §2.4).
//! Windows-only (spec's macOS non-goal).

use std::path::{Path, PathBuf};
use tauri::AppHandle;

/// Pinned `uv` release facts (verified 2026-07-31 against
/// github.com/astral-sh/uv/releases — see Chunk 3 plan header for the exact
/// verification). Bump deliberately, not casually — a new `uv` version changes
/// the exact bytes this hash-checks.
const UV_VERSION: &str = "0.12.1";
const UV_DOWNLOAD_URL: &str =
    "https://github.com/astral-sh/uv/releases/download/0.12.1/uv-x86_64-pc-windows-msvc.zip";
const UV_ZIP_SIZE: u64 = 19_073_343;
const UV_ZIP_SHA256: &str = "8fcb0cb46e1229065e344758980924e569bef5882ef45f46fada8fb24e06b74a";

pub fn get_narration_dir() -> PathBuf {
    let app_data = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    app_data.join("hebrew-dictation").join("narration")
}

fn get_uv_zip_path() -> PathBuf {
    get_narration_dir().join(format!("uv-{UV_VERSION}.zip"))
}

pub fn get_uv_exe_path() -> PathBuf {
    get_narration_dir().join("uv").join("uv.exe")
}

pub fn is_uv_ready() -> bool {
    get_uv_exe_path().exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narration_dir_is_under_app_data_narration_subfolder() {
        let dir = get_narration_dir();
        assert!(dir.ends_with(Path::new("hebrew-dictation").join("narration")));
    }

    #[test]
    fn uv_exe_path_is_under_narration_dir_uv_subfolder() {
        let path = get_uv_exe_path();
        assert!(path.ends_with(Path::new("uv").join("uv.exe")));
        assert!(path.starts_with(get_narration_dir()));
    }

    #[test]
    fn is_uv_ready_false_when_not_downloaded() {
        // get_uv_exe_path() points at a real app-data location that won't
        // exist on a fresh test-running machine/CI runner.
        if !get_uv_exe_path().exists() {
            assert!(!is_uv_ready());
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml narration_provision::tests`
Expected: `test result: ok. 3 passed`

- [ ] **Step 3: Declare the module**

In `src-tauri/src/lib.rs`, add `#[cfg(target_os = "windows")] mod narration_provision;` alongside the other `mod` declarations — matching the exact precedent of `#[cfg(target_os = "windows")] mod system_audio;` already in this file, since this whole feature is Windows-only for Phase 1 (spec's macOS non-goal). Gating the module (not just individual items inside it) means a hypothetical non-Windows build never compiles-and-silently-ships PowerShell/`uv`-provisioning code it can't use.

- [ ] **Step 4: Write the failing test for download+extract**

Add to `narration_provision.rs`, above the `#[cfg(test)]` block:

```rust
/// Download `uv` (hash-verified via `model::download_and_verify`) and extract
/// `uv.exe` from the zip via Windows' built-in `Expand-Archive` — no new Rust
/// zip-handling dependency for a one-time provisioning step.
///
/// Generic over `R: tauri::Runtime` for the same reason as `download_and_verify`
/// above (testability against `tauri::test::mock_app()` — see Chunk 7). Its own
/// caller, `provision_narration_engine`, keeps taking a plain `&AppHandle`
/// unchanged; `R` is inferred as `Wry` there automatically.
pub async fn ensure_uv_available<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    let uv_exe = get_uv_exe_path();
    if uv_exe.exists() {
        return Ok(uv_exe);
    }

    let zip_path = get_uv_zip_path();
    crate::model::download_and_verify(
        app,
        UV_DOWNLOAD_URL,
        &zip_path,
        UV_ZIP_SIZE,
        UV_ZIP_SHA256,
        "narration-engine-download-progress",
        "מנוע ההקראה",
    )
    .await?;

    let extract_dir = get_narration_dir().join("uv");
    std::fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("יצירת תיקיית החילוץ נכשלה. (פרטים טכניים: {e})"))?;

    // Blocking Command::status() runs via spawn_blocking rather than directly
    // in this async fn, so it doesn't occupy a tokio worker thread for the
    // duration of Expand-Archive (a few seconds for a ~19MB zip).
    let zip_path_for_blocking = zip_path.clone();
    let extract_dir_for_blocking = extract_dir.clone();
    let status = tokio::task::spawn_blocking(move || {
        std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                    zip_path_for_blocking.display(),
                    extract_dir_for_blocking.display()
                ),
            ])
            .status()
    })
    .await
    .map_err(|e| format!("חילוץ מנוע ההקראה נכשל להתחיל (task panic). (פרטים טכניים: {e})"))?
    .map_err(|e| format!("חילוץ מנוע ההקראה נכשל להתחיל. (פרטים טכניים: {e})"))?;

    let _ = std::fs::remove_file(&zip_path);

    if !status.success() {
        return Err("חילוץ מנוע ההקראה נכשל.".to_string());
    }

    if !uv_exe.exists() {
        return Err(format!(
            "חילוץ מנוע ההקראה הושלם אך uv.exe לא נמצא בנתיב הצפוי ({}).",
            uv_exe.display()
        ));
    }

    Ok(uv_exe)
}
```

Add this test inside `mod tests`:

```rust
    #[tokio::test]
    #[ignore = "hits the real network (GitHub) and takes ~10s — run explicitly with `cargo test -- --ignored`, not part of the default suite"]
    async fn ensure_uv_available_downloads_and_extracts_a_working_exe() {
        // This is the one real integration point in this task: confirms the
        // pinned URL/size/hash in this file are still correct AND that
        // Expand-Archive actually produces a runnable uv.exe. Needs a real
        // AppHandle, which a plain #[test] can't construct — this is more
        // naturally exercised as part of the mock-Tauri-app integration test
        // in Chunk 7. Left here as a marker with #[ignore] so it's discoverable,
        // not deleted — implement its body in Chunk 7 alongside the other
        // #[ignore]-gated real-environment tests.
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml narration_provision::tests`
Expected: `test result: ok. 3 passed; 1 ignored`

- [ ] **Step 6: Manually verify the real download once**

This is the one place in Chunk 3 that's worth actually running by hand before moving on, since it's the load-bearing "does our pinned hash still match reality" check:

```powershell
# From a scratch Rust test harness, or temporarily call ensure_uv_available
# from a throwaway #[tokio::main] in src-tauri/src/main.rs and run it once.
```
Confirm: the zip downloads, the hash check passes (if it fails, the pinned `UV_ZIP_SHA256`/`UV_ZIP_SIZE` constants are wrong — re-verify against the GitHub release, don't just relax the check), extraction produces a real `uv.exe`, and running `uv.exe --version` from a terminal prints `uv 0.12.1`.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/narration_provision.rs src-tauri/src/lib.rs
git commit -m "feat(narration): download and extract uv (hash-verified)"
```

### Task 3: Provisioning state machine — venv, `piper-tts`, voice, atomic marker

**Files:**
- Modify: `src-tauri/src/narration_provision.rs`
- Modify: `src-tauri/Cargo.toml` (add the `process` feature to the `tokio` dependency)
- Test: `src-tauri/src/narration_provision.rs` (inline)

⚠️ **Dependency gap found during review:** `tokio`'s `process` feature is **not** currently enabled anywhere in this crate's dependency graph (verified via `cargo tree -e features -i tokio`: the resolved feature set is `default, fs, io-util, libc, macros, mio, net, rt, rt-multi-thread, socket2, sync, time, tokio-macros, windows-sys` — no `process`). Every existing use of `tokio` in this codebase is `tokio::fs`; `tokio::process::Command` (used below) would be the first use of that feature and **will not compile without it.**

- [ ] **Step 0: Enable the `tokio` `process` feature**

In `src-tauri/Cargo.toml`, change:
```toml
tokio = { version = "1", features = ["rt", "rt-multi-thread", "io-util", "macros"] }
```
to:
```toml
tokio = { version = "1", features = ["rt", "rt-multi-thread", "io-util", "macros", "process"] }
```

✅ **Spike completed 2026-08-06 — placeholders below now filled with real, verified values** (was previously a hard gate; `docs/superpowers/plans/2026-07-31-hebrew-tts-narration-phase1-spike-findings.md` has the full evidence). **Verdict: CONDITIONAL GO, approved by Henry** — natural/paragraph-length Hebrew narration (the actual target use case) tests consistently good-to-excellent via objective ASR round-trip; two accepted limitations ship documented, not fixed, in Phase 1: (1) short (~4-word) utterances show real run-to-run stochastic quality variance on a warm/reused voice instance — sometimes near-perfect, sometimes garbled, for identical input, not resolved by lowering `noise_scale`/`noise_w_scale`; (2) mixed Hebrew+English+digit text (e.g. brand names) breaks down badly — Hebrew-only phonemizer, expected. Neither blocks shipping; both should surface as known-limitation UI copy in Chunk 6 (e.g. "לתוצאה הכי טובה, כתבו משפטים מלאים בעברית בלבד").

- [ ] **Step 1: Write the state-machine tests**

Add to `narration_provision.rs`, above the `#[cfg(test)]` block:

```rust
use serde::{Deserialize, Serialize};

/// Written LAST, only after every provisioning step below succeeds. Its mere
/// existence (plus a version match) IS the "provisioned" signal — there is no
/// separate flag file or partial-state tracking. A provisioning run that dies
/// halfway leaves this marker absent, so the state machine naturally reports
/// "not provisioned" and a retry is just running provisioning again — every
/// step (uv download, venv creation, pip install, voice download) is already
/// idempotent/overwrite-safe on its own (spec §2.4).
#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct NarrationEngineMarker {
    marker_version: u32,
    piper_tts_version: String,
    voice_name: String,
}

const MARKER_VERSION: u32 = 1;
// Pinned from the spike (2026-08-06) — see spike-findings.md for how each was verified.
const PIPER_TTS_VERSION: &str = "1.6.0";
pub const VOICE_NAME: &str = "he_IL-saspeech-medium"; // pub: Chunk 4's narration_process.rs needs this too
const VOICE_URL: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/main/he/he_IL/saspeech/medium/he_IL-saspeech-medium.onnx";
const VOICE_SIZE: u64 = 63_221_984;
const VOICE_SHA256: &str = "3dc067debc9e782a8a0d095dbb58786648743d406366dcc2aa81009660873b4d";

pub fn get_venv_dir() -> PathBuf { // pub: Chunk 4's narration_process.rs needs the venv's python.exe path
    get_narration_dir().join("venv")
}

fn get_voice_path() -> PathBuf {
    get_narration_dir().join(format!("{VOICE_NAME}.onnx"))
}

fn get_marker_path() -> PathBuf {
    get_narration_dir().join("engine.json")
}

#[derive(Debug, PartialEq)]
pub enum NarrationEngineState {
    NotProvisioned,
    Ready,
}

/// Pure given the marker file's contents — reads it if present, returns
/// `NotProvisioned` on any absence/parse-failure/version-mismatch rather than
/// panicking or treating a corrupt marker as ready.
pub fn narration_engine_state() -> NarrationEngineState {
    let marker_path = get_marker_path();
    let Ok(contents) = std::fs::read_to_string(&marker_path) else {
        return NarrationEngineState::NotProvisioned;
    };
    match serde_json::from_str::<NarrationEngineMarker>(&contents) {
        Ok(marker) if marker.marker_version == MARKER_VERSION => NarrationEngineState::Ready,
        _ => NarrationEngineState::NotProvisioned,
    }
}
```

Add these tests inside `mod tests`:

```rust
    /// ⚠️ Safety-critical test helper — read this comment before touching it.
    /// `narration_engine_state()`/`get_marker_path()` read the REAL, non-sandboxed
    /// app-data path (via `dirs::data_dir()`), which isn't overridable per-call
    /// without threading a base-dir parameter through every function in this
    /// file (not done now — would touch every path-returning fn for a test-only
    /// concern). On a machine where narration has already been provisioned for
    /// real (including during later manual-verify steps in this same plan),
    /// a naive "delete marker, run test, delete marker again" helper would
    /// PERMANENTLY DESTROY the real completion marker, forcing an unwanted
    /// ~200-300MB re-provision on next app launch. This helper snapshots
    /// whatever was there first and restores it — including on panic, via a
    /// `Drop` guard — so it is safe to run against a machine with a real,
    /// already-provisioned engine.
    struct MarkerGuard {
        path: PathBuf,
        original: Option<String>,
    }

    impl MarkerGuard {
        fn new() -> Self {
            let path = get_marker_path();
            let original = std::fs::read_to_string(&path).ok();
            let _ = std::fs::remove_file(&path);
            Self { path, original }
        }
    }

    impl Drop for MarkerGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(contents) => {
                    let _ = std::fs::write(&self.path, contents);
                }
                None => {
                    let _ = std::fs::remove_file(&self.path);
                }
            }
        }
    }

    fn with_temp_app_data<T>(f: impl FnOnce() -> T) -> T {
        let _guard = MarkerGuard::new(); // restores the real marker on drop, even on panic
        f()
    }

    #[test]
    fn state_is_not_provisioned_when_marker_absent() {
        with_temp_app_data(|| {
            assert_eq!(narration_engine_state(), NarrationEngineState::NotProvisioned);
        });
    }

    #[test]
    fn state_is_ready_when_marker_present_and_version_matches() {
        with_temp_app_data(|| {
            std::fs::create_dir_all(get_narration_dir()).unwrap();
            let marker = NarrationEngineMarker {
                marker_version: MARKER_VERSION,
                piper_tts_version: "1.6.0".to_string(),
                voice_name: VOICE_NAME.to_string(),
            };
            std::fs::write(get_marker_path(), serde_json::to_string(&marker).unwrap()).unwrap();

            assert_eq!(narration_engine_state(), NarrationEngineState::Ready);
        });
    }

    #[test]
    fn state_is_not_provisioned_when_marker_version_is_stale() {
        with_temp_app_data(|| {
            std::fs::create_dir_all(get_narration_dir()).unwrap();
            std::fs::write(get_marker_path(), r#"{"marker_version":0,"piper_tts_version":"old","voice_name":"old"}"#).unwrap();

            assert_eq!(narration_engine_state(), NarrationEngineState::NotProvisioned);
        });
    }

    #[test]
    fn state_is_not_provisioned_when_marker_is_corrupt_json() {
        with_temp_app_data(|| {
            std::fs::create_dir_all(get_narration_dir()).unwrap();
            std::fs::write(get_marker_path(), "not valid json{{{").unwrap();

            assert_eq!(narration_engine_state(), NarrationEngineState::NotProvisioned);
        });
    }
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml narration_provision::tests -- --test-threads=1`
Expected: `test result: ok. 7 passed; 1 ignored` (4 new + 3 from Task 2, run single-threaded since these tests share one real file path — see the `with_temp_app_data` comment above).

⚠️ Note the `-- --test-threads=1` flag: without it, `state_is_ready_when_marker_present_and_version_matches` and the other marker tests can race on the same real `engine.json` path and flake. Chunk 4/5 do not need this flag (they don't share mutable file state across tests).

- [ ] **Step 3: Write the full provisioning function**

Add to `narration_provision.rs`:

```rust
/// Run the full provisioning flow if not already done: `uv` → venv →
/// `piper-tts` → voice → atomic marker. Safe to call every time the user
/// opens the narration screen — returns immediately if already `Ready`.
pub async fn provision_narration_engine(app: &AppHandle) -> Result<(), String> {
    if narration_engine_state() == NarrationEngineState::Ready {
        return Ok(());
    }

    // Guards against shipping with an un-filled-in placeholder from Task 3's
    // header warning — fails loudly and immediately instead of a confusing
    // HTTP/URL error partway through provisioning.
    if PIPER_TTS_VERSION.starts_with('<') || VOICE_URL.starts_with('<') || VOICE_SHA256.starts_with('<') {
        return Err(
            "מנוע ההקראה עדיין לא מוגדר בקוד (קבועים מסוג placeholder לא הוחלפו בערכי ה-spike האמיתיים)."
                .to_string(),
        );
    }

    let uv_exe = ensure_uv_available(app).await?;
    let narration_dir = get_narration_dir();
    let venv_dir = get_venv_dir();

    // Every uv invocation below is scoped to app-data via these env vars —
    // this is what keeps Python fully invisible to the rest of the system
    // (spec's guiding principle: no PATH changes, no registry entries).
    // Verified real UV_* variables (astral.sh reference docs, checked while
    // planning this chunk) — do not add/remove without re-checking there.
    let uv_env = [
        ("UV_PYTHON_INSTALL_DIR", narration_dir.join("python").to_string_lossy().to_string()),
        ("UV_PYTHON_INSTALL_BIN", "0".to_string()),
        ("UV_PYTHON_NO_REGISTRY", "1".to_string()),
        ("UV_CACHE_DIR", narration_dir.join("uv-cache").to_string_lossy().to_string()),
    ];

    let venv_status = tokio::process::Command::new(&uv_exe)
        .args(["venv", &venv_dir.to_string_lossy(), "--python", "3.11"])
        .envs(uv_env.iter().map(|(k, v)| (*k, v.as_str())))
        .status()
        .await
        .map_err(|e| format!("יצירת סביבת ההרצה למנוע ההקראה נכשלה להתחיל. (פרטים טכניים: {e})"))?;
    if !venv_status.success() {
        return Err("יצירת סביבת ההרצה למנוע ההקראה נכשלה.".to_string());
    }

    let pip_status = tokio::process::Command::new(&uv_exe)
        .args([
            "pip",
            "install",
            "--python",
            &venv_dir.to_string_lossy(),
            // [http] is required — confirmed by the spike: plain `piper-tts`
            // has no Flask dependency, so `python -m piper.http_server` fails
            // immediately with `ModuleNotFoundError: No module named 'flask'`.
            &format!("piper-tts[http]=={PIPER_TTS_VERSION}"),
        ])
        .envs(uv_env.iter().map(|(k, v)| (*k, v.as_str())))
        .status()
        .await
        .map_err(|e| format!("התקנת מנוע ההקראה נכשלה להתחיל. (פרטים טכניים: {e})"))?;
    if !pip_status.success() {
        return Err("התקנת מנוע ההקראה נכשלה.".to_string());
    }

    // Same "already downloaded and valid" short-circuit `download_model` has
    // for whisper models (spec §2.4: "resume/overwrite per the existing
    // model.rs semantics") — without it, a retry after a late failure (e.g.
    // the marker-write below failing right after a successful ~63MB voice
    // download) would re-download the voice for no reason.
    let voice_path = get_voice_path();
    let voice_already_valid = voice_path.exists()
        && std::fs::metadata(&voice_path).map(|m| m.len() == VOICE_SIZE).unwrap_or(false);
    if !voice_already_valid {
        crate::model::download_and_verify(
            app,
            VOICE_URL,
            &voice_path,
            VOICE_SIZE,
            VOICE_SHA256,
            "narration-voice-download-progress",
            "קול ההקראה",
        )
        .await?;
    }

    // Written LAST and only here — this is the atomic completion marker.
    let marker = NarrationEngineMarker {
        marker_version: MARKER_VERSION,
        piper_tts_version: PIPER_TTS_VERSION.to_string(),
        voice_name: VOICE_NAME.to_string(),
    };
    std::fs::write(get_marker_path(), serde_json::to_string(&marker).unwrap())
        .map_err(|e| format!("סימון סיום ההתקנה נכשל. (פרטים טכניים: {e})"))?;

    Ok(())
}
```

- [ ] **Step 4: Run clippy**

Run: `cargo clippy --manifest-path src-tauri/Cargo.toml`
Expected: no new warnings from `narration_provision.rs` or `model.rs`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/narration_provision.rs src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "feat(narration): provisioning state machine (uv, venv, piper-tts, voice, atomic marker)"
```

**Note for whoever implements Chunk 5 (Tauri commands):** `provision_narration_engine` has no progress-stage granularity beyond the three download-progress events it already emits (`narration-engine-download-progress` for `uv`, `narration-voice-download-progress` for the voice) — venv creation and `pip install` are opaque waits from the frontend's perspective. If that turns out to be a bad UX in practice (uv+Python+piper-tts install can take real wall-clock time), add stage-change events then; don't build it speculatively now (YAGNI).

---

## Chunk 4: Sidecar lifecycle — spawn, adopt, restart, teardown

**Files:**
- Create: `src-tauri/src/narration_process.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod narration_process;`)
- Modify: `src-tauri/Cargo.toml` (add `win32job` under the existing `[target.'cfg(windows)'.dependencies]` section)
- Test: `src-tauri/src/narration_process.rs` (inline)

**Design decision worth stating explicitly (not hand-waved):** the spec's §3 teardown design lists three layers — (a) `kill_on_drop` for clean exit, (b) a Windows Job Object for crash cases, (c) "a startup stale-sidecar sweep... probe the configured port and reclaim/kill any leftover sidecar." Layer (c) as literally worded ("kill") would require finding a process by the port it's bound to on Windows, which means either parsing `netstat`/`GetExtendedTcpTable` output or calling undocumented multi-step Win32 APIs — fragile, and disproportionate given layers (a) and (b) already cover the overwhelming majority of real cases (a true orphan only survives if the app died in the narrow window before the Job Object assignment completed). **This chunk folds layer (c) into `spawn_or_adopt`** instead of a separate kill-focused function: before spawning anything, it health-checks the configured port first. If something is already answering there and looks like a healthy piper sidecar, it's **reused as-is** (an `Unmanaged` variant — no wasted duplicate process, no confusing bind-conflict error) rather than killed and replaced. If nothing answers, a fresh `Owned` sidecar is spawned normally. This satisfies the spec's actual goal (no orphan pile-up, no wasted resources, no confusing errors) without the fragile kill-by-port-lookup implementation.

⚠️ **Self-corrected while planning Chunk 5 — `spawn_or_adopt` is called LAZILY ONLY, never from `setup()`.** An earlier draft of this note said it should also run "once at app startup," but that's wrong: `spawn_or_adopt` spawns a fresh sidecar whenever health-check finds nothing — calling it unconditionally at app launch would eagerly start the Python process on every app start even if the user never opens the narration screen that session, directly contradicting spec §3's idle-policy tradeoff ("the sidecar stays warm from screen-entry until app exit," i.e. lazy spawn, not eager). The fix costs nothing: an orphan from a previous session is still sitting there listening whenever the user *eventually* does open the narration screen — `spawn_or_adopt`'s health-check-first logic adopts it exactly the same way whether it's checked at app launch or at first real use. So Chunk 5 calls `spawn_or_adopt` **only** from the narration-screen entry point / `generate_narration`, never from `setup()`.

**Verified facts used below:**
- `win32job` crate 2.0.3 (MIT/Apache-2.0, targets `x86_64-pc-windows-msvc` — matches this feature's Windows-only scope): `Job::create_with_limit_info(&ExtendedLimitInfo) -> Result<Job, JobError>`, `Job::assign_process(&self, proc_handle: isize) -> Result<(), JobError>`, `ExtendedLimitInfo::new()`, `ExtendedLimitInfo::limit_kill_on_job_close(&mut self) -> &mut Self` — confirmed against the crate's docs.rs pages for `Job` and `ExtendedLimitInfo` while planning this chunk.
- `tokio::process::Child::raw_handle(&self) -> Option<RawHandle>` exists on Windows (confirmed against tokio's docs) — `RawHandle` casts to `isize` for `win32job`'s `assign_process`.
- `tokio`'s `net` feature (used for a TCP-level check, though ultimately not needed — see below) is already enabled per Chunk 3's `cargo tree` finding; no new tokio feature needed for this chunk beyond Chunk 3's `process` addition.

### Task 1: `NarrationServer` — spawn, adopt, restart, teardown

- [ ] **Step 1: Find where modules are declared and add the dependency**

Run: `grep -n "^mod " src-tauri/src/lib.rs` to find the insertion point for `mod narration_process;` (alphabetical, after `mod narration_provision;`).

In `src-tauri/Cargo.toml`, add `win32job` to the existing Windows-only section:
```toml
[target.'cfg(windows)'.dependencies]
wasapi = "0.23"
win32job = "2.0"
```

- [ ] **Step 2: Write the failing test**

Create `src-tauri/src/narration_process.rs`:

```rust
//! Sidecar process lifecycle for the narration engine: spawn, health-check,
//! Windows Job Object crash-protection, and guaranteed teardown. Talks to
//! `narration.rs` for the HTTP protocol itself (build_server_args, health_check,
//! synthesize) — this module only owns "is there a running process and is it
//! healthy," never the HTTP details. Windows-only (spec's macOS non-goal);
//! Chunk 5's Tauri commands provide the non-Windows stub.

use crate::narration::{build_server_args, health_check, synthesize, NarrationError};
use crate::narration_provision::{get_venv_dir, VOICE_NAME};
use std::time::Duration;

#[cfg(target_os = "windows")]
use win32job::{ExtendedLimitInfo, Job};

/// A running (or adopted) narration sidecar. `Owned` means this process spawned
/// it and holds the handles needed to kill it; `Unmanaged` means it was already
/// healthily running when we checked (our own earlier orphan, or a startup
/// race) — usable, but this instance has no way to kill it. See this file's
/// module-level design note in the plan for why "adopt, don't kill-by-port" is
/// the deliberate v1 choice.
pub enum NarrationServer {
    Owned {
        child: tokio::process::Child,
        #[cfg(target_os = "windows")]
        _job: Job,
        port: u16,
        client: reqwest::Client,
    },
    Unmanaged {
        port: u16,
        client: reqwest::Client,
    },
}

impl NarrationServer {
    fn port(&self) -> u16 {
        match self {
            NarrationServer::Owned { port, .. } => *port,
            NarrationServer::Unmanaged { port, .. } => *port,
        }
    }

    fn client(&self) -> &reqwest::Client {
        match self {
            NarrationServer::Owned { client, .. } => client,
            NarrationServer::Unmanaged { client, .. } => client,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_fake_healthy_sidecar() -> u16 {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let response = tiny_http::Response::from_data(b"{}".to_vec());
                let _ = request.respond(response);
            }
        });
        port
    }
}
```

- [ ] **Step 3: Run to confirm it compiles (no behavior yet)**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: builds clean (this step has no logic yet, just the enum/struct shape — confirming the types and imports are right before adding behavior).

- [ ] **Step 4: Write the failing test for `spawn_or_adopt`'s adoption path**

Add to the `tests` module:

```rust
    #[tokio::test]
    async fn spawn_or_adopt_returns_unmanaged_when_port_already_healthy() {
        let port = spawn_fake_healthy_sidecar();

        let result = NarrationServer::spawn_or_adopt(port).await;

        assert!(
            matches!(result, Ok(NarrationServer::Unmanaged { .. })),
            "expected Unmanaged (adopted) when something already answers /info, got {:?}",
            result.is_ok()
        );
    }
```

(This is the one meaningfully unit-testable path in this task — the `Owned` spawn path needs a real, provisioned `python.exe` + `piper-tts`, which doesn't exist on a bare checkout. That path is `#[ignore]`-gated and implemented in Chunk 7 alongside the other real-environment integration tests, per spec §7.)

- [ ] **Step 5: Run to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml narration_process::tests::spawn_or_adopt_returns_unmanaged_when_port_already_healthy`
Expected: FAIL — `spawn_or_adopt` doesn't exist yet.

- [ ] **Step 6: Implement `spawn_or_adopt`, `synthesize_with_restart`, `shutdown`**

Add to `narration_process.rs`, above the `#[cfg(test)]` block:

```rust
impl NarrationServer {
    /// Get a running sidecar on `port`: adopt an already-healthy one if
    /// present, otherwise spawn a fresh one and wait for it to become ready.
    /// This is both the normal "start the engine" path AND the spec's
    /// startup stale-sweep (see this chunk's module design note) — there is
    /// deliberately no separate sweep function.
    pub async fn spawn_or_adopt(port: u16) -> Result<Self, String> {
        let client = reqwest::Client::new();
        if health_check(&client, port).await {
            return Ok(NarrationServer::Unmanaged { port, client });
        }
        Self::spawn_owned(port, client).await
    }

    async fn spawn_owned(port: u16, client: reqwest::Client) -> Result<Self, String> {
        let venv_dir = get_venv_dir();
        let python_exe = venv_dir.join("Scripts").join("python.exe");
        if !python_exe.exists() {
            return Err(format!(
                "מנוע ההקראה לא מותקן כראוי — python.exe לא נמצא בנתיב הצפוי ({}).",
                python_exe.display()
            ));
        }

        let args = build_server_args(VOICE_NAME, "127.0.0.1", port);

        let mut child = tokio::process::Command::new(&python_exe)
            .args(&args)
            .kill_on_drop(true) // layer (a): clean-exit teardown
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("הפעלת מנוע ההקראה נכשלה. (פרטים טכניים: {e})"))?;

        // Layer (b): Windows Job Object with KILL_ON_JOB_CLOSE — the sidecar
        // dies with this app even on a hard crash/taskkill, not just clean exit.
        #[cfg(target_os = "windows")]
        let job = {
            let mut info = ExtendedLimitInfo::new();
            info.limit_kill_on_job_close();
            let job = Job::create_with_limit_info(&info)
                .map_err(|e| format!("יצירת הגנת תהליך למנוע ההקראה נכשלה. (פרטים טכניים: {e})"))?;
            let handle = child
                .raw_handle()
                .ok_or_else(|| "מנוע ההקראה יצא מיד לאחר ההפעלה.".to_string())?
                as isize;
            job.assign_process(handle)
                .map_err(|e| format!("שיוך מנוע ההקראה להגנת התהליך נכשל. (פרטים טכניים: {e})"))?;
            job
        };

        // Poll /info until healthy or a 30s ceiling — piper1-gpl's cold start
        // (loading the ONNX voice + Nakdimon) is real wall-clock time, measured
        // in Chunk 1's spike findings, not instant.
        let mut attempts = 0;
        loop {
            if health_check(&client, port).await {
                break;
            }
            attempts += 1;
            if attempts > 60 {
                let _ = child.kill().await;
                return Err("מנוע ההקראה לא הגיב בזמן סביר. נסה שוב.".to_string());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        Ok(NarrationServer::Owned {
            child,
            #[cfg(target_os = "windows")]
            _job: job,
            port,
            client,
        })
    }

    /// Generate audio for `text`. On failure, restart once (spec §5: "restart
    /// once; if still failing, clear error") — but **only when `self` is
    /// `Owned`**. This is a deliberate, reviewer-caught correction: an earlier
    /// draft of this function tried to respawn on `Unmanaged` failure too, but
    /// that's unsafe — we don't know whether the adopted process is actually
    /// dead or just returned one flaky response, `shutdown()` is a no-op for
    /// `Unmanaged` (nothing to kill), and `spawn_owned`'s readiness poll can't
    /// tell "my new process answered" from "the still-alive adopted process
    /// answered" on the same port. Concretely: if the adopted process is
    /// actually still alive, this would silently create a second process
    /// bound to the same port, orphan the tracked (new, likely dead-on-bind)
    /// one, and leave the real orphan permanently untracked — the exact
    /// failure mode `spawn_or_adopt`'s adopt-instead-of-kill design was
    /// trying to avoid. So an `Unmanaged` failure just surfaces the error;
    /// the caller (Chunk 5) can re-run `spawn_or_adopt` from scratch, which
    /// correctly re-probes health before deciding anything.
    pub async fn synthesize_with_restart(&mut self, text: &str) -> Result<Vec<u8>, NarrationError> {
        if let Ok(bytes) = synthesize(self.client(), self.port(), text).await {
            return Ok(bytes);
        }

        let port = match self {
            NarrationServer::Owned { port, .. } => *port,
            NarrationServer::Unmanaged { .. } => {
                return Err(NarrationError::Unreachable(
                    "מנוע ההקראה (שאומץ מריצה קודמת) לא הגיב. נסה שוב.".to_string(),
                ));
            }
        };

        self.shutdown().await;

        match Self::spawn_owned(port, reqwest::Client::new()).await {
            Ok(restarted) => {
                let result = synthesize(restarted.client(), restarted.port(), text).await;
                *self = restarted;
                result
            }
            Err(e) => Err(NarrationError::Unreachable(e)),
        }
    }

    /// Kill the process if we own it. A no-op for `Unmanaged` — there is
    /// nothing to kill (see this chunk's module design note).
    pub async fn shutdown(&mut self) {
        if let NarrationServer::Owned { child, .. } = self {
            let _ = child.kill().await;
        }
    }
}
```

- [ ] **Step 7: Run tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml narration_process::tests`
Expected: `test result: ok. 1 passed`

- [ ] **Step 8: Declare the module and run clippy**

In `src-tauri/src/lib.rs`, add `#[cfg(target_os = "windows")] mod narration_process;` — same reasoning and precedent as `narration_provision`'s gate (Chunk 3). ⚠️ **This means Chunk 5's `AppState` field holding `NarrationServer` must also be `#[cfg(target_os = "windows")]`-gated**, exactly like the existing `#[cfg(target_os = "windows")] system_recorder: Mutex<system_audio::SystemAudioRecorder>` field — and Chunk 5's Tauri commands need the same `#[cfg(not(target_os = "windows"))]` stub pattern already used elsewhere in `lib.rs` (the "Non-Windows stub so the command is always registrable in `generate_handler!`" comment near line 863) so the commands still compile and register on non-Windows, just returning a clear "not available on this platform" error instead of touching `NarrationServer` at all.

Run: `cargo clippy --manifest-path src-tauri/Cargo.toml`
Expected: no new warnings from `narration_process.rs`.

- [ ] **Step 9: Commit**

```bash
git add src-tauri/src/narration_process.rs src-tauri/src/lib.rs src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "feat(narration): sidecar lifecycle — spawn/adopt, restart-once, layered teardown"
```

---

## Chunk 5: Tauri commands, `AppState`, and settings wiring

This is where the previous chunks' pieces (HTTP client, provisioning, sidecar lifecycle) become something the frontend can actually call.

**Files:**
- Modify: `src-tauri/src/settings.rs` (new `narration_port` field, 6 touch points)
- Modify: `src-tauri/src/lib.rs` (`AppState` field, 3 new commands + non-Windows stubs, `invoke_handler!` registration)

### Task 1: `narration_port` setting

**Files:**
- Modify: `src-tauri/src/settings.rs`
- Test: `src-tauri/src/settings.rs` (extends the existing `#[cfg(test)] mod merge_tests`)

Mirrors `local_api_port` exactly (same shape: opt-in, no dedicated frontend UI yet, default via a named function, preserved across saves in `merge_frontend_update`).

- [ ] **Step 1: Write the failing test**

Find the existing `#[cfg(test)] mod merge_tests` block in `src-tauri/src/settings.rs` (starts ~line 481) and add:

```rust
    #[test]
    fn merge_frontend_update_preserves_narration_port_not_exposed_to_frontend_yet() {
        let current = AppSettings { narration_port: 9999, ..AppSettings::default() };
        let incoming = AppSettings::default(); // simulates a frontend payload that never carries this field
        let merged = current.merge_frontend_update(incoming);
        assert_eq!(merged.narration_port, 9999, "a settings-json-only field must survive a frontend save");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml settings::merge_tests::merge_frontend_update_preserves_narration_port_not_exposed_to_frontend_yet`
Expected: FAIL — compile error, `narration_port` isn't a field on `AppSettings` yet.

- [ ] **Step 3: Add the field at all 6 touch points**

In `src-tauri/src/settings.rs`:

1. `AppSettings` struct, right after `local_api_port` (~line 138):
```rust
    /// Port for the narration engine's local HTTP sidecar. Default 5758
    /// (sibling of the local API's 5757). No dedicated frontend UI in Phase 1.
    #[serde(default = "default_narration_port")]
    pub narration_port: u16,
```

2. `RedactedSettings` struct, right after `local_api_port` (~line 171):
```rust
    pub narration_port: u16,
```

3. `AppSettings::redacted()`, right after the `local_api_port` line (~line 204):
```rust
            narration_port: self.narration_port,
```

4. New default function, right after `default_local_api_port` (~line 247):
```rust
fn default_narration_port() -> u16 {
    5758
}
```

5. `impl Default for AppSettings`, right after `local_api_port` (~line 279):
```rust
            narration_port: default_narration_port(),
```

6. `merge_frontend_update`, right after the `local_api_port` line (~line 476) — same "not exposed to the frontend yet" preservation as its sibling:
```rust
        incoming.narration_port = self.narration_port;
```

- [ ] **Step 4: Run tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml settings::`
Expected: all pass, including the new test and every pre-existing `settings.rs` test (this field addition must not change behavior for anyone not touching it — `#[serde(default = ...)]` means old `settings.json` files without this key keep loading fine).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/settings.rs
git commit -m "feat(settings): add narration_port (default 5758), mirrors local_api_port"
```

### Task 2: `AppState` field, the 3 commands, and registration

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add the `AppState` field**

In the `AppState` struct (~line 32), right after the existing `#[cfg(target_os = "windows")] system_recorder` field, add:

```rust
    /// The narration engine's sidecar, once spawned/adopted. `tokio::sync::Mutex`
    /// (not `std::sync::Mutex`) because it's held across `.await` points inside
    /// `synthesize_with_restart` and `spawn_or_adopt` — same reasoning as the
    /// existing `streaming` field. `None` until first use (lazy spawn — see
    /// Chunk 4's self-corrected design note: never spawned from `setup()`).
    /// Windows-only (spec's macOS non-goal).
    #[cfg(target_os = "windows")]
    narration_server: tokio::sync::Mutex<Option<narration_process::NarrationServer>>,
```

Also add the corresponding initializer wherever `AppState { ... }` is constructed in `setup()` (search for the existing `streaming: tokio::sync::Mutex::new(None),` line and add immediately after it):

```rust
            #[cfg(target_os = "windows")]
            narration_server: tokio::sync::Mutex::new(None),
```

- [ ] **Step 2: Write `ensure_narration_ready`**

Add near the other model/settings-adjacent commands in `lib.rs`:

```rust
/// Cheap readiness check for the frontend: is the narration engine fully
/// provisioned (uv + venv + piper-tts + voice + marker)? Does NOT spawn or
/// touch the sidecar process — purely a provisioning-state read (spec §4
/// data flow step 1: "If the engine isn't provisioned → show the download flow").
#[cfg(target_os = "windows")]
#[tauri::command]
fn ensure_narration_ready() -> bool {
    narration_provision::narration_engine_state() == narration_provision::NarrationEngineState::Ready
}

/// Non-Windows stub so the command is always registrable in `generate_handler!`.
#[cfg(not(target_os = "windows"))]
#[tauri::command]
fn ensure_narration_ready() -> bool {
    false
}
```

⚠️ Note the non-Windows stub returns `false` (a plain "not ready," matching what a non-Windows user's UI should show — go to the same "unavailable" messaging as a not-yet-provisioned state) rather than an `Err`, since the frontend's job here is only "show the download flow or not," and "not available on this platform" is a variant of "not ready" the UI can word appropriately (Chunk 6 decides that copy).

- [ ] **Step 3: Write `narration_setup`**

```rust
/// Run the full provisioning flow (uv → venv → piper-tts → voice → marker),
/// emitting the download-progress events `narration_provision.rs` already
/// defines. Safe to call repeatedly — returns immediately if already `Ready`.
#[cfg(target_os = "windows")]
#[tauri::command]
async fn narration_setup(app: AppHandle) -> Result<(), String> {
    narration_provision::provision_narration_engine(&app).await
}

#[cfg(not(target_os = "windows"))]
#[tauri::command]
async fn narration_setup(_app: AppHandle) -> Result<(), String> {
    Err("הקראה זמינה רק ב-Windows כרגע".to_string())
}
```

- [ ] **Step 4: Write `generate_narration`**

```rust
/// Generate Hebrew narration audio for `text`, returning raw WAV bytes.
/// Lazily spawns/adopts the sidecar on first call in a session (spec §4 data
/// flow steps 2-3) — never from `setup()`. Guards: empty text (client should
/// already disable the button per spec §5, this is defense in depth) and
/// not-yet-provisioned (a clear pre-flight error, not a confusing spawn failure).
#[cfg(target_os = "windows")]
#[tauri::command]
async fn generate_narration(state: State<'_, AppState>, text: String) -> Result<Vec<u8>, String> {
    if text.trim().is_empty() {
        return Err("אין טקסט להקראה".to_string());
    }
    if narration_provision::narration_engine_state() != narration_provision::NarrationEngineState::Ready {
        return Err("מנוע ההקראה עדיין לא הותקן. לך להגדרות והתקן אותו קודם.".to_string());
    }

    let port = {
        let settings = state.settings.lock().map_err(|e| e.to_string())?;
        settings.narration_port
    };

    let mut guard = state.narration_server.lock().await;
    if guard.is_none() {
        let server = narration_process::NarrationServer::spawn_or_adopt(port).await?;
        *guard = Some(server);
    }
    // guard.is_none() was just checked/filled above, so this unwrap is safe.
    let server = guard.as_mut().unwrap();
    server.synthesize_with_restart(&text).await.map_err(|e| e.to_string())
}

#[cfg(not(target_os = "windows"))]
#[tauri::command]
async fn generate_narration(
    _state: State<'_, AppState>,
    _text: String,
) -> Result<Vec<u8>, String> {
    Err("הקראה זמינה רק ב-Windows כרגע".to_string())
}
```

⚠️ **`Vec<u8>` crosses Tauri IPC as a plain JSON number array by default (no `serde_bytes`), which has real overhead for a WAV-sized payload** — this is the exact tradeoff spec §5 already names explicitly ("known accepted tradeoff... acceptable for the paragraph scenario, revisit only if needed"). Not fixed here on purpose (YAGNI) — if Chunk 6's manual verify finds it sluggish for realistic paragraph lengths, that's the trigger to revisit (e.g. `serde_bytes`, or writing to a temp file and returning a path instead), not before.

- [ ] **Step 5: Register the 3 commands**

In the `.invoke_handler(tauri::generate_handler![...])` list (~line 2071), add after `cancel_batch_recording`:

```rust
            ensure_narration_ready,
            narration_setup,
            generate_narration,
```

- [ ] **Step 6: Build and run the full test suite**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: clean build.

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: every test from Chunks 2-5 plus every pre-existing test in this crate passes — this is the first point where all the new modules are wired together into the same binary as everything else, so it's the real "did any of this break existing behavior" checkpoint.

- [ ] **Step 7: Run clippy**

Run: `cargo clippy --manifest-path src-tauri/Cargo.toml`
Expected: no new warnings.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat(narration): AppState wiring + 3 Tauri commands (ensure_narration_ready, narration_setup, generate_narration)"
```

---

## Chunk 6: `save_narration_wav` command + the "הקראה" screen

**Files:**
- Modify: `src-tauri/src/lib.rs` (one more small command, `save_narration_wav`)
- Modify: `src/App.tsx` (new `"narration"` `AppView`, the screen itself, a home-screen entry point)

**Note on testing in this chunk:** this codebase's frontend has **zero automated test infrastructure** (confirmed: no `vitest`/`jest`, no `"test"` script in `package.json` — verification is `tsc && vite build` type-checking plus manual `npm run tauri dev` runs, per every existing release in `HANDOFF.md`). This chunk follows that same convention rather than introducing a new testing pattern unilaterally.

### Task 1: `save_narration_wav` command

Discovered while planning this chunk: the app's existing export pattern (`export_history`, `export_srt`) does the save-dialog + file-write entirely on the **Rust side** — the frontend never handles a save dialog itself, it just calls a command that returns the saved path. `generate_narration` (Chunk 5) returns raw `Vec<u8>` for **playback only**; saving needs its own command following the same established pattern, not a browser-side download hack.

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write the command**

Add near `export_history`/`export_srt` in `lib.rs`:

```rust
/// Save previously-generated narration WAV bytes to a user-chosen path.
/// Mirrors `export_history`/`export_srt`'s save-dialog pattern exactly — the
/// frontend already has the bytes (from `generate_narration`) and passes them
/// back here rather than the backend re-generating or caching them.
#[cfg(target_os = "windows")]
#[tauri::command]
async fn save_narration_wav(
    app: AppHandle,
    state: State<'_, AppState>,
    bytes: Vec<u8>,
    suggested_name: Option<String>,
) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;

    if bytes.is_empty() {
        return Err("אין הקלטה לשמירה — צור הקראה קודם.".to_string());
    }

    let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M");
    let default_name = match suggested_name.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(name) => format!("{}.wav", sanitize_filename(name)),
        None => format!("hebrew-dictation-narration_{}.wav", timestamp),
    };

    let restore_on_top = state.settings.lock().map(|s| s.always_on_top).unwrap_or(true);
    set_main_on_top(&app, false);

    let (tx, rx) = tokio::sync::oneshot::channel::<Option<std::path::PathBuf>>();
    app.dialog()
        .file()
        .set_title("שמור את ההקראה כ-WAV")
        .set_file_name(&default_name)
        .add_filter("קובץ WAV", &["wav"])
        .save_file(move |result| {
            let path = result.and_then(|fp| fp.into_path().ok());
            let _ = tx.send(path);
        });

    let path = rx.await.map_err(|_| "דיאלוג השמירה נסגר ללא תגובה".to_string());
    set_main_on_top(&app, restore_on_top);
    let path = path?;
    let path = match path {
        Some(p) => p,
        None => return Err("השמירה בוטלה".to_string()),
    };

    std::fs::write(&path, &bytes)
        .map_err(|e| format!("שמירת קובץ ה-WAV נכשלה. (פרטים טכניים: {e})"))?;

    Ok(path.to_string_lossy().to_string())
}

#[cfg(not(target_os = "windows"))]
#[tauri::command]
async fn save_narration_wav(
    _app: AppHandle,
    _state: State<'_, AppState>,
    _bytes: Vec<u8>,
    _suggested_name: Option<String>,
) -> Result<String, String> {
    Err("הקראה זמינה רק ב-Windows כרגע".to_string())
}
```

- [ ] **Step 2: Register the command**

Add `save_narration_wav,` to the `generate_handler!` list, right after `generate_narration,`.

- [ ] **Step 3: Build and run existing tests**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: clean build.

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: all pass (no new tests in this task — it's a thin, established-pattern wrapper; the existing `export_history`/`export_srt` have no dedicated unit tests either, for the same reason — the interesting logic is the save-dialog interaction, which isn't unit-testable without a real window).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat(narration): save_narration_wav command (mirrors export_history/export_srt)"
```

### Task 2: the "הקראה" screen

**Files:**
- Modify: `src/App.tsx`
- Modify: `src/App.css` (new rules for the screen; none of the classes this task uses exist yet — verified by grep while planning)

- [ ] **Step 1: Extend the `AppView` type and add state**

Change (line ~50):
```typescript
type AppView = "main" | "settings" | "onboarding" | "batch";
```
to:
```typescript
type AppView = "main" | "settings" | "onboarding" | "batch" | "narration";
```

Near the other `useState` declarations (alongside the batch-view state), add:
```typescript
  const [narrationText, setNarrationText] = useState("");
  const [narrationGenerating, setNarrationGenerating] = useState(false);
  const [narrationAudioUrl, setNarrationAudioUrl] = useState<string | null>(null);
  const [narrationBytes, setNarrationBytes] = useState<Uint8Array | null>(null);
  const [narrationReady, setNarrationReady] = useState(false);
  const [narrationProvisioning, setNarrationProvisioning] = useState(false);
  const [narrationProvisionProgress, setNarrationProvisionProgress] = useState(0);
```

- [ ] **Step 2: Add the progress-event listener**

In the existing `useEffect` that sets up `listen("model-download-progress", ...)` (~line 749), add two more listeners for the events `narration_provision.rs` (Chunk 3) already emits — `narration-engine-download-progress` (for the `uv` download) and `narration-voice-download-progress` (for the voice download). Both drive the same progress bar; the screen doesn't need to distinguish which sub-step is running, just show forward motion:

```typescript
    const unlistenNarrationEngineProgress = listen("narration-engine-download-progress", (event) => {
      const data = event.payload as { progress: number };
      setNarrationProvisionProgress(data.progress);
    });
    const unlistenNarrationVoiceProgress = listen("narration-voice-download-progress", (event) => {
      const data = event.payload as { progress: number };
      setNarrationProvisionProgress(data.progress);
    });
```

Add both to the cleanup return alongside the existing `unlistenProgress()` etc.

- [ ] **Step 3: Write the provisioning-check and generate/save handlers**

Add near the other `useCallback` handlers (alongside `exportHistory`):

```typescript
  const checkNarrationReady = useCallback(async () => {
    try {
      const ready = await invoke<boolean>("ensure_narration_ready");
      setNarrationReady(ready);
    } catch {
      setNarrationReady(false);
    }
  }, []);

  const runNarrationSetup = useCallback(async () => {
    setNarrationProvisioning(true);
    setNarrationProvisionProgress(0);
    setError("");
    try {
      await invoke("narration_setup");
      await checkNarrationReady();
    } catch (e) {
      setError(`התקנת מנוע ההקראה נכשלה: ${e}`);
    } finally {
      setNarrationProvisioning(false);
    }
  }, [checkNarrationReady]);

  const generateNarration = useCallback(async () => {
    if (!narrationText.trim()) return;
    setNarrationGenerating(true);
    setError("");
    try {
      const bytes = await invoke<number[]>("generate_narration", { text: narrationText });
      const arr = new Uint8Array(bytes);
      const blob = new Blob([arr], { type: "audio/wav" });
      const newUrl = URL.createObjectURL(blob);
      // Only release the PREVIOUS clip once the new one is confirmed created —
      // revoking eagerly at function entry (an earlier draft of this plan did
      // that) meant a failed second generate left `narrationAudioUrl` state
      // pointing at an already-revoked URL, silently breaking playback of the
      // still-valid first clip. Revoke-after-success avoids that entirely.
      if (narrationAudioUrl) {
        URL.revokeObjectURL(narrationAudioUrl);
      }
      setNarrationBytes(arr);
      setNarrationAudioUrl(newUrl);
    } catch (e) {
      setError(`יצירת ההקראה נכשלה: ${e}`);
    } finally {
      setNarrationGenerating(false);
    }
  }, [narrationText, narrationAudioUrl]);

  const saveNarrationWav = useCallback(async () => {
    if (!narrationBytes) return;
    try {
      const suggested = narrationText.trim().slice(0, 40) || undefined;
      await invoke<string>("save_narration_wav", {
        bytes: Array.from(narrationBytes),
        suggested_name: suggested,
      });
    } catch (e) {
      setError(`שמירת הקובץ נכשלה: ${e}`);
    }
  }, [narrationBytes, narrationText]);
```

⚠️ `Array.from(narrationBytes)` re-expands the `Uint8Array` back into a plain JS number array to cross Tauri IPC as `Vec<u8>` — the same "plain JSON array, not base64" overhead already flagged and deliberately accepted in Chunk 5 (spec §5's known tradeoff). Consistent, not a new cost.

✅ **Reuses the app's existing global `error`/`setError` state** (the same one rendered at `App.tsx:2418`/`2894` as `{error && <p className="error" onClick={() => setError("")}>❌ {error}</p>}`) rather than a separate `narrationError` state — matches this codebase's one established error-display convention instead of inventing a second, non-dismissible one.

- [ ] **Step 4: Call `checkNarrationReady` when the screen opens**

Add a `useEffect` that runs it when `view` becomes `"narration"`:

```typescript
  useEffect(() => {
    if (view === "narration") {
      checkNarrationReady();
    }
  }, [view, checkNarrationReady]);
```

- [ ] **Step 5: Add the home-screen entry point**

✅ **Insertion point corrected while planning** (an earlier draft of this step pointed at a "📁 + ⚙ button row" that doesn't actually exist — verified against the real layout). The real home screen has a `.main-header-modes` container (`App.tsx` ~line 2805) holding one full-width mode button (`btn-batch-nav btn-mode-combined`, "הקלט ותמלל / תמלול קבצי שמע"); `⚙` settings lives in a separate `.controls` row next to the record button. `.main-header-modes` is a flex row with `gap: 0.4rem`, and its child `.btn-batch-nav` has `flex: 1` — designed to hold more than one mode entry side by side. Add a second button there:

```typescript
        <div className="main-header-modes">
          <button
            className="btn-batch-nav btn-mode-combined"
            onClick={() => setView("batch")}
            aria-label="הקלט ותמלל או תמלול קבצי שמע"
          >הקלט ותמלל / תמלול קבצי שמע</button>
          <button
            className="btn-batch-nav btn-mode-narration"
            onClick={() => setView("narration")}
            aria-label="הקראה"
          >🔊 הקראה</button>
        </div>
```

(The existing `btn-mode-combined` button's JSX is shown above **for placement context only** — do not duplicate it; just add the new `btn-mode-narration` button as its sibling inside the existing `.main-header-modes` div.)

- [ ] **Step 6: Add the screen's render block**

Add a new `if (view === "narration") { ... }` block, reusing the batch view's real header structure/classes (`container batch-view`, `.batch-view-header`, `.btn-back`, `.batch-view-title`, `.btn-settings-labeled` — all confirmed to exist in `App.css` and to be layout-generic, not batch-specific, while planning this chunk):

```typescript
  if (view === "narration") {
    return (
      <main className="container batch-view" dir="rtl">
        <div className="batch-view-header">
          <button className="btn-back" onClick={() => setView("main")} aria-label="חזור">חזור</button>
          <h2 className="batch-view-title">הקראה</h2>
          <button
            className="btn-settings-labeled"
            style={{ marginInlineStart: "auto" }}
            onClick={() => { setSettingsReturn("narration"); setView("settings"); }}
            title="הגדרות"
            aria-label="הגדרות"
          >
            <span className="gear" aria-hidden="true">⚙</span> הגדרות
          </button>
        </div>

        {error && <p className="error" onClick={() => setError("")}>❌ {error}</p>}

        {!narrationReady && !narrationProvisioning && (
          <div className="narration-setup-prompt">
            <p>מנוע ההקראה עדיין לא הותקן. ההתקנה חד-פעמית, פועלת ברקע, ולא דורשת ידע טכני.</p>
            <button className="btn-primary" onClick={runNarrationSetup}>התקן מנוע הקראה</button>
          </div>
        )}

        {narrationProvisioning && (
          <div className="narration-setup-progress">
            <p>מתקין מנוע הקראה… ({narrationProvisionProgress}%)</p>
            <progress value={narrationProvisionProgress} max={100} />
          </div>
        )}

        {narrationReady && (
          <>
            <textarea
              className="narration-textarea"
              value={narrationText}
              onChange={(e) => setNarrationText(e.target.value)}
              placeholder="הדביקו או הקלידו טקסט בעברית להקראה…"
              dir="rtl"
              rows={8}
            />
            <button
              className="btn-primary"
              onClick={generateNarration}
              disabled={!narrationText.trim() || narrationGenerating}
            >
              {narrationGenerating ? "יוצר…" : "צור קול"}
            </button>

            {narrationAudioUrl && (
              <div className="narration-result">
                <audio controls src={narrationAudioUrl} />
                <button className="btn-secondary" onClick={saveNarrationWav}>שמור כ-WAV</button>
              </div>
            )}
          </>
        )}
      </main>
    );
  }
```

✅ Verified while planning: `settingsReturn` is declared `useState<AppView>("main")` (line 357) — the same `AppView` type Step 1 extends, so `setSettingsReturn("narration")` above is correct as written, no cast needed.

- [ ] **Step 7: Add the CSS rules**

None of `.narration-setup-prompt`, `.narration-setup-progress`, `.narration-textarea`, `.narration-result`, `.btn-mode-narration` exist yet (verified by grep while planning — Step 6/5's markup above is otherwise unstyled). Add to `src/App.css`, near the existing `.batch-view`/`.btn-mode-combined` rules:

```css
/* Home-screen mode entry, sibling of .btn-mode-combined — distinct accent so
   the two entries read as separate actions (same pattern as the teal/gold
   split already used for other mode pairs in this file). */
.btn-mode-narration {
  color: #f0abfc;
  border-color: #f0abfc55;
  white-space: normal;
  line-height: 1.25;
}
.btn-mode-narration:hover { background: #3b1332; border-color: #f0abfc; }

.narration-setup-prompt {
  text-align: center;
  padding: 2rem 1rem;
}
.narration-setup-prompt p {
  color: #a0a8c0;
  margin-bottom: 1rem;
}

.narration-setup-progress {
  text-align: center;
  padding: 1.5rem 1rem;
}
.narration-setup-progress progress {
  width: 100%;
  height: 8px;
}

.narration-textarea {
  width: 100%;
  min-height: 160px;
  background: #0f1729;
  border: 1px solid #2a3a5e;
  border-radius: 8px;
  color: #e5e9f5;
  font-family: inherit;
  font-size: 0.95rem;
  padding: 0.75rem;
  resize: vertical;
  margin-bottom: 0.75rem;
}

.narration-result {
  margin-top: 1rem;
  display: flex;
  flex-direction: column;
  gap: 0.6rem;
}
.narration-result audio {
  width: 100%;
}
```

Exact colors/spacing are a starting point, not pixel-locked — Step 9's manual verify is where real visual review happens; adjust there if something looks off against the rest of the app's dark theme, but don't skip adding *some* real rules (the point of this step is "not literally unstyled," not "final design").

- [ ] **Step 8: Type-check and build**

Run: `npx tsc --noEmit` (from the repo root, or via `npm run build`'s first step)
Expected: no type errors.

Run: `npm run build`
Expected: clean `vite build`.

- [ ] **Step 9: Manual verify in dev mode**

Run: `npm run tauri dev`

Happy path: home → 🔊 הקראה (new button, next to the existing mode button) → (if not provisioned) התקן מנוע הקראה → watch the progress bar move → once ready, type a Hebrew sentence → צור קול → confirm audio plays inline → שמור כ-WAV → confirm a real file lands on disk and plays in another player.

**Failure-path check (regression test for the revoke-order bug fixed in Task 2 Step 3):** generate once successfully and confirm the clip plays → without reloading, force a second `generate_narration` call to fail (e.g. temporarily stop the sidecar process externally, or set an invalid `narration_port` in settings.json to break the connection) → confirm the error banner shows AND the first clip is still present and still plays. If the first clip silently stops working after the second call fails, the bug is back.

- [ ] **Step 10: Commit**

```bash
git add src/App.tsx src/App.css
git commit -m "feat(narration): הקראה screen — provisioning flow, generate, play, save"
```

---

## Chunk 7: Real-environment integration test, manual verify, and handoff

Everything through Chunk 6 is provable with mocks or type-checking alone. This chunk is where the plan touches reality: a real `uv`+`piper-tts` install, a real Hebrew voice, and a real human.

**Files:**
- Modify: `src-tauri/src/narration_provision.rs` (implement the `#[ignore]`-gated `ensure_uv_available_downloads_and_extracts_a_working_exe` marker left in Chunk 3, Task 2)
- Modify: `src-tauri/src/narration_process.rs` (new `#[ignore]`-gated integration test covering the real `Owned` spawn path Chunk 4 deferred)
- Modify: `HANDOFF.md` (repo root — mark Phase 1 shipped, once it actually is — see Task 3's framing)

### Task 1: Implement the deferred `#[ignore]`-gated integration tests

**Files:**
- Modify: `src-tauri/src/narration_provision.rs`
- Modify: `src-tauri/src/narration_process.rs`
- Modify: `src-tauri/Cargo.toml` (add Tauri's `test` feature, dev-dependency only)

⚠️ **Prerequisite: Chunk 1's spike must have already produced a GO verdict, and Chunk 3 Task 3's placeholder constants (`PIPER_TTS_VERSION`, `VOICE_URL`, `VOICE_SIZE`, `VOICE_SHA256`) must already be filled in with real values** — these tests hit the real network and a real (provisioned, in this test run) engine; they cannot pass against placeholders.

⚠️ **Dependency gap found during review (same category as Chunk 3's `tokio` `process` feature):** `tauri::test::mock_app()` — needed by Step 1 below to get a usable `AppHandle` without a live window — is gated behind Tauri's `test` cargo feature, which isn't enabled anywhere in this crate. Without it, `tauri::test` doesn't resolve at all.

- [ ] **Step 0: Enable Tauri's `test` feature (dev-only)**

In `src-tauri/Cargo.toml`, add a **dev-dependency** entry for `tauri` alongside the existing regular dependency (Cargo unions the features for `cargo test` builds only — the real app binary, built via plain `cargo build`, never sees the `test` feature or pays for it):

```toml
[dev-dependencies]
tauri = { version = "2", features = ["test"] }
```

- [ ] **Step 1: Implement `ensure_uv_available_downloads_and_extracts_a_working_exe`**

In `src-tauri/src/narration_provision.rs`, replace the empty marker body from Chunk 3 Task 2 Step 4:

```rust
    #[tokio::test]
    #[ignore = "hits the real network (GitHub) and takes ~10s — run explicitly with `cargo test -- --ignored`, not part of the default suite"]
    async fn ensure_uv_available_downloads_and_extracts_a_working_exe() {
        // Real Tauri AppHandle needed for the download-progress `app.emit` calls
        // inside `download_and_verify` — `tauri::test::mock_app()` gives us one
        // without a real window, which is exactly what a plain #[test] couldn't do.
        let app = tauri::test::mock_app();
        let handle = app.handle();

        // Clean slate: if a previous run left uv.exe in place, this test would
        // trivially "pass" via the early-return in ensure_uv_available without
        // exercising the download+extract path at all.
        let uv_exe = get_uv_exe_path();
        let _ = std::fs::remove_file(&uv_exe);

        let result = ensure_uv_available(handle).await;
        assert!(result.is_ok(), "expected uv to download and extract, got {result:?}");

        let returned_path = result.unwrap();
        assert!(returned_path.exists());

        // The real proof: is this actually a working uv.exe, not just a file
        // that happens to exist at the right path?
        let version_output = std::process::Command::new(&returned_path)
            .arg("--version")
            .output()
            .expect("uv.exe should be directly executable");
        let version_text = String::from_utf8_lossy(&version_output.stdout);
        assert!(
            version_text.contains(UV_VERSION),
            "expected `uv --version` to report {UV_VERSION}, got: {version_text}"
        );
    }
```

- [ ] **Step 2: Run it**

Run: `cargo test --manifest-path src-tauri/Cargo.toml narration_provision::tests::ensure_uv_available_downloads_and_extracts_a_working_exe -- --ignored`
Expected: PASS (takes real wall-clock time — a real ~19MB download + extraction).

If this fails on the hash check specifically: the pinned `UV_ZIP_SIZE`/`UV_ZIP_SHA256` constants in `narration_provision.rs` no longer match what's actually at the pinned URL (astral-sh published a same-numbered re-release, which shouldn't happen for a tagged GitHub release, but verify at `https://github.com/astral-sh/uv/releases/tag/0.12.1` before assuming the test is wrong rather than the pin).

- [ ] **Step 3: Write the real sidecar integration test**

In `src-tauri/src/narration_process.rs`, first extend the existing top-of-file import (Chunk 4) — `looks_like_valid_wav` is used below but wasn't imported:

```rust
use crate::narration::{build_server_args, health_check, looks_like_valid_wav, synthesize, NarrationError};
```

Then add this test (this is new — there was no marker left for it, unlike Step 1):

```rust
    #[tokio::test]
    #[ignore = "needs a fully provisioned narration engine (real uv+venv+piper-tts+voice) — run explicitly after Chunk 3's provisioning has completed for real, e.g. via the app's own settings UI once built, or by calling provision_narration_engine directly in a throwaway harness"]
    async fn spawn_owned_produces_a_working_sidecar_and_shutdown_kills_it() {
        let port = 15758; // a port unlikely to collide with anything else in the test run

        let mut server = NarrationServer::spawn_owned(port, reqwest::Client::new())
            .await
            .expect("spawn_owned should succeed against a real provisioned engine");

        // Real proof #1: it actually generates audio, not just "the process started."
        let audio = synthesize(server.client(), server.port(), "בדיקה")
            .await
            .expect("a live, healthy sidecar should synthesize real audio");
        assert!(looks_like_valid_wav(&audio));

        // Real proof #2: shutdown() (layer (a), the clean-exit path) actually
        // kills the process — this is the orphan-prevention claim from spec
        // §3/§8, verified behaviorally rather than assumed from reading the
        // win32job/tokio API docs. Re-probing the port after shutdown should
        // find nothing there.
        server.shutdown().await;

        // Give the OS a moment to actually tear down the listening socket —
        // kill() returning doesn't guarantee the port is instantly free.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let client = reqwest::Client::new();
        assert!(
            !health_check(&client, port).await,
            "the sidecar should be unreachable after shutdown() — if this fails, \
             either kill_on_drop or the explicit kill() call isn't actually \
             terminating the process, which is the exact orphan bug this whole \
             chunk exists to catch"
        );
    }
```

- [ ] **Step 4: Run it**

Run: `cargo test --manifest-path src-tauri/Cargo.toml narration_process::tests::spawn_owned_produces_a_working_sidecar_and_shutdown_kills_it -- --ignored`
Expected: PASS.

- [ ] **Step 5: Run clippy and commit**

```bash
cargo clippy --manifest-path src-tauri/Cargo.toml
git add src-tauri/src/narration_provision.rs src-tauri/src/narration_process.rs src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "test(narration): implement deferred #[ignore]-gated real-environment integration tests"
```

### Task 2: Manual verify with Henry (cannot be automated)

This is the acceptance gate spec §8 describes ("Henry manually verifies real output on his machine before this ships") plus the crash-orphan check that genuinely can't be written as a clean automated test (it requires force-killing the app's own process from outside itself and observing system state afterward).

- [ ] **Step 1: Fresh-install walkthrough**

On a clean checkout (or after deleting the `narration` folder under app-data to simulate a fresh user): run the app, open הקראה, click "התקן מנוע הקראה," confirm the progress bar moves and eventually completes without the app hanging or crashing.

- [ ] **Step 2: Generate + quality spot-check**

Type a real paragraph of Hebrew text (not just Chunk 1's test sentences — something Henry would actually want narrated), generate, listen. This is the final human judgment call the spike's ASR round-trip (Chunk 1) was a proxy for — confirm it actually sounds acceptable for real use, not just "words are recognizable."

- [ ] **Step 3: Save + verify the file**

Click "שמור כ-WAV," confirm a real, playable WAV lands wherever the save dialog was pointed, and that it plays correctly in an external player (not just the app's own `<audio>` element).

- [ ] **Step 4: The crash-orphan check (layer (b), Job Object)**

With the sidecar running (after at least one generate), open Task Manager, confirm a `python.exe` process is present, then **force-kill the main "הכתבה בעברית" app process directly from Task Manager** (not a graceful close — this is specifically testing the crash case `kill_on_drop` does NOT cover). After a few seconds, confirm the `python.exe` process is **also gone** from Task Manager. If it's still there, the Job Object assignment isn't actually working as intended (Chunk 4's `assign_process` call, or the timing of when it runs relative to when the sidecar becomes reachable) and needs debugging before this ships — this is the single most safety-critical behavioral claim in the whole feature (spec §3/§8's orphan-prevention guarantee) and the automated test in Task 1 only proves the *clean*-exit path, not this one.

- [ ] **Step 5: Report back**

Report pass/fail on all four steps. Any failure here blocks calling Phase 1 done, regardless of how clean the automated test suite is — per this plan's own repeated framing (spec §6.4: "An ASR round-trip proves intelligibility, not pleasantness").

### Task 3: Update `HANDOFF.md` (only once Tasks 1-2 actually pass)

**Files:**
- Modify: `HANDOFF.md` (the top-level one, `docs/superpowers/HANDOFF.md` if it lives there, or wherever this repo's existing HANDOFF.md is — same file referenced throughout the spec's backlog item #4)

- [ ] **Step 1: Add a shipped entry**

Following this repo's own established HANDOFF convention (see the existing "✅ v2.12.1 — FULLY RELEASED" and "✅ RESOLVED" entries for the exact tone/format), add a new dated section: what shipped (local Hebrew narration via `he_IL-saspeech-medium`, one-time in-app provisioning via `uv`, no cloud, no voice cloning), what's explicitly still Phase 2 (VoxCPM2 cloning, still blocked on Henry's RunPod setup — unchanged from before this plan started), and a pointer back to this plan file + the spec for implementation detail.

- [ ] **Step 2: Commit**

```bash
git add HANDOFF.md
git commit -m "docs(handoff): Hebrew narration Phase 1 shipped — local generic-voice TTS via piper1-gpl"
```

---

## Execution handoff

Plan complete: 7 chunks, all reviewed and approved (Chunk 1 spike-gated; Chunks 2-6 each caught and fixed at least one real issue during review — a missing Cargo feature, a data-loss-risking test helper, a same-port double-spawn/orphan bug, incorrect non-Windows cfg gating, a wrong UI insertion point, missing CSS, and a blob-URL revoke-ordering bug). Saved to `docs/superpowers/plans/2026-07-31-hebrew-tts-narration-phase1.md`.

**Per the spec's own §6 rollout order, Chunk 1 is a hard gate: do not start Chunk 2 until Chunk 1's Step 5 verdict is GO.**

Ready to execute? This harness has subagent support, so per the writing-plans skill, execution should use **superpowers:subagent-driven-development** (fresh subagent per task, two-stage review) rather than running everything in this session.
