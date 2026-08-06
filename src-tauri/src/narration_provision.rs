//! Provisions the Hebrew narration engine: an isolated `uv`-managed Python
//! venv running `piper-tts`, plus the Hebrew voice model. Everything lives
//! under app-data — no system Python, no PATH/registry changes.
//! Windows-only (macOS out of scope for Phase 1).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

/// Pinned `uv` release facts (verified against github.com/astral-sh/uv/releases).
/// Bump deliberately, not casually — a new `uv` version changes the exact
/// bytes this hash-checks.
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

/// Download `uv` (hash-verified via `model::download_and_verify`) and extract
/// `uv.exe` from the zip via Windows' built-in `Expand-Archive` — no new Rust
/// zip-handling dependency for a one-time provisioning step.
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
    // Paths are passed via environment variables rather than interpolated
    // into the -Command string: a single quote inside a Windows username
    // (e.g. "O'Brien") would otherwise terminate the PowerShell quoted
    // string early and break the command. Env vars sidestep quoting
    // entirely, regardless of what characters the path contains.
    let zip_path_for_blocking = zip_path.clone();
    let extract_dir_for_blocking = extract_dir.clone();
    let status = tokio::task::spawn_blocking(move || {
        std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Expand-Archive -LiteralPath $env:HD_UV_ZIP -DestinationPath $env:HD_UV_DIR -Force",
            ])
            .env("HD_UV_ZIP", &zip_path_for_blocking)
            .env("HD_UV_DIR", &extract_dir_for_blocking)
            .status()
    })
    .await
    .map_err(|e| format!("חילוץ מנוע ההקראה נכשל להתחיל (task panic). (פרטים טכניים: {e})"))?
    .map_err(|e| format!("חילוץ מנוע ההקראה נכשל להתחיל. (פרטים טכניים: {e})"))?;

    let _ = std::fs::remove_file(&zip_path);

    if !status.success() {
        return Err(format!(
            "חילוץ מנוע ההקראה נכשל (קוד יציאה: {}). נסה להוריד מחדש.",
            status.code().map_or_else(|| "לא ידוע".to_string(), |c| c.to_string())
        ));
    }

    if !uv_exe.exists() {
        return Err(format!(
            "חילוץ מנוע ההקראה הושלם אך uv.exe לא נמצא בנתיב הצפוי ({}).",
            uv_exe.display()
        ));
    }

    Ok(uv_exe)
}

/// Written LAST, only after every provisioning step below succeeds. Its mere
/// existence (plus a version match) IS the "provisioned" signal — there is no
/// separate flag file or partial-state tracking. A provisioning run that dies
/// halfway leaves this marker absent, so the state machine naturally reports
/// "not provisioned" and a retry is just running provisioning again — every
/// step (uv download, venv creation, pip install, voice download) is already
/// idempotent/overwrite-safe on its own.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct NarrationEngineMarker {
    marker_version: u32,
    piper_tts_version: String,
    voice_name: String,
}

const MARKER_VERSION: u32 = 1;
// Pinned from a spike that ran the real HTTP server end-to-end and verified
// these values against the actual downloaded artifacts.
const PIPER_TTS_VERSION: &str = "1.6.0";
pub const VOICE_NAME: &str = "he_IL-saspeech-medium"; // pub: a later sidecar-lifecycle module needs this too
const VOICE_URL: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/main/he/he_IL/saspeech/medium/he_IL-saspeech-medium.onnx";
const VOICE_SIZE: u64 = 63_221_984;
const VOICE_SHA256: &str = "3dc067debc9e782a8a0d095dbb58786648743d406366dcc2aa81009660873b4d";

pub fn get_venv_dir() -> PathBuf { // pub: a later sidecar-lifecycle module needs the venv's python.exe path
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

/// Last ~5 lines of a subprocess's stderr, for embedding in an error
/// message. Lossy-converts — `uv`/pip stderr is expected to be UTF-8, but
/// this must never panic on a stray non-UTF-8 byte.
fn tail_stderr(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(5);
    lines[start..].join("\n")
}

/// Run the full provisioning flow if not already done: `uv` → venv →
/// `piper-tts` → voice → atomic marker. Safe to call every time the user
/// opens the narration screen — returns immediately if already `Ready`.
pub async fn provision_narration_engine(app: &AppHandle) -> Result<(), String> {
    if narration_engine_state() == NarrationEngineState::Ready {
        return Ok(());
    }

    // Guards against shipping with an un-filled-in placeholder — fails loudly
    // and immediately instead of a confusing HTTP/URL error partway through
    // provisioning. (All constants above are real, pinned values — this is
    // defense in depth, not expected to ever fire.)
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
    // (no PATH changes, no registry entries). Real uv environment variables,
    // verified against astral.sh's reference docs.
    let uv_env = [
        ("UV_PYTHON_INSTALL_DIR", narration_dir.join("python").to_string_lossy().to_string()),
        ("UV_PYTHON_INSTALL_BIN", "0".to_string()),
        ("UV_PYTHON_NO_REGISTRY", "1".to_string()),
        ("UV_CACHE_DIR", narration_dir.join("uv-cache").to_string_lossy().to_string()),
    ];

    // `uv venv --python 3.11` may need to download an entire Python build
    // (~30-60MB from astral.sh) on first run — 300s covers a slow/stalled
    // connection without hanging the provisioning UI forever.
    let mut venv_cmd = tokio::process::Command::new(&uv_exe);
    venv_cmd
        .args(["venv", &venv_dir.to_string_lossy(), "--python", "3.11"])
        .envs(uv_env.iter().map(|(k, v)| (*k, v.as_str())));
    let venv_output = match tokio::time::timeout(Duration::from_secs(300), venv_cmd.output()).await {
        Err(_) => {
            return Err(
                "יצירת סביבת ההרצה למנוע ההקראה ארכה יותר מדי זמן, בדוק את החיבור לאינטרנט ונסה שוב."
                    .to_string(),
            )
        }
        Ok(Err(e)) => {
            return Err(format!("יצירת סביבת ההרצה למנוע ההקראה נכשלה להתחיל. (פרטים טכניים: {e})"))
        }
        Ok(Ok(output)) => output,
    };
    if !venv_output.status.success() {
        return Err(format!(
            "יצירת סביבת ההרצה למנוע ההקראה נכשלה (קוד יציאה: {}).\n{}",
            venv_output.status.code().map_or_else(|| "לא ידוע".to_string(), |c| c.to_string()),
            tail_stderr(&venv_output.stderr)
        ));
    }

    // `piper-tts[http]` pulls in a bigger dependency set (torch etc. via
    // transitive deps) than the venv step — 600s gives it more room.
    let mut pip_cmd = tokio::process::Command::new(&uv_exe);
    pip_cmd
        .args([
            "pip",
            "install",
            "--python",
            &venv_dir.to_string_lossy(),
            // [http] is required — plain piper-tts has no Flask dependency, so
            // `python -m piper.http_server` fails with
            // `ModuleNotFoundError: No module named 'flask'`.
            &format!("piper-tts[http]=={PIPER_TTS_VERSION}"),
        ])
        .envs(uv_env.iter().map(|(k, v)| (*k, v.as_str())));
    let pip_output = match tokio::time::timeout(Duration::from_secs(600), pip_cmd.output()).await {
        Err(_) => {
            return Err(
                "התקנת מנוע ההקראה ארכה יותר מדי זמן, בדוק את החיבור לאינטרנט ונסה שוב.".to_string(),
            )
        }
        Ok(Err(e)) => return Err(format!("התקנת מנוע ההקראה נכשלה להתחיל. (פרטים טכניים: {e})")),
        Ok(Ok(output)) => output,
    };
    if !pip_output.status.success() {
        return Err(format!(
            "התקנת מנוע ההקראה נכשלה (קוד יציאה: {}).\n{}",
            pip_output.status.code().map_or_else(|| "לא ידוע".to_string(), |c| c.to_string()),
            tail_stderr(&pip_output.stderr)
        ));
    }

    // Same "already downloaded and valid" short-circuit download_model has
    // for whisper models — without it, a retry after a late failure (e.g. the
    // marker-write below failing right after a successful ~63MB voice
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
    let marker_json = serde_json::to_string(&marker)
        .map_err(|e| format!("סידור נתוני הסימון נכשל. (פרטים טכניים: {e})"))?;
    std::fs::write(get_marker_path(), marker_json)
        .map_err(|e| format!("סימון סיום ההתקנה נכשל. (פרטים טכניים: {e})"))?;

    Ok(())
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

    #[test]
    fn uv_download_url_contains_uv_version() {
        // UV_DOWNLOAD_URL hardcodes the version again (a const fn can't
        // format!() into another const), so nothing stops the two from
        // drifting apart on a future version bump. If they ever mismatch,
        // ensure_uv_available downloads a real uv.zip whose hash won't
        // match UV_ZIP_SHA256 (pinned to the version in the URL) — a
        // confusing hash-mismatch failure instead of a clear "these don't
        // match" signal. This test makes the drift fail fast and legibly.
        assert!(UV_DOWNLOAD_URL.contains(UV_VERSION));
    }

    #[tokio::test]
    #[ignore = "hits the real network (GitHub) and takes ~10s — run explicitly with `cargo test -- --ignored`, not part of the default suite"]
    async fn ensure_uv_available_downloads_and_extracts_a_working_exe() {
        // This is the one real integration point in this task: confirms the
        // pinned URL/size/hash in this file are still correct AND that
        // Expand-Archive actually produces a runnable uv.exe. Needs a real
        // AppHandle, which a plain #[test] can't construct — this is more
        // naturally exercised as part of a later integration-test task with
        // tauri::test::mock_app(). Left here as a marker with #[ignore] so
        // it's discoverable, not deleted — implement its body in that later
        // task alongside the other #[ignore]-gated real-environment tests.
        // NOTE: this body is currently EMPTY — running it with `--ignored`
        // right now passes trivially and verifies nothing. An empty-body
        // pass here is not real verification; don't mistake it for one.
    }

    /// ⚠️ Safety-critical test helper — read this comment before touching it.
    /// `narration_engine_state()`/`get_marker_path()` read the REAL, non-sandboxed
    /// app-data path (via `dirs::data_dir()`), which isn't overridable per-call
    /// without threading a base-dir parameter through every function in this
    /// file (not done now — would touch every path-returning fn for a test-only
    /// concern). On a machine where narration has already been provisioned for
    /// real, a naive "delete marker, run test, delete marker again" helper would
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

    // Serializes the ENTIRE with_temp_app_data body (acquired before
    // MarkerGuard::new()) because all 4 marker tests below share the same
    // real file path (get_marker_path() isn't per-test-overridable — see
    // MarkerGuard's doc comment). Without this, `cargo test`'s default
    // parallel-within-binary execution lets one thread's MarkerGuard
    // snapshot+clear race another thread's in-progress write, corrupting or
    // destroying it. Makes `--test-threads=1` redundant for this specific
    // race, but that flag stays as the documented convention regardless.
    static MARKER_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_temp_app_data<T>(f: impl FnOnce() -> T) -> T {
        let _lock = MARKER_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
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
}
