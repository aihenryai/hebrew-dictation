use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;

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
/// `AppHandle<MockRuntime>`** in a later task that needs to write a real
/// integration test without a live window. Every existing call site
/// (`download_model` below) keeps calling this with a plain `&AppHandle`
/// unchanged — `R` is inferred as `Wry` automatically there, so this is not a
/// breaking change to any `#[tauri::command]` entry point.
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

/// (name, url, expected_size, sha256_hex)
const MODELS: &[(&str, &str, u64, &str)] = &[
    (
        "tiny",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
        77_691_713,
        "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
    ),
    (
        "base",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        147_951_465,
        "60ed5bc3dd14eea856493d334f5408d7e5e0b243a58c35bf5fde5b114b5b6cf6",
    ),
    (
        "small",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
        487_601_967,
        "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
    ),
    (
        "medium",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
        1_533_774_781,
        "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
    ),
    (
        "large-v3-turbo",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
        1_624_555_275,
        "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
    ),
    (
        // ivrit.ai fine-tune of Whisper large-v3-turbo, trained on ~5,000h of
        // Hebrew (Knesset plenums, crowd-transcribed audio, Wikipedia). Same
        // GGML format / size as the standard turbo. Apache-2.0.
        "ivrit-large-v3-turbo",
        "https://huggingface.co/ivrit-ai/whisper-large-v3-turbo-ggml/resolve/main/ggml-model.bin",
        1_624_555_275,
        "c8090411113357097bfafc2b8e228ec1639fa7f5fe4ecb5d054ac0ccef8641b1",
    ),
];

const VALID_MODEL_NAMES: &[&str] = &[
    "tiny",
    "base",
    "small",
    "medium",
    "large-v3-turbo",
    "ivrit-large-v3-turbo",
];

pub fn validate_model_name(model_name: &str) -> Result<(), String> {
    if VALID_MODEL_NAMES.contains(&model_name) {
        Ok(())
    } else {
        Err(format!(
            "Invalid model name '{}'. Valid: {}",
            model_name,
            VALID_MODEL_NAMES.join(", ")
        ))
    }
}

pub fn get_models_dir() -> PathBuf {
    let app_data = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    app_data.join("hebrew-dictation").join("models")
}

pub fn get_model_path(model_name: &str) -> PathBuf {
    get_models_dir().join(format!("ggml-{}.bin", model_name))
}

pub fn is_model_downloaded(model_name: &str) -> bool {
    if validate_model_name(model_name).is_err() {
        return false;
    }
    get_model_path(model_name).exists()
}

#[derive(serde::Serialize)]
pub struct ModelInfo {
    pub name: String,
    pub size_bytes: u64,
    pub size_label: String,
    pub downloaded: bool,
    pub description: String,
}

pub fn get_all_models_status() -> Vec<ModelInfo> {
    MODELS
        .iter()
        .map(|(name, _, size, _)| {
            let downloaded = get_model_path(name).exists();
            let (size_label, description) = match *name {
                "tiny" => ("~75MB".to_string(), "מיידי, דיוק נמוך בעברית".to_string()),
                "base" => ("~140MB".to_string(), "מהיר, דיוק סביר".to_string()),
                "small" => ("~500MB".to_string(), "מאוזן, מומלץ לרוב המשתמשים".to_string()),
                "medium" => ("~1.5GB".to_string(), "מדויק לעברית, דורש 4GB+ RAM".to_string()),
                "large-v3-turbo" => (
                    "~1.6GB".to_string(),
                    "Whisper סטנדרטי, איכות גבוהה, דורש 6GB+ RAM".to_string(),
                ),
                "ivrit-large-v3-turbo" => (
                    "~1.6GB".to_string(),
                    "מותאם לעברית — מודל ivrit.ai מאומן ~5,000 שעות (כנסת + ויקי). מומלץ לעברית. דורש 6GB+ RAM".to_string(),
                ),
                _ => (format!("{}B", size), String::new()),
            };
            ModelInfo {
                name: name.to_string(),
                size_bytes: *size,
                size_label,
                downloaded,
                description,
            }
        })
        .collect()
}

pub fn delete_model(model_name: &str) -> Result<(), String> {
    validate_model_name(model_name)?;
    let path = get_model_path(model_name);
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Failed to delete model: {}", e))?;
    }
    Ok(())
}

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

/// Friendly Hebrew label for the model name used in user-facing notifications.
fn friendly_model_label(name: &str) -> String {
    match name {
        "tiny" => "Tiny — מהיר".to_string(),
        "base" => "Base — בסיסי".to_string(),
        "small" => "Small — מאוזן".to_string(),
        "medium" => "Medium — מדויק".to_string(),
        "large-v3-turbo" => "Large v3 Turbo".to_string(),
        "ivrit-large-v3-turbo" => "ivrit.ai (מותאם לעברית)".to_string(),
        other => other.to_string(),
    }
}

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
