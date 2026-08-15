//! Provisions the Hebrew narration engine: an isolated `uv`-managed Python
//! venv, the Phonikud diacritizer, and the Hebrew voice model. Everything lives
//! under app-data — no system Python, no PATH/registry changes.
//! Windows-only (macOS out of scope for Phase 1).

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

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
        "מנוע הקריינות",
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
    .map_err(|e| format!("חילוץ מנוע הקריינות נכשל להתחיל (task panic). (פרטים טכניים: {e})"))?
    .map_err(|e| format!("חילוץ מנוע הקריינות נכשל להתחיל. (פרטים טכניים: {e})"))?;

    let _ = std::fs::remove_file(&zip_path);

    if !status.success() {
        return Err(format!(
            "חילוץ מנוע הקריינות נכשל (קוד יציאה: {}). נסה להוריד מחדש.",
            status.code().map_or_else(|| "לא ידוע".to_string(), |c| c.to_string())
        ));
    }

    if !uv_exe.exists() {
        return Err(format!(
            "חילוץ מנוע הקריינות הושלם אך uv.exe לא נמצא בנתיב הצפוי ({}).",
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
/// step (uv download, venv creation, pip install, artifact downloads) is
/// already idempotent/overwrite-safe on its own.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct NarrationEngineMarker {
    marker_version: u32,
    engine: String,
    voice_name: String,
}

/// Bumped to 2 when the engine moved from piper's own phonemizer to
/// Phonikud. An existing v1 marker therefore reads as "not provisioned" and
/// the new artifacts are fetched, rather than leaving a working-looking
/// install pointed at a voice the new server can't drive.
const MARKER_VERSION: u32 = 2;

/// Python packages the sidecar imports. `piper-tts` is deliberately gone: we
/// run our own server and inference now, so its dependency tree (including
/// espeak-ng) is no longer installed.
const PIP_PACKAGES: [&str; 5] = [
    "onnxruntime==1.28.0",
    "numpy",
    "phonikud",
    "phonikud-onnx",
    "tokenizers",
];

/// Voice: a VITS model trained on Phonikud's stress-marked IPA. "michael" was
/// chosen over the "shaul" checkpoint by listening to both against the
/// previously shipped voice on the same paragraph.
pub const VOICE_NAME: &str = "michael"; // pub: the sidecar-lifecycle module needs this too
const VOICE_URL: &str =
    "https://huggingface.co/Phonikud/phonikud-tts-checkpoints/resolve/main/michael.onnx";
const VOICE_SIZE: u64 = 63_516_050;
const VOICE_SHA256: &str = "d2824d46ecd7ca8a206686d818d3178effe4661ce78bdb4e754eed32fe604320";

/// Shared config for the Phonikud voices: phoneme→id map, sample rate,
/// inference defaults. Without it the server cannot map IPA to symbol ids.
const VOICE_CONFIG_URL: &str =
    "https://huggingface.co/Phonikud/phonikud-tts-checkpoints/resolve/main/model.config.json";
const VOICE_CONFIG_SIZE: u64 = 7_073;
const VOICE_CONFIG_SHA256: &str =
    "7f790dce7e26969a535ecda2715cc7da9c5261269cbf11c213d107b611458679";

/// The diacritizer that adds nikud AND stress. Stress is the part plain nikud
/// never encoded, and its absence is what made the old output sound flat.
const PHONIKUD_URL: &str =
    "https://huggingface.co/thewh1teagle/phonikud-onnx/resolve/main/phonikud-1.0.int8.onnx";
const PHONIKUD_SIZE: u64 = 307_844_158;
const PHONIKUD_SHA256: &str =
    "113afb58d3140502aa1e7691cdc6b240b56cf97e5852fc870e1a7fb5a400dd62";

/// Tokenizer for the diacritizer. Downloaded here so the sidecar never calls
/// out to HuggingFace at synthesis time — `phonikud_onnx` would otherwise
/// fetch it on every construction, breaking offline use.
const TOKENIZER_URL: &str =
    "https://huggingface.co/dicta-il/dictabert-large-char-menaked/resolve/main/tokenizer.json";
const TOKENIZER_SIZE: u64 = 18_016;
const TOKENIZER_SHA256: &str =
    "8e62e3b46c924e14fc32c749ef8944c311411ce9c4dc01c5b606953a169140ba";

/// The sidecar itself, compiled into the binary and written out during
/// provisioning so the installed engine is self-contained.
const NARRATION_SERVER_PY: &str = include_str!("../resources/narration_server.py");

pub fn get_venv_dir() -> PathBuf { // pub: a later sidecar-lifecycle module needs the venv's python.exe path
    get_narration_dir().join("venv")
}

pub fn get_voice_path() -> PathBuf {
    get_narration_dir().join(format!("{VOICE_NAME}.onnx"))
}

pub fn get_voice_config_path() -> PathBuf {
    get_narration_dir().join("model.config.json")
}

pub fn get_phonikud_path() -> PathBuf {
    get_narration_dir().join("phonikud-1.0.int8.onnx")
}

pub fn get_tokenizer_path() -> PathBuf {
    get_narration_dir().join("tokenizer.json")
}

pub fn get_server_script_path() -> PathBuf {
    get_narration_dir().join("narration_server.py")
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

/// Write the bundled sidecar script to app-data, overwriting any older copy.
///
/// Called on BOTH provisioning paths — the full install and the
/// already-provisioned early return — because the script travels inside the
/// app binary while the downloaded artifacts do not. Skipping it when the
/// marker reads Ready would pin every user to whichever version of the
/// script happened to be current at their first install.
///
/// Skips the write when the content already matches, so the common case
/// costs one read instead of a rewrite on every screen open.
fn sync_server_script() -> Result<(), String> {
    let path = get_server_script_path();
    if std::fs::read_to_string(&path).is_ok_and(|existing| existing == NARRATION_SERVER_PY) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("יצירת תיקיית המנוע נכשלה. (פרטים טכניים: {e})"))?;
    }
    std::fs::write(&path, NARRATION_SERVER_PY)
        .map_err(|e| format!("כתיבת שרת הקריינות נכשלה. (פרטים טכניים: {e})"))
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

/// Run the full provisioning flow if not already done: `uv` → venv → pip →
/// voice + config + diacritizer + tokenizer → server script → atomic marker.
/// Safe to call every time the user opens the narration screen — returns
/// immediately if already `Ready`.
///
/// Generic over `R: tauri::Runtime` for the same reason as
/// `ensure_uv_available`: it makes the whole flow testable against
/// `tauri::test::mock_app()`. The real call site passes a plain `&AppHandle`
/// and infers `Wry`, so this is not a breaking change.
pub async fn provision_narration_engine<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<(), String> {
    if narration_engine_state() == NarrationEngineState::Ready {
        // Refresh the sidecar script even on the already-provisioned path.
        // It ships inside the binary, so an app update can carry a fixed or
        // extended server while the downloaded artifacts are untouched and
        // the marker still reads Ready. Returning early without this wrote
        // the script exactly once, at first install, and every later change
        // to it silently never reached disk.
        sync_server_script()?;
        return Ok(());
    }

    // Guards against shipping with an un-filled-in placeholder — fails loudly
    // and immediately instead of a confusing HTTP/URL error partway through
    // provisioning. (All constants above are real, pinned values — this is
    // defense in depth, not expected to ever fire.)
    if VOICE_URL.starts_with('<') || VOICE_SHA256.starts_with('<') || PHONIKUD_SHA256.starts_with('<') {
        return Err(
            "מנוע הקריינות עדיין לא מוגדר בקוד (קבועים מסוג placeholder לא הוחלפו בערכי ה-spike האמיתיים)."
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
        // `--managed-python` is what actually guarantees the app's "no Python
        // required" promise: without it, uv is free to satisfy `--python 3.11`
        // from a system install if the user happens to have that exact version,
        // silently making the engine depend on their Python instead of ours.
        // With it, uv only ever uses the interpreter it downloaded into
        // UV_PYTHON_INSTALL_DIR under our own app-data folder.
        .args(["venv", &venv_dir.to_string_lossy(), "--python", "3.11", "--managed-python"])
        .envs(uv_env.iter().map(|(k, v)| (*k, v.as_str())));
    let venv_output = match tokio::time::timeout(Duration::from_secs(300), venv_cmd.output()).await {
        Err(_) => {
            return Err(
                "יצירת סביבת ההרצה למנוע הקריינות ארכה יותר מדי זמן, בדוק את החיבור לאינטרנט ונסה שוב."
                    .to_string(),
            )
        }
        Ok(Err(e)) => {
            return Err(format!("יצירת סביבת ההרצה למנוע הקריינות נכשלה להתחיל. (פרטים טכניים: {e})"))
        }
        Ok(Ok(output)) => output,
    };
    if !venv_output.status.success() {
        return Err(format!(
            "יצירת סביבת ההרצה למנוע הקריינות נכשלה (קוד יציאה: {}).\n{}",
            venv_output.status.code().map_or_else(|| "לא ידוע".to_string(), |c| c.to_string()),
            tail_stderr(&venv_output.stderr)
        ));
    }

    // onnxruntime is the heavy one here (~200MB of wheels with its deps) —
    // 600s gives a slow connection room without hanging the UI forever.
    let mut pip_cmd = tokio::process::Command::new(&uv_exe);
    pip_cmd
        .args(["pip", "install", "--python", &venv_dir.to_string_lossy()])
        .args(PIP_PACKAGES)
        .envs(uv_env.iter().map(|(k, v)| (*k, v.as_str())));
    let pip_output = match tokio::time::timeout(Duration::from_secs(600), pip_cmd.output()).await {
        Err(_) => {
            return Err(
                "התקנת מנוע הקריינות ארכה יותר מדי זמן, בדוק את החיבור לאינטרנט ונסה שוב.".to_string(),
            )
        }
        Ok(Err(e)) => return Err(format!("התקנת מנוע הקריינות נכשלה להתחיל. (פרטים טכניים: {e})")),
        Ok(Ok(output)) => output,
    };
    if !pip_output.status.success() {
        return Err(format!(
            "התקנת מנוע הקריינות נכשלה (קוד יציאה: {}).\n{}",
            pip_output.status.code().map_or_else(|| "לא ידוע".to_string(), |c| c.to_string()),
            tail_stderr(&pip_output.stderr)
        ));
    }

    // Same "already downloaded and valid" short-circuit download_model has
    // for whisper models — without it, a retry after a late failure (e.g. the
    // marker-write below failing right after a successful ~63MB voice
    // download) would re-download the voice for no reason.
    // Config first — it's a few KB, so any progress-bar flash from it is
    // over before the much longer model download even shows meaningful
    // progress.
    let voice_config_path = get_voice_config_path();
    let voice_config_already_valid = voice_config_path.exists()
        && std::fs::metadata(&voice_config_path).map(|m| m.len() == VOICE_CONFIG_SIZE).unwrap_or(false);
    if !voice_config_already_valid {
        crate::model::download_and_verify(
            app,
            VOICE_CONFIG_URL,
            &voice_config_path,
            VOICE_CONFIG_SIZE,
            VOICE_CONFIG_SHA256,
            "narration-voice-download-progress",
            "הגדרות קול הקריינות",
        )
        .await?;
    }

    let tokenizer_path = get_tokenizer_path();
    let tokenizer_already_valid = tokenizer_path.exists()
        && std::fs::metadata(&tokenizer_path).map(|m| m.len() == TOKENIZER_SIZE).unwrap_or(false);
    if !tokenizer_already_valid {
        crate::model::download_and_verify(
            app,
            TOKENIZER_URL,
            &tokenizer_path,
            TOKENIZER_SIZE,
            TOKENIZER_SHA256,
            "narration-voice-download-progress",
            "מנתח הטקסט",
        )
        .await?;
    }

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
            "קול הקריינות",
        )
        .await?;
    }

    // Largest artifact (~308MB), so it goes last: an earlier failure costs
    // the user less bandwidth before it surfaces.
    let phonikud_path = get_phonikud_path();
    let phonikud_already_valid = phonikud_path.exists()
        && std::fs::metadata(&phonikud_path).map(|m| m.len() == PHONIKUD_SIZE).unwrap_or(false);
    if !phonikud_already_valid {
        crate::model::download_and_verify(
            app,
            PHONIKUD_URL,
            &phonikud_path,
            PHONIKUD_SIZE,
            PHONIKUD_SHA256,
            "narration-voice-download-progress",
            "מנוע ההטעמה",
        )
        .await?;
    }

    sync_server_script()?;

    // Reclaim the ~63MB piper voice from the pre-Phonikud engine. Best-effort:
    // failing to delete a leftover must never fail provisioning.
    let _ = std::fs::remove_file(narration_dir.join("he_IL-saspeech-medium.onnx"));
    let _ = std::fs::remove_file(narration_dir.join("he_IL-saspeech-medium.onnx.json"));

    // Written LAST and only here — this is the atomic completion marker.
    let marker = NarrationEngineMarker {
        marker_version: MARKER_VERSION,
        engine: "phonikud".to_string(),
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
    use std::path::Path;

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

    #[test]
    fn sync_server_script_overwrites_a_stale_copy() {
        // Regression: the script used to be written only on the full
        // provisioning path, so once the marker read Ready every later change
        // to the sidecar silently never reached disk and the app kept running
        // whatever version shipped at first install.
        let _lock = MARKER_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let path = get_server_script_path();
        let original = std::fs::read_to_string(&path).ok();

        std::fs::create_dir_all(get_narration_dir()).unwrap();
        std::fs::write(&path, "# stale, from an older app version\n").unwrap();

        sync_server_script().expect("sync should rewrite the stale script");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            NARRATION_SERVER_PY,
            "a stale script must be replaced with the bundled one"
        );

        match original {
            Some(contents) => std::fs::write(&path, contents).unwrap(),
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    #[tokio::test]
    #[ignore = "provisions the ENTIRE engine for real: ~520MB of downloads plus a venv and pip install, several minutes. Run explicitly with `-- --ignored`. Leaves a working engine behind on purpose."]
    async fn provision_narration_engine_produces_a_usable_engine() {
        // The one test that exercises the real chain end to end: uv -> managed
        // Python -> venv -> pip -> the four artifacts -> marker. Everything
        // else about provisioning is either a pure-function unit test or a
        // manual click, and the pieces have failed independently before (a
        // missing voice config, a system-Python fallback), so the assembled
        // whole is worth one slow, explicit check.
        let app = tauri::test::mock_app();

        let result = provision_narration_engine(app.handle()).await;
        assert!(result.is_ok(), "provisioning failed: {result:?}");

        // Every artifact the sidecar's argv points at must actually exist —
        // the marker alone would happily be written over a missing file.
        for path in [
            get_voice_path(),
            get_voice_config_path(),
            get_phonikud_path(),
            get_tokenizer_path(),
            get_server_script_path(),
            get_venv_dir().join("Scripts").join("python.exe"),
        ] {
            assert!(path.exists(), "missing after provisioning: {}", path.display());
        }

        assert_eq!(narration_engine_state(), NarrationEngineState::Ready);

        // Idempotence: a second call must be a cheap no-op, not a re-download.
        let again = provision_narration_engine(app.handle()).await;
        assert!(again.is_ok(), "re-provisioning should no-op, got {again:?}");
    }

    #[tokio::test]
    #[ignore = "hits the real network (GitHub) and takes ~10s — run explicitly with `cargo test -- --ignored`, not part of the default suite"]
    async fn ensure_uv_available_downloads_and_extracts_a_working_exe() {
        // Real Tauri AppHandle needed for the download-progress `app.emit` calls
        // inside `download_and_verify` — `tauri::test::mock_app()` gives us one
        // without a real window.
        let app = tauri::test::mock_app();
        let handle = app.handle();

        // Clean slate: if a previous run left uv.exe in place, this test would
        // trivially "pass" via the early-return in ensure_uv_available without
        // exercising the download+extract path at all. Mirrors MarkerGuard
        // below — restores the real uv.exe on drop (even on panic/assertion
        // failure mid-test) so a real, already-provisioned machine never ends
        // up with the engine broken just because this test ran.
        struct UvExeGuard {
            path: PathBuf,
            original: Option<Vec<u8>>,
        }
        impl Drop for UvExeGuard {
            fn drop(&mut self) {
                match &self.original {
                    Some(bytes) => {
                        let _ = std::fs::write(&self.path, bytes);
                    }
                    None => {
                        let _ = std::fs::remove_file(&self.path);
                    }
                }
            }
        }
        let uv_exe = get_uv_exe_path();
        let _guard = UvExeGuard { path: uv_exe.clone(), original: std::fs::read(&uv_exe).ok() };
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
                engine: "phonikud".to_string(),
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
