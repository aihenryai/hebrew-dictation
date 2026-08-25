//! Provisions the Hebrew narration engine: an isolated `uv`-managed Python
//! venv, the Phonikud diacritizer, and the Hebrew voice model. Everything lives
//! under app-data — no system Python, no PATH/registry changes.
//! Windows-only (macOS out of scope for Phase 1).

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Emitter;

#[cfg(target_os = "windows")]
use crate::narration_process::CREATE_NO_WINDOW;
/// Needed for `.creation_flags()` on the blocking `std::process::Command`;
/// `tokio::process::Command` exposes its own inherent method on Windows.
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

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

/// Bytes the engine occupies on disk, 0 when nothing is installed.
///
/// Reads the directory rather than the marker deliberately: after a delete
/// that failed partway, the marker is gone but hundreds of megabytes remain.
/// The settings screen keys its "delete engine" affordance off THIS, so those
/// leftovers stay reachable instead of becoming invisible junk.
pub fn narration_dir_size() -> u64 {
    fn dir_size(path: &std::path::Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(path) else {
            return 0;
        };
        entries
            .filter_map(|e| e.ok())
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => dir_size(&e.path()),
                Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
                Err(_) => 0,
            })
            .sum()
    }
    dir_size(&get_narration_dir())
}

/// `remove_dir_all` with a short retry.
///
/// Windows releases a dead process's file handles asynchronously, so a delete
/// issued immediately after killing the holder can still hit a sharing
/// violation on a file whose owner is already gone. Retrying turns that race
/// into a non-event.
///
/// This is NOT a substitute for stopping the holders first — retrying against
/// a process that is still alive just fails five times instead of once.
pub async fn remove_dir_all_with_retry(dir: &std::path::Path) -> std::io::Result<()> {
    let mut last_error = None;
    for attempt in 0..5 {
        // The engine directory holds hundreds of package files (~700MB) —
        // walking and unlinking all of it is real wall-clock time on Windows
        // NTFS, not a cheap syscall. `spawn_blocking` moves that off the
        // async worker thread this future is polled on, so it doesn't stall
        // whatever else is scheduled on the same tokio runtime for the
        // duration (this command already holds the narration_server mutex,
        // so other narration commands are blocked either way, but the
        // runtime's OTHER worker threads — unrelated commands, timers —
        // should not pay for this one delete).
        let owned = dir.to_path_buf();
        let result = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&owned))
            .await
            .unwrap_or_else(|e| Err(std::io::Error::other(format!("spawn_blocking panicked: {e}"))));
        match result {
            Ok(()) => return Ok(()),
            // Already gone (or a previous attempt got there): success, not an
            // error — `remove_dir_all` reports NotFound for a missing path.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => last_error = Some(e),
        }
        if attempt < 4 {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    Err(last_error.expect("the loop only exits here after storing an error"))
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
        let mut cmd = std::process::Command::new("powershell");
        cmd.args([
            "-NoProfile",
            "-Command",
            "Expand-Archive -LiteralPath $env:HD_UV_ZIP -DestinationPath $env:HD_UV_DIR -Force",
        ])
        .env("HD_UV_ZIP", &zip_path_for_blocking)
        .env("HD_UV_DIR", &extract_dir_for_blocking);
        // Otherwise a PowerShell console window flashes over the installer UI.
        #[cfg(target_os = "windows")]
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.status()
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
///
/// All five pinned to exact versions — until this, 4 of 5 were left
/// unpinned while every OTHER artifact this module fetches (uv itself, the
/// voice, the diacritizer) is SHA-256-verified. An unpinned `pip install`
/// silently picks up whatever the next release of a small, low-profile
/// package happens to be — a real supply-chain gap for `phonikud`/
/// `phonikud-onnx` specifically, which are exactly the kind of niche package
/// a typosquat or a compromised maintainer account targets.
///
/// `numpy` is pinned to 2.4.6, NOT numpy's own latest (2.5.2 at the time of
/// writing) — 2.5.x ships wheels for cp312+ only, no cp311, and this venv is
/// created with `--python 3.11` (see `provision_venv`). Pinning to a version
/// with no matching wheel would force a from-source build with no C
/// toolchain guaranteed on the user's machine, i.e. would break provisioning
/// outright. Re-verify wheel availability for the target Python version
/// before ever bumping this, not just "is it the newest release".
const PIP_PACKAGES: [&str; 5] = [
    "onnxruntime==1.28.0",
    "numpy==2.4.6",
    "phonikud==0.4.1",
    "phonikud-onnx==1.0.6",
    "tokenizers==0.23.1",
];

/// A selectable narration voice. All are VITS models trained on Phonikud's
/// stress-marked IPA and share one `model.config.json`, so switching voices
/// means swapping one ~63MB file — no other artifact changes.
pub struct NarrationVoice {
    /// Stable id: the filename stem and the value stored in settings.
    pub id: &'static str,
    /// Hebrew label for the picker.
    pub label: &'static str,
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// The catalog. Upstream publishes four checkpoints; all four were generated
/// and judged by ear, and only this one was kept — "shaul" in particular was
/// rejected as clearly worse. Offering a 63MB download of a voice nobody
/// wants is worse than not offering it. The picker hides itself while this
/// holds a single entry, and adding a voice back is just another element.
/// All are male; no free Hebrew female voice exists (see the spike findings).
pub const VOICES: [NarrationVoice; 1] = [
    NarrationVoice {
        id: "michael",
        label: "מיכאל",
        url: "https://huggingface.co/Phonikud/phonikud-tts-checkpoints/resolve/main/michael.onnx",
        size: 63_516_050,
        sha256: "d2824d46ecd7ca8a206686d818d3178effe4661ce78bdb4e754eed32fe604320",
    },
];

/// Voice installed by the initial provisioning run, and the fallback whenever
/// a stored setting names a voice this build no longer ships.
pub const DEFAULT_VOICE: &str = "michael";

/// Look a voice up by id, falling back to the default rather than failing:
/// a settings file naming a removed voice must degrade to working audio, not
/// a hard error the user cannot clear from the UI.
pub fn voice_by_id(id: &str) -> &'static NarrationVoice {
    VOICES
        .iter()
        .find(|v| v.id == id)
        .unwrap_or_else(|| VOICES.iter().find(|v| v.id == DEFAULT_VOICE).unwrap())
}

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

pub fn get_voice_path(voice_id: &str) -> PathBuf {
    get_narration_dir().join(format!("{}.onnx", voice_by_id(voice_id).id))
}

/// True when this voice's model file is present at its expected size. Size
/// alone rather than a re-hash: this is called on every screen open, and
/// `download_and_verify` already hash-checks before the file is put in place.
pub fn is_voice_downloaded(voice_id: &str) -> bool {
    let v = voice_by_id(voice_id);
    let path = get_voice_path(v.id);
    std::fs::metadata(&path).map(|m| m.len() == v.size).unwrap_or(false)
}

/// Download one voice if it isn't already present. Used when the user picks a
/// voice that wasn't installed at provisioning time.
pub async fn ensure_voice_downloaded<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    voice_id: &str,
) -> Result<(), String> {
    let v = voice_by_id(voice_id);
    if is_voice_downloaded(v.id) {
        return Ok(());
    }
    crate::model::download_and_verify(
        app,
        v.url,
        &get_voice_path(v.id),
        v.size,
        v.sha256,
        "narration-voice-download-progress",
        "קול הקריינות",
    )
    .await
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

/// Where the CURRENT sidecar's nonce lives — see `narration_process::spawn_owned`
/// (writes it on every spawn) and `spawn_or_adopt` (reads it to decide whether a
/// process already listening on the port is verifiably ours before trusting it).
pub fn get_nonce_path() -> PathBuf {
    get_narration_dir().join("current_nonce")
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

/// Delete `<id>.onnx` files for voices this build no longer offers, so a
/// voice removed from the catalog stops costing the user 63MB forever.
/// Deliberately matches only the exact `.onnx` files we could have written —
/// never a wildcard sweep of the directory, which shares space with the venv,
/// the diacritizer and the config.
fn remove_uncatalogued_voices() {
    const KNOWN_RETIRED: [&str; 3] = ["shaul", "shaul_whisper_heb_ipa1", "model"];
    for id in KNOWN_RETIRED {
        if VOICES.iter().any(|v| v.id == id) {
            continue;
        }
        let _ = std::fs::remove_file(get_narration_dir().join(format!("{id}.onnx")));
    }
}

/// Total provisioning steps, for the "step N of M" readout.
const SETUP_STEPS: u8 = 7;

/// Announce the current provisioning step and reset the progress bar.
///
/// Without this the UI only ever saw download progress, so the two slowest
/// steps — creating the venv (which downloads a whole Python) and installing
/// the packages — emitted nothing at all and left the bar frozen at the 100%
/// of the *previous* download for a minute or more. That reads as a hang.
fn emit_stage<R: tauri::Runtime>(app: &tauri::AppHandle<R>, step: u8, label: &str) {
    let _ = app.emit(
        "narration-setup-stage",
        serde_json::json!({ "step": step, "total": SETUP_STEPS, "label": label }),
    );
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
/// Serializes the whole provisioning flow. Two concurrent calls (a double
/// click, two windows, a retry racing the original attempt) would otherwise
/// both run `uv venv --clear` against the same directory and both download
/// into the same fixed `<dest>.tmp` path — two writers on one file handle,
/// each hashing only the bytes it wrote, so the verified rename can succeed
/// with content that is actually an interleaving of both downloads. The
/// frontend already prevents this in the common case (the install button is
/// hidden while `narrationProvisioning` is true), so this is defense for the
/// cases it can't cover — a second app window, or a caller other than the UI.
fn provision_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Minimum free space required to START provisioning. The final install is
/// ~695MB (measured, see HANDOFF.md), but that number is the STEADY STATE —
/// mid-provisioning also briefly holds the `uv` zip, the managed Python
/// download, and pip's wheel cache alongside the unpacked venv, so this asks
/// for meaningfully more headroom than the final footprint alone.
const MIN_FREE_SPACE_BYTES: u64 = 1_200 * 1024 * 1024; // 1200MB

/// Fails fast, before any download starts, if the drive holding app-data
/// doesn't have room for the engine. Without this, a low-disk machine burns
/// however much of the ~600MB it can fetch before failing deep inside a
/// download or extraction step, with an error naming THAT step rather than
/// the actual cause — the same "close programs to free memory"-style
/// confusion the whisper model loader's error text was written to avoid.
fn check_disk_space() -> Result<(), String> {
    let dir = get_narration_dir();
    // The directory may not exist yet on a first-ever install — walk up to
    // the nearest existing ancestor so the disk lookup still finds a mount
    // point instead of silently checking nothing.
    let mut probe = dir.as_path();
    while !probe.exists() {
        match probe.parent() {
            Some(parent) => probe = parent,
            None => return Ok(()), // nothing on this system resolves — don't block on a check we can't perform
        }
    }

    let disks = sysinfo::Disks::new_with_refreshed_list();
    let available = disks
        .iter()
        .filter(|d| probe.starts_with(d.mount_point()))
        // Longest matching mount point wins (e.g. a drive mounted inside
        // another drive) — shortest-first iteration order isn't guaranteed.
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space());

    match available {
        Some(bytes) if bytes < MIN_FREE_SPACE_BYTES => Err(format!(
            "אין מספיק שטח פנוי בדיסק להתקנת מנוע הקריינות. פנוי: {}MB, נדרש לפחות {}MB.",
            bytes / 1024 / 1024,
            MIN_FREE_SPACE_BYTES / 1024 / 1024
        )),
        // No matching disk found (unusual mount setup) — don't block
        // provisioning on a check that couldn't resolve; the download/extract
        // steps still fail with their own honest errors if space really is
        // the problem.
        _ => Ok(()),
    }
}

pub async fn provision_narration_engine<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<(), String> {
    let _guard = provision_lock().lock().await;

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
    if PHONIKUD_SHA256.starts_with('<') || TOKENIZER_SHA256.starts_with('<') {
        return Err(
            "מנוע הקריינות עדיין לא מוגדר בקוד (קבועים מסוג placeholder לא הוחלפו בערכי ה-spike האמיתיים)."
                .to_string(),
        );
    }

    check_disk_space()?;

    emit_stage(app, 1, "מוריד את מנהל הסביבה");
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

    // `--clear` below DELETES the existing venv, so it fails on exactly the
    // files a leftover sidecar holds open — its own `python.exe`. That is not
    // hypothetical: a delete that failed partway leaves the venv behind with
    // the process that blocked it still running, and every reinstall attempt
    // then dies here. Stop anything running out of our directory first.
    let survivors = crate::narration_process::kill_processes_under_narration_dir().await;
    if let Some(first) = survivors.first() {
        return Err(format!(
            "לא ניתן להתקין את מנוע הקריינות כי תהליך קודם שלו עדיין פועל ולא ניתן לסגור אותו: {first}. סגור את האפליקציה, פתח מחדש, ונסה שוב."
        ));
    }

    // `uv venv --python 3.11` may need to download an entire Python build
    // (~30-60MB from astral.sh) on first run — 300s covers a slow/stalled
    // connection without hanging the provisioning UI forever.
    emit_stage(app, 2, "מתקין סביבת פייתון (עשוי לקחת דקה)");
    let mut venv_cmd = tokio::process::Command::new(&uv_exe);
    venv_cmd
        // `--managed-python` is what actually guarantees the app's "no Python
        // required" promise: without it, uv is free to satisfy `--python 3.11`
        // from a system install if the user happens to have that exact version,
        // silently making the engine depend on their Python instead of ours.
        // With it, uv only ever uses the interpreter it downloaded into
        // UV_PYTHON_INSTALL_DIR under our own app-data folder.
        // `--clear` is what makes this step retryable: `uv venv` ERRORS on an
        // existing directory, so without it any failure after this point (a
        // dropped connection during the ~308MB download, say) left every
        // retry failing here forever -- with this module's own "each step is
        // idempotent" claim quietly untrue. Rebuilding costs ~20s and
        // guarantees the installed package set matches PIP_PACKAGES.
        .args([
            "venv",
            &venv_dir.to_string_lossy(),
            "--python",
            "3.11",
            "--managed-python",
            "--clear",
        ])
        .envs(uv_env.iter().map(|(k, v)| (*k, v.as_str())));
    // uv is a console program; without this it gets its own visible window.
    #[cfg(target_os = "windows")]
    venv_cmd.creation_flags(CREATE_NO_WINDOW);
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
    emit_stage(app, 3, "מתקין חבילות");
    let mut pip_cmd = tokio::process::Command::new(&uv_exe);
    pip_cmd
        .args(["pip", "install", "--python", &venv_dir.to_string_lossy()])
        .args(PIP_PACKAGES)
        .envs(uv_env.iter().map(|(k, v)| (*k, v.as_str())));
    #[cfg(target_os = "windows")]
    pip_cmd.creation_flags(CREATE_NO_WINDOW);
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
    emit_stage(app, 4, "מוריד הגדרות קול");
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

    emit_stage(app, 5, "מוריד מנתח טקסט");
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

    emit_stage(app, 6, "מוריד קול");
    ensure_voice_downloaded(app, DEFAULT_VOICE).await?;

    // Largest artifact (~308MB), so it goes last: an earlier failure costs
    // the user less bandwidth before it surfaces.
    emit_stage(app, 7, "מוריד מנוע הטעמה (הקובץ הגדול)");
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

    // Reclaim ~63MB per stale voice: the pre-Phonikud piper voice, and any
    // voice dropped from the catalog since it was downloaded. Best-effort —
    // failing to delete a leftover must never fail provisioning.
    let _ = std::fs::remove_file(narration_dir.join("he_IL-saspeech-medium.onnx"));
    let _ = std::fs::remove_file(narration_dir.join("he_IL-saspeech-medium.onnx.json"));
    remove_uncatalogued_voices();

    // Written LAST and only here — this is the atomic completion marker.
    let marker = NarrationEngineMarker {
        marker_version: MARKER_VERSION,
        engine: "phonikud".to_string(),
        voice_name: DEFAULT_VOICE.to_string(),
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

    /// A real-environment smoke test: whatever CI/dev machine runs this has
    /// nowhere near a 1.2GB-free disk being the reason a test fails, so this
    /// mainly proves the mount-point resolution and disk lookup don't panic
    /// or return a false positive against a real filesystem.
    #[test]
    fn check_disk_space_passes_on_a_normal_dev_machine() {
        assert!(check_disk_space().is_ok());
    }

    /// Deleting a directory that is already gone is the SUCCESS case, not a
    /// failure: the delete command runs this after killing the engine's
    /// processes, and a partially-completed earlier attempt can legitimately
    /// have removed it already. Reporting `NotFound` as an error there would
    /// tell the user their cleanup failed when it had in fact finished.
    #[tokio::test]
    async fn remove_dir_all_with_retry_removes_a_tree_and_treats_absent_as_done() {
        let base = std::env::temp_dir().join(format!("hd-narration-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("venv").join("Scripts")).unwrap();
        std::fs::write(base.join("venv").join("Scripts").join("python.exe"), b"not really").unwrap();

        remove_dir_all_with_retry(&base).await.expect("a tree nothing holds open must delete");
        assert!(!base.exists());

        remove_dir_all_with_retry(&base)
            .await
            .expect("an already-absent directory is done, not an error");
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

        // Capture the stage events. The slow steps (venv, pip) emit no
        // download progress, so these are the ONLY signal the UI has that
        // anything is happening — a silent step reads as a hang, which is
        // exactly what a user reported before they existed.
        let seen: std::sync::Arc<std::sync::Mutex<Vec<u8>>> = Default::default();
        let sink = seen.clone();
        tauri::Listener::listen(app.handle(), "narration-setup-stage", move |event: tauri::Event| {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(event.payload()) {
                if let Some(step) = v.get("step").and_then(|s| s.as_u64()) {
                    sink.lock().unwrap().push(step as u8);
                }
            }
        });

        let result = provision_narration_engine(app.handle()).await;
        assert!(result.is_ok(), "provisioning failed: {result:?}");

        let steps = seen.lock().unwrap().clone();
        assert_eq!(
            steps,
            (1..=SETUP_STEPS).collect::<Vec<u8>>(),
            "every step must report, in order, or the UI goes silent mid-install"
        );

        // Every artifact the sidecar's argv points at must actually exist —
        // the marker alone would happily be written over a missing file.
        for path in [
            get_voice_path(DEFAULT_VOICE),
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
                voice_name: DEFAULT_VOICE.to_string(),
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
