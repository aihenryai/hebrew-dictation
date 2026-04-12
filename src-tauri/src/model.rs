use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

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
];

const VALID_MODEL_NAMES: &[&str] = &["tiny", "base", "small", "medium", "large-v3-turbo"];

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
                    "האיכות הגבוהה ביותר לעברית, דורש 6GB+ RAM".to_string(),
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

    // Check if already downloaded and valid
    if model_path.exists() {
        let metadata = std::fs::metadata(&model_path).map_err(|e| e.to_string())?;
        if metadata.len() == *expected_size {
            return Ok(model_path.to_string_lossy().to_string());
        }
    }

    // Create directory
    let models_dir = get_models_dir();
    std::fs::create_dir_all(&models_dir)
        .map_err(|e| format!("Failed to create models dir: {}", e))?;

    // Download with progress
    let client = reqwest::Client::new();
    let response = client
        .get(*url)
        .send()
        .await
        .map_err(|e| format!("Download request failed: {}", e))?;

    let total_size = response.content_length().unwrap_or(*expected_size);

    let tmp_path = model_path.with_extension("bin.tmp");
    let mut file = tokio::fs::File::create(&tmp_path)
        .await
        .map_err(|e| format!("Failed to create file: {}", e))?;

    let mut downloaded: u64 = 0;
    let mut hasher = Sha256::new();
    let mut stream = response.bytes_stream();
    let max_size = expected_size + (expected_size / 10); // 10% tolerance

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download error: {}", e))?;

        downloaded += chunk.len() as u64;

        // Abort if download exceeds expected size
        if downloaded > max_size {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err("Download exceeded expected size — aborting".to_string());
        }

        hasher.update(&chunk);

        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|e| format!("Write error: {}", e))?;

        let progress = (downloaded as f64 / total_size as f64 * 100.0) as u32;
        let _ = app.emit(
            "model-download-progress",
            serde_json::json!({
                "downloaded": downloaded,
                "total": total_size,
                "progress": progress
            }),
        );
    }

    // Verify SHA-256 hash
    let hash_result = format!("{:x}", hasher.finalize());
    if hash_result != *expected_hash {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(format!(
            "Hash mismatch! Expected: {}..., Got: {}... — file deleted",
            &expected_hash[..16],
            &hash_result[..16]
        ));
    }

    // Rename tmp to final
    std::fs::rename(&tmp_path, &model_path)
        .map_err(|e| format!("Failed to finalize download: {}", e))?;

    Ok(model_path.to_string_lossy().to_string())
}
