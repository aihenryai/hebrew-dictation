//! Provisions the Hebrew narration engine: an isolated `uv`-managed Python
//! venv running `piper-tts`, plus the Hebrew voice model. Everything lives
//! under app-data — no system Python, no PATH/registry changes.
//! Windows-only (macOS out of scope for Phase 1).

use std::path::{Path, PathBuf};

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
}
