mod api_transcribe;
mod audio;
mod enhance;
mod export;
mod injector;
mod model;
mod secure_keys;
mod settings;
mod streaming;
mod whisper;

use audio::AudioRecorder;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconEvent;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

struct ActiveStreaming {
    session: Arc<streaming::StreamingSession>,
    audio_tx: tokio::sync::mpsc::UnboundedSender<Vec<f32>>,
    dispatch_task: tokio::task::JoinHandle<()>,
}

struct AppState {
    recorder: Mutex<AudioRecorder>,
    whisper_engine: Mutex<Option<whisper::WhisperEngine>>,
    settings: Mutex<settings::AppSettings>,
    streaming: tokio::sync::Mutex<Option<ActiveStreaming>>,
    /// Tracks whether the main window was visible when the floating toolbar
    /// took over, so we can restore it to the same state when recording stops.
    main_was_visible_before_toolbar: AtomicBool,
    /// One-shot migration outcome (set at load time, taken at setup time, then None).
    migration_outcome: Mutex<Option<settings::MigrationOutcome>>,
}

#[tauri::command]
fn start_recording(state: State<AppState>) -> Result<(), String> {
    let mut recorder = state.recorder.lock().map_err(|e| e.to_string())?;
    recorder.start_recording()
}

#[tauri::command]
fn stop_recording(state: State<AppState>) -> Result<Vec<f32>, String> {
    let mut recorder = state.recorder.lock().map_err(|e| e.to_string())?;
    recorder.stop_recording()
}

#[tauri::command]
fn is_recording(state: State<AppState>) -> bool {
    state.recorder.lock().map(|r| r.is_recording()).unwrap_or(false)
}

#[tauri::command]
fn pause_recording(state: State<AppState>) -> Result<(), String> {
    let recorder = state.recorder.lock().map_err(|e| e.to_string())?;
    recorder.set_paused(true);
    Ok(())
}

#[tauri::command]
fn resume_recording(state: State<AppState>) -> Result<(), String> {
    let recorder = state.recorder.lock().map_err(|e| e.to_string())?;
    recorder.set_paused(false);
    Ok(())
}

#[tauri::command]
fn is_paused(state: State<AppState>) -> bool {
    state.recorder.lock().map(|r| r.is_paused()).unwrap_or(false)
}

#[tauri::command]
fn check_silence(state: State<AppState>) -> bool {
    state.recorder.lock().map(|r| r.is_silence_detected()).unwrap_or(false)
}

#[tauri::command]
fn check_timeout(state: State<AppState>) -> bool {
    state.recorder.lock().map(|r| r.is_timeout_reached()).unwrap_or(false)
}

#[tauri::command]
fn set_vad_enabled(state: State<AppState>, enabled: bool) -> Result<(), String> {
    let recorder = state.recorder.lock().map_err(|e| e.to_string())?;
    recorder.set_vad_enabled(enabled);
    Ok(())
}

#[tauri::command]
fn set_max_recording_secs(state: State<AppState>, secs: f32) -> Result<(), String> {
    let recorder = state.recorder.lock().map_err(|e| e.to_string())?;
    recorder.set_max_recording_secs(secs);
    Ok(())
}

/// Configure the silence-to-stop duration. Frontend slider value flows here.
#[tauri::command]
fn set_silence_duration_secs(state: State<AppState>, secs: f32) -> Result<(), String> {
    let recorder = state.recorder.lock().map_err(|e| e.to_string())?;
    recorder.set_silence_duration_secs(secs);
    Ok(())
}

/// Choose the input device by name. `device` is `None` (or absent) for system default.
#[tauri::command]
fn set_preferred_audio_device(
    state: State<AppState>,
    device: Option<String>,
) -> Result<(), String> {
    let recorder = state.recorder.lock().map_err(|e| e.to_string())?;
    recorder.set_preferred_device(device);
    Ok(())
}

/// Re-register the global toggle hotkey at runtime. On success, persists the new
/// combo to settings.json. On failure, the previous registration is gone — caller
/// must ask the user to retry, or we fall back below.
#[tauri::command]
fn set_hotkey(app: AppHandle, state: State<AppState>, combo: String) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let trimmed = combo.trim().to_lowercase();
    if trimmed.is_empty() {
        return Err("קיצור ריק — בחר שילוב מקשים תקין".to_string());
    }

    // Capture pause combo so we can restore it after unregister_all wipes everything.
    let pause_combo = state
        .settings
        .lock()
        .ok()
        .and_then(|s| s.pause_hotkey.clone());

    let _ = app.global_shortcut().unregister_all();

    if let Err(e) = register_toggle_shortcut(&app, &trimmed) {
        let prev = state
            .settings
            .lock()
            .map(|s| s.hotkey.clone())
            .unwrap_or_else(|_| "alt+d".to_string());
        let _ = register_toggle_shortcut(&app, &prev);
        if let Some(p) = &pause_combo {
            if !p.eq_ignore_ascii_case(&prev) {
                let _ = register_pause_shortcut(&app, p);
            }
        }
        return Err(e);
    }

    // Re-register pause shortcut if it was active and doesn't conflict with the new toggle.
    if let Some(p) = &pause_combo {
        if !p.eq_ignore_ascii_case(&trimmed) {
            if let Err(e) = register_pause_shortcut(&app, p) {
                eprintln!("Could not restore pause shortcut '{}': {}", p, e);
            }
        }
    }

    let mut s = state.settings.lock().map_err(|e| e.to_string())?;
    s.hotkey = trimmed;
    settings::save_settings(&s)?;
    Ok(())
}

/// Re-register or disable the Pause hotkey at runtime. `combo = None` clears the
/// pause hotkey entirely. Conflicts with the toggle hotkey are rejected.
#[tauri::command]
fn set_pause_hotkey(
    app: AppHandle,
    state: State<AppState>,
    combo: Option<String>,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let normalized = combo.map(|c| c.trim().to_lowercase()).filter(|c| !c.is_empty());

    let toggle_combo = state
        .settings
        .lock()
        .map(|s| s.hotkey.clone())
        .unwrap_or_else(|_| "alt+d".to_string());

    if let Some(c) = &normalized {
        if c.eq_ignore_ascii_case(&toggle_combo) {
            return Err("קיצור ההשהיה זהה לקיצור הראשי — בחר שילוב אחר".to_string());
        }
    }

    // Re-register everything so changing the pause hotkey doesn't leave a stale
    // listener on the previous combo.
    let _ = app.global_shortcut().unregister_all();
    if let Err(e) = register_toggle_shortcut(&app, &toggle_combo) {
        return Err(format!("רענון הקיצור הראשי נכשל: {}", e));
    }

    if let Some(c) = &normalized {
        if let Err(e) = register_pause_shortcut(&app, c) {
            // Toggle still works — surface the error to the UI but don't roll back.
            return Err(e);
        }
    }

    let mut s = state.settings.lock().map_err(|e| e.to_string())?;
    s.pause_hotkey = normalized;
    settings::save_settings(&s)?;
    Ok(())
}

/// Stop the floating toolbar AND show the main window — used when the user
/// clicks the toolbar's stop button (vs. pressing the hotkey from another app).
#[tauri::command]
fn stop_via_toolbar(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    if let Some(t) = app.get_webview_window("toolbar") {
        let _ = t.hide();
    }
    state
        .main_was_visible_before_toolbar
        .store(false, Ordering::Relaxed);
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.set_focus();
    }
    Ok(())
}

fn transcribe_local(state: &State<AppState>, samples: &[f32], lang: &str) -> Result<String, String> {
    let engine = state.whisper_engine.lock().map_err(|e| e.to_string())?;
    match engine.as_ref() {
        Some(engine) => engine.transcribe(samples, lang),
        None => Err("המודל המקומי לא טעון — הורד מודל Whisper מההגדרות לפני תמלול מקומי".to_string()),
    }
}

#[tauri::command]
async fn transcribe(state: State<'_, AppState>, samples: Vec<f32>, language: Option<String>) -> Result<String, String> {
    // Mic captured effectively nothing — muted, disabled, or no OS permission.
    // Surface a clear, actionable message instead of silently returning no text.
    if audio::is_effectively_silent(&samples, 0.01) {
        return Err("לא נקלט קול מהמיקרופון. ודאו שהמיקרופון פתוח ומחובר, ושלאפליקציה יש הרשאה: הגדרות Windows ← פרטיות ← מיקרופון.".to_string());
    }
    let lang = language.unwrap_or_else(|| "he".to_string());
    let (mode, provider, api_key) = {
        let s = state.settings.lock().map_err(|e| e.to_string())?;
        (
            s.transcription_mode.clone(),
            s.api_provider.clone(),
            s.active_api_key().map(|k| k.to_string()),
        )
    };

    match mode {
        settings::TranscriptionMode::Api => {
            let key = api_key.ok_or("מפתח API לא מוגדר — הגדר אותו בהגדרות")?;
            api_transcribe::transcribe_api(&provider, &samples, &key, &lang).await
        }
        settings::TranscriptionMode::Local => {
            transcribe_local(&state, &samples, &lang)
        }
        settings::TranscriptionMode::AutoFallback => {
            // Try API first if key is configured, then fall back to local
            if let Some(key) = api_key {
                match api_transcribe::transcribe_api(&provider, &samples, &key, &lang).await {
                    Ok(text) => Ok(text),
                    Err(api_err) => {
                        eprintln!("API transcription failed, trying local: {}", api_err);
                        transcribe_local(&state, &samples, &lang).map_err(|local_err| {
                            format!("API: {} | מקומי: {}", api_err, local_err)
                        })
                    }
                }
            } else {
                transcribe_local(&state, &samples, &lang)
            }
        }
    }
}

/// Smart Cleanup (רישוף חכם) — opt-in post-transcription enhancement via Groq.
/// Reads `groq_api_key` DIRECTLY (not `active_api_key()`) — enhancement is always
/// Groq, regardless of which provider transcribed. The frontend falls back to the
/// raw transcript on any Err, so this never blocks injection. Double-guards
/// `enhance_enabled` so a stale frontend can't force enhancement.
#[tauri::command]
async fn enhance_text(
    state: State<'_, AppState>,
    text: String,
    mode: Option<String>,
) -> Result<String, String> {
    let (enabled, mode_str, api_key) = {
        let s = state.settings.lock().map_err(|e| e.to_string())?;
        (
            s.enhance_enabled,
            mode.unwrap_or_else(|| s.enhance_mode.clone()),
            s.groq_api_key.clone(),
        )
    };
    if !enabled {
        return Ok(text); // no-op when the feature is off
    }
    let key = api_key
        .filter(|k| !k.is_empty())
        .ok_or("מפתח Groq לא מוגדר — נדרש לרישוף")?;
    let m = enhance::EnhanceMode::from_str(&mode_str);
    enhance::enhance_inner(&text, m, &key)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn start_streaming_transcription(
    state: State<'_, AppState>,
    app: AppHandle,
    language: Option<String>,
) -> Result<(), String> {
    // Only one active session at a time.
    {
        let guard = state.streaming.lock().await;
        if guard.is_some() {
            return Err("כבר פעיל חיבור streaming קיים — המתן רגע ונסה שוב".to_string());
        }
    }

    // Deepgram streaming rejects "auto" — map it to "he" to match the batch path behavior.
    let lang = match language.as_deref() {
        Some("auto") | None => "he".to_string(),
        Some(other) => other.to_string(),
    };
    let api_key = {
        let s = state.settings.lock().map_err(|e| e.to_string())?;
        if !matches!(s.api_provider, settings::ApiProvider::Deepgram) {
            return Err("Streaming זמין רק עם Deepgram. עבור ל-Deepgram בהגדרות.".to_string());
        }
        s.deepgram_api_key
            .clone()
            .filter(|k| !k.is_empty())
            .ok_or_else(|| "מפתח Deepgram לא מוגדר — הגדר אותו בהגדרות.".to_string())?
    };

    // Channel to pipe audio chunks from the CPAL callback (sync) to an async dispatcher.
    let (audio_tx, mut audio_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<f32>>();

    // Attach the chunk callback BEFORE starting the recorder, so the very first CPAL
    // callback (which may fire ~10ms after start) already has somewhere to send audio.
    {
        let recorder = state.recorder.lock().map_err(|e| e.to_string())?;
        let tx_for_cb = audio_tx.clone();
        recorder.set_chunk_callback(move |chunk: &[f32]| {
            let _ = tx_for_cb.send(chunk.to_vec());
        });
    }

    // Start audio capture IMMEDIATELY — before opening the WebSocket. Audio chunks flow
    // into `audio_rx` and buffer there during the WS handshake (~300ms). Once the WS is
    // open below, the dispatch task drains the buffered pre-roll and continues live.
    // This prevents the "first words lost" bug when the user presses Alt+D and starts
    // speaking immediately.
    let start_err = {
        let mut recorder = state.recorder.lock().map_err(|e| e.to_string())?;
        recorder.start_recording().err()
    };
    if let Some(e) = start_err {
        let recorder = state.recorder.lock().map_err(|e| e.to_string())?;
        recorder.clear_chunk_callback();
        return Err(e);
    }

    // Open WebSocket. If this fails, roll back the recorder we just started.
    let session = match streaming::StreamingSession::start(&api_key, &lang, app.clone()).await {
        Ok(s) => s,
        Err(e) => {
            let mut recorder = state.recorder.lock().map_err(|e| e.to_string())?;
            recorder.clear_chunk_callback();
            let _ = recorder.stop_recording();
            return Err(e);
        }
    };

    let session_for_task = session.clone();
    let dispatch_task = tokio::spawn(async move {
        while let Some(chunk) = audio_rx.recv().await {
            if let Err(e) = session_for_task.send_audio_pcm16(&chunk).await {
                eprintln!("streaming send error: {}", e);
                break;
            }
        }
    });

    // Store the active session so stop_streaming_transcription can find it.
    let mut guard = state.streaming.lock().await;
    *guard = Some(ActiveStreaming {
        session,
        audio_tx,
        dispatch_task,
    });

    Ok(())
}

#[tauri::command]
async fn stop_streaming_transcription(state: State<'_, AppState>) -> Result<String, String> {
    // Stop the CPAL stream FIRST. With the chunk callback still wired, the final 10-30ms
    // of WASAPI-buffered audio is delivered via the callback into `audio_tx` before the
    // stream is dropped. clear_chunk_callback AFTER ensures nothing further is queued.
    // This prevents the "last words cut off" bug.
    {
        let mut recorder = state.recorder.lock().map_err(|e| e.to_string())?;
        let _ = recorder.stop_recording();
    }
    {
        let recorder = state.recorder.lock().map_err(|e| e.to_string())?;
        recorder.clear_chunk_callback();
    }

    let active = {
        let mut guard = state.streaming.lock().await;
        guard.take()
    };

    let Some(active) = active else {
        return Err("אין חיבור streaming פעיל".to_string());
    };

    // Drop the sender so the dispatch task terminates when the channel drains.
    drop(active.audio_tx);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), active.dispatch_task).await;

    // Close the WebSocket and return the accumulated final text.
    let text = active.session.stop().await?;
    Ok(text)
}

#[tauri::command]
fn load_whisper_model(state: State<AppState>, model_name: String) -> Result<(), String> {
    model::validate_model_name(&model_name)?;

    let required_mb = match model_name.as_str() {
        "tiny" => 400,
        "base" => 700,
        "small" => 1500,
        "medium" => 3500,
        "large-v3-turbo" => 6000,
        "ivrit-large-v3-turbo" => 6000,
        _ => 1000,
    };
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let available_mb = sys.available_memory() / (1024 * 1024);
    if available_mb < required_mb {
        return Err(format!(
            "אין מספיק זיכרון RAM. נדרש: ~{}MB, זמין: {}MB. סגור תוכנות אחרות או בחר מודל קטן יותר.",
            required_mb, available_mb
        ));
    }

    let model_path = model::get_model_path(&model_name);
    if !model_path.exists() {
        return Err(format!("Model not found: {}", model_path.display()));
    }

    let engine = whisper::WhisperEngine::new(&model_path, model_name.clone())?;
    let mut whisper = state.whisper_engine.lock().map_err(|e| e.to_string())?;
    *whisper = Some(engine);
    Ok(())
}

#[tauri::command]
fn is_whisper_loaded(state: State<AppState>) -> bool {
    state
        .whisper_engine
        .lock()
        .map(|e| e.is_some())
        .unwrap_or(false)
}

#[tauri::command]
fn is_model_downloaded(model_name: String) -> bool {
    model::is_model_downloaded(&model_name)
}

#[tauri::command]
async fn download_model(app: AppHandle, model_name: String) -> Result<String, String> {
    model::download_model(app, model_name).await
}

#[tauri::command]
fn get_all_models_status() -> Vec<model::ModelInfo> {
    model::get_all_models_status()
}

#[tauri::command]
fn delete_model(state: State<AppState>, model_name: String) -> Result<(), String> {
    model::delete_model(&model_name)?;
    let mut engine = state.whisper_engine.lock().map_err(|e| e.to_string())?;
    *engine = None;
    Ok(())
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Result<settings::RedactedSettings, String> {
    let s = state.settings.lock().map_err(|e| e.to_string())?;
    Ok(s.redacted())
}

#[tauri::command]
fn update_settings(state: State<AppState>, new_settings: settings::AppSettings) -> Result<(), String> {
    let mut s = state.settings.lock().map_err(|e| e.to_string())?;
    // Preserve backend-managed fields (API keys, onboarding/terms flags, toolbar
    // position) — the frontend's persistSettings omits them, so trusting the
    // payload would reset them to serde defaults on every save (wizard reappears,
    // toolbar position lost, keys dropped). See AppSettings::merge_frontend_update.
    let merged = s.merge_frontend_update(new_settings);
    settings::save_settings(&merged)?;
    *s = merged;
    Ok(())
}

/// Save an API key to OS-secure storage (Windows Credential Manager / macOS Keychain)
/// and update the in-memory cache so subsequent transcribe calls find it.
/// `provider` must be `"deepgram"` or `"groq"`.
#[tauri::command]
fn set_api_key(state: State<AppState>, provider: String, key: String) -> Result<(), String> {
    if key.is_empty() {
        return Err("מפתח ריק — אם ברצונך למחוק, השתמש ב-clear_api_key".to_string());
    }
    let provider_id: &'static str = match provider.as_str() {
        "deepgram" => "deepgram",
        "groq" => "groq",
        other => return Err(format!("ספק לא נתמך: {}", other)),
    };
    secure_keys::save_key(provider_id, &key)?;
    let mut s = state.settings.lock().map_err(|e| e.to_string())?;
    match provider_id {
        "deepgram" => s.deepgram_api_key = Some(key),
        "groq" => s.groq_api_key = Some(key),
        _ => unreachable!(),
    }
    Ok(())
}

/// Remove an API key from OS-secure storage and clear the in-memory cache.
#[tauri::command]
fn clear_api_key(state: State<AppState>, provider: String) -> Result<(), String> {
    let provider_id: &'static str = match provider.as_str() {
        "deepgram" => "deepgram",
        "groq" => "groq",
        other => return Err(format!("ספק לא נתמך: {}", other)),
    };
    secure_keys::delete_key(provider_id)?;
    let mut s = state.settings.lock().map_err(|e| e.to_string())?;
    match provider_id {
        "deepgram" => s.deepgram_api_key = None,
        "groq" => s.groq_api_key = None,
        _ => unreachable!(),
    }
    Ok(())
}

#[tauri::command]
async fn test_api_key(provider: settings::ApiProvider, api_key: String) -> Result<String, String> {
    api_transcribe::test_api_key(&provider, &api_key).await?;
    Ok("ok".to_string())
}

/// Mark the onboarding wizard as completed without disturbing any other settings.
/// Used to backfill the flag for users who configured keys directly in the settings
/// view and should not see the wizard on every launch.
#[tauri::command]
fn mark_onboarding_complete(state: State<AppState>) -> Result<(), String> {
    let mut s = state.settings.lock().map_err(|e| e.to_string())?;
    if !s.onboarding_completed {
        s.onboarding_completed = true;
        settings::save_settings(&s)?;
    }
    Ok(())
}

/// Record that the user has accepted the terms of use shown in the wizard.
/// Persists to settings.json so the terms gate is not re-shown on every launch.
#[tauri::command]
fn accept_terms(state: State<AppState>) -> Result<(), String> {
    let mut s = state.settings.lock().map_err(|e| e.to_string())?;
    if !s.terms_accepted {
        s.terms_accepted = true;
        settings::save_settings(&s)?;
    }
    Ok(())
}

#[tauri::command]
fn inject_text(app: AppHandle, text: String) -> Result<(), String> {
    // Hide the main window briefly so its focus doesn't capture the simulated Ctrl+V.
    // Without this, a focused Tauri window swallows the paste and nothing lands in the target app.
    let main_window = app.get_webview_window("main");
    let was_visible = main_window
        .as_ref()
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);

    if was_visible {
        if let Some(w) = &main_window {
            let _ = w.hide();
        }
        // Let Windows promote the previously-active window to the foreground.
        std::thread::sleep(std::time::Duration::from_millis(80));
    }

    let result = injector::inject_text(&text, &injector::InjectionMethod::Clipboard);

    if was_visible {
        if let Some(w) = &main_window {
            let _ = w.show();
        }
    }

    result
}

/// Export the user's dictation history to a TXT or DOCX file. The frontend
/// passes the items so the backend doesn't need to manage history persistence.
/// `format` must be "txt" or "docx".
#[tauri::command]
async fn export_history(
    app: AppHandle,
    items: Vec<export::HistoryItem>,
    format: String,
) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;

    if items.is_empty() {
        return Err("אין פריטים להיסטוריה — הקלט קודם מספר תמלולים.".to_string());
    }

    let format_lc = format.to_lowercase();
    let (extension, dialog_label) = match format_lc.as_str() {
        "txt" => ("txt", "קובץ טקסט"),
        "docx" => ("docx", "מסמך Word"),
        _ => return Err(format!("פורמט לא נתמך: {}", format)),
    };

    let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M");
    let default_name = format!("hebrew-dictation-history_{}.{}", timestamp, extension);

    // tauri-plugin-dialog `save` is callback-based — wrap it in a oneshot channel.
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<std::path::PathBuf>>();
    app.dialog()
        .file()
        .set_title(&format!("שמור את ההיסטוריה כ{}", dialog_label))
        .set_file_name(&default_name)
        .add_filter(dialog_label, &[extension])
        .save_file(move |result| {
            // FilePath -> PathBuf via Display (cross-platform-safe enough for our case).
            let path = result.and_then(|fp| {
                fp.into_path().ok()
            });
            let _ = tx.send(path);
        });

    let path = rx
        .await
        .map_err(|_| "דיאלוג השמירה נסגר ללא תגובה".to_string())?;
    let path = match path {
        Some(p) => p,
        None => return Err("הייצוא בוטל".to_string()),
    };

    match format_lc.as_str() {
        "txt" => export::write_txt(&path, &items)?,
        "docx" => export::write_docx(&path, &items)?,
        _ => unreachable!(),
    }

    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
fn get_audio_devices() -> Result<Vec<String>, String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let devices: Vec<String> = host
        .input_devices()
        .map_err(|e| e.to_string())?
        .filter_map(|d| d.name().ok())
        .collect();
    Ok(devices)
}

#[tauri::command]
fn set_window_always_on_top(app: AppHandle, enabled: bool) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window.set_always_on_top(enabled).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Logical dimensions of the toolbar window — must match `tauri.conf.json`.
const TOOLBAR_W: f64 = 220.0;
const TOOLBAR_H: f64 = 76.0;

/// Logical dimensions of the idle floating button (same window, circular mode).
const IDLE_W: f64 = 56.0;
const IDLE_H: f64 = 56.0;

/// Compute the on-screen logical position for a floating window of size `w`×`h`.
/// Honors the user's saved drag position when it stays on the active monitor,
/// else falls back to bottom-center, 80px above the bottom edge. Shared by the
/// recording toolbar and the idle button so both stick to the same spot.
fn resolve_float_position(
    app: &AppHandle,
    saved_pos: Option<settings::ToolbarPosition>,
    w: f64,
    h: f64,
) -> Option<(f64, f64)> {
    let main = app.get_webview_window("main");
    let toolbar = app.get_webview_window("toolbar")?;
    let monitor = main
        .as_ref()
        .and_then(|win| win.current_monitor().ok().flatten())
        .or_else(|| toolbar.primary_monitor().ok().flatten())?;

    let scale = monitor.scale_factor();
    let mon_size = monitor.size();
    let mon_pos = monitor.position();
    let logical_w = mon_size.width as f64 / scale;
    let logical_h = mon_size.height as f64 / scale;
    let logical_x = mon_pos.x as f64 / scale;
    let logical_y = mon_pos.y as f64 / scale;

    let default_x = logical_x + (logical_w - w) / 2.0;
    let default_y = logical_y + logical_h - h - 80.0;

    let (x, y) = match saved_pos {
        Some(p) => {
            let min_x = logical_x - w + 40.0;
            let max_x = logical_x + logical_w - 40.0;
            let min_y = logical_y - h + 20.0;
            let max_y = logical_y + logical_h - 20.0;
            if p.x < min_x || p.x > max_x || p.y < min_y || p.y > max_y {
                (default_x, default_y)
            } else {
                (p.x, p.y)
            }
        }
        None => (default_x, default_y),
    };
    Some((x, y))
}

/// Show the small floating idle button by reusing the `toolbar` window in a
/// 56×56 circular mode. The `toolbar-mode` event tells the webview to render
/// the circle instead of the recording bar.
///
/// Enforces the core invariant *idle circle visible ⟺ main window hidden*:
/// the circle and the main window must never be on screen at the same time,
/// so we defensively hide main here. All real callers already run with main
/// hidden, so this is a no-op in practice — it just closes any future gap.
fn show_idle_button_inner(app: &AppHandle, saved_pos: Option<settings::ToolbarPosition>) {
    let Some(toolbar) = app.get_webview_window("toolbar") else {
        return;
    };
    if let Some(main) = app.get_webview_window("main") {
        if main.is_visible().unwrap_or(false) {
            let _ = main.hide();
        }
    }
    if let Some((x, y)) = resolve_float_position(app, saved_pos, IDLE_W, IDLE_H) {
        let _ = toolbar.set_position(tauri::LogicalPosition::new(x, y));
    }
    let _ = toolbar.set_size(tauri::LogicalSize::new(IDLE_W, IDLE_H));
    let _ = toolbar.set_always_on_top(true);
    let _ = app.emit("toolbar-mode", "idle");
    let _ = toolbar.show();
}

/// Re-evaluate whether the idle button should be on screen: show it when the
/// feature is on and the main window is hidden; otherwise hide the toolbar
/// window (covers the idle circle — same window). Skipped while a recording
/// is active so the recording bar is never yanked out from under the user.
fn refresh_idle_button(app: &AppHandle, state: &AppState) {
    let recording = state
        .recorder
        .lock()
        .map(|r| r.is_recording())
        .unwrap_or(false);
    if recording {
        return;
    }
    let (enabled, saved_pos) = {
        let s = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        (s.idle_button_enabled, s.toolbar_position)
    };
    let main_visible = app
        .get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if enabled && !main_visible {
        show_idle_button_inner(app, saved_pos);
    } else if let Some(t) = app.get_webview_window("toolbar") {
        let _ = t.hide();
    }
}

/// Bring the main window back from the idle button (right-click on the
/// circle). Hides the floating window — same one — and surfaces + focuses
/// main so the user can reach settings / history without the tray.
#[tauri::command]
fn open_main_window(app: AppHandle) -> Result<(), String> {
    if let Some(t) = app.get_webview_window("toolbar") {
        let _ = t.hide();
    }
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.set_focus();
    }
    Ok(())
}

/// Command wrapper — show the idle button now (no-op unless enabled).
#[tauri::command]
fn show_idle_button(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let (enabled, saved_pos) = {
        let s = state.settings.lock().map_err(|e| e.to_string())?;
        (s.idle_button_enabled, s.toolbar_position)
    };
    if enabled {
        show_idle_button_inner(&app, saved_pos);
    }
    Ok(())
}

/// Persist the idle-button toggle and immediately reflect it on screen.
#[tauri::command]
fn set_idle_button_enabled(
    app: AppHandle,
    state: State<AppState>,
    enabled: bool,
) -> Result<(), String> {
    {
        let mut s = state.settings.lock().map_err(|e| e.to_string())?;
        s.idle_button_enabled = enabled;
        settings::save_settings(&s)?;
    }
    refresh_idle_button(&app, &state);
    Ok(())
}

/// Show the floating toolbar at the bottom-center of the active monitor
/// (or at the user's saved drag position) and hide the main window (remembered
/// for restore on hide).
///
/// `streaming` forces main to hide even when the toolbar itself is disabled —
/// this keeps focus on the user's target app so live injection lands there,
/// not in our own window.
#[tauri::command]
fn show_toolbar_window(
    app: AppHandle,
    state: State<AppState>,
    streaming: bool,
) -> Result<(), String> {
    let (toolbar_enabled, idle_enabled, saved_pos) = {
        let s = state.settings.lock().map_err(|e| e.to_string())?;
        (s.floating_toolbar_enabled, s.idle_button_enabled, s.toolbar_position)
    };

    // When the idle button is on, the toolbar window is already on screen as a
    // circle — recording must grow it into the bar even if the user disabled
    // the floating toolbar for the main-window flow.
    let show_bar = toolbar_enabled || idle_enabled;
    let should_hide_main = show_bar || streaming;
    if !should_hide_main {
        return Ok(());
    }

    let main = app.get_webview_window("main");
    let toolbar = app
        .get_webview_window("toolbar")
        .ok_or_else(|| "toolbar window not found".to_string())?;

    let was_visible = main
        .as_ref()
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    state
        .main_was_visible_before_toolbar
        .store(was_visible, Ordering::Relaxed);

    if show_bar {
        if let Some((x, y)) = resolve_float_position(&app, saved_pos, TOOLBAR_W, TOOLBAR_H) {
            let _ = toolbar.set_position(tauri::LogicalPosition::new(x, y));
        }
        // Grow from idle-circle size back to the full recording bar and tell
        // the webview to render the recording layout.
        let _ = toolbar.set_size(tauri::LogicalSize::new(TOOLBAR_W, TOOLBAR_H));
        let _ = toolbar.set_always_on_top(true);
        let _ = app.emit("toolbar-mode", "recording");
        let _ = toolbar.show();
    }

    if let Some(w) = &main {
        let _ = w.hide();
    }

    Ok(())
}

/// Persist the toolbar's screen position after the user drags it. Called from
/// the ToolbarApp's `tauri://move` event handler (debounced on the JS side).
#[tauri::command]
fn set_toolbar_position(state: State<AppState>, x: f64, y: f64) -> Result<(), String> {
    let mut s = state.settings.lock().map_err(|e| e.to_string())?;
    s.toolbar_position = Some(settings::ToolbarPosition { x, y });
    settings::save_settings(&s)?;
    Ok(())
}

/// Hide the floating toolbar and restore the main window if it was visible
/// before the toolbar took over.
#[tauri::command]
fn hide_toolbar_window(
    app: AppHandle,
    state: State<AppState>,
    force_show_main: Option<bool>,
) -> Result<(), String> {
    if let Some(t) = app.get_webview_window("toolbar") {
        let _ = t.hide();
    }

    let was_visible = state
        .main_was_visible_before_toolbar
        .swap(false, Ordering::Relaxed);
    let force = force_show_main.unwrap_or(false);

    // Force is set when the user clicked the toolbar's stop button — they want
    // to see the transcription, so promote the main window even if it wasn't
    // visible before. Alt+D toggling preserves the original behavior.
    let (idle_enabled, saved_pos) = {
        let s = state.settings.lock().map_err(|e| e.to_string())?;
        (s.idle_button_enabled, s.toolbar_position)
    };

    if idle_enabled && !was_visible {
        // The dictation session ran from the idle circle (main was never up).
        // Return to the circle instead of yanking the main window into the
        // user's face — this also avoids stealing focus from their target
        // app right after the text was injected.
        show_idle_button_inner(&app, saved_pos);
    } else if was_visible || force {
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.show();
            if force {
                let _ = main.set_focus();
            }
        }
    }

    Ok(())
}

#[tauri::command]
fn set_autostart_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let autostart = app.autolaunch();
    if enabled {
        autostart.enable().map_err(|e| e.to_string())?;
    } else {
        autostart.disable().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Try to register the toggle hotkey on the global shortcut manager. The combo
/// string follows the `tauri_plugin_global_shortcut` syntax — "alt+d", "ctrl+shift+f1",
/// etc. Returns a Hebrew error string on parse failure or OS-level conflict.
fn register_toggle_shortcut(app: &AppHandle, combo: &str) -> Result<(), String> {
    let parsed: Shortcut = combo
        .parse()
        .map_err(|e| format!("פורמט קיצור לא תקין ('{}'): {}", combo, e))?;

    let app_handle = app.clone();
    app.global_shortcut()
        .on_shortcut(parsed, move |_app, shortcut, event| {
            if event.state == ShortcutState::Pressed {
                // Emit event without showing/focusing the window — keeps focus in the text field
                let _ = app_handle.emit("hotkey-pressed", shortcut.to_string());
            }
        })
        .map_err(|e| format!("רישום הקיצור נכשל ('{}'): {}", combo, e))
}

/// Register a Pause/Resume hotkey. Emits `pause-pressed` to the frontend, which
/// decides whether to call `pause_recording` or `resume_recording`. Independent
/// of the toggle hotkey — only fires while a recording is active.
fn register_pause_shortcut(app: &AppHandle, combo: &str) -> Result<(), String> {
    let parsed: Shortcut = combo
        .parse()
        .map_err(|e| format!("פורמט קיצור לא תקין ('{}'): {}", combo, e))?;

    let app_handle = app.clone();
    app.global_shortcut()
        .on_shortcut(parsed, move |_app, shortcut, event| {
            if event.state == ShortcutState::Pressed {
                let _ = app_handle.emit("pause-pressed", shortcut.to_string());
            }
        })
        .map_err(|e| format!("רישום קיצור ההשהיה נכשל ('{}'): {}", combo, e))
}

/// Apply the user's preferred hotkeys on startup. Falls back to "alt+d" for the
/// toggle if its registration fails (a corrupted settings.json picks a combo
/// Windows already grabbed). Pause hotkey failure is non-fatal — the toolbar
/// still has its own Pause button.
fn setup_global_shortcuts(app: &AppHandle, combo: &str, pause_combo: Option<&str>) {
    if let Err(e) = register_toggle_shortcut(app, combo) {
        eprintln!("Hotkey '{}' failed to register: {}. Falling back to alt+d.", combo, e);
        if let Err(e2) = register_toggle_shortcut(app, "alt+d") {
            eprintln!("Fallback alt+d also failed: {}", e2);
        }
    }
    if let Some(pause) = pause_combo {
        // Skip silently if pause matches toggle — already-registered will error,
        // and the user can clear it from settings.
        if pause.eq_ignore_ascii_case(combo) {
            eprintln!("Pause hotkey same as toggle — skipping registration");
        } else if let Err(e) = register_pause_shortcut(app, pause) {
            eprintln!("Pause hotkey '{}' failed to register: {}", pause, e);
        }
    }
}

fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let show_item = MenuItemBuilder::with_id("show", "הגדרות").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "יציאה").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&show_item)
        .separator()
        .item(&quit_item)
        .build()?;

    if let Some(tray) = app.tray_by_id("main") {
        tray.set_menu(Some(menu))?;
        tray.on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
                // Main is back — the idle circle (same window) must step aside.
                if let Some(t) = app.get_webview_window("toolbar") {
                    let _ = t.hide();
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        });

        let app_clone = app.clone();
        tray.on_tray_icon_event(move |_tray, event| {
            if let TrayIconEvent::DoubleClick { .. } = event {
                if let Some(window) = app_clone.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
                if let Some(t) = app_clone.get_webview_window("toolbar") {
                    let _ = t.hide();
                }
            }
        });
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage({
            let load_result = settings::load_settings();
            AppState {
                recorder: Mutex::new(AudioRecorder::new()),
                whisper_engine: Mutex::new(None),
                settings: Mutex::new(load_result.settings),
                streaming: tokio::sync::Mutex::new(None),
                main_was_visible_before_toolbar: AtomicBool::new(false),
                migration_outcome: Mutex::new(Some(load_result.migration)),
            }
        })
        .setup(|app| {
            // Apply persisted recorder settings BEFORE wiring shortcuts so the very
            // first hotkey press uses the user's configured behavior.
            {
                let state = app.state::<AppState>();
                let s = state
                    .settings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let recorder = state
                    .recorder
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                recorder.set_vad_enabled(s.vad_enabled);
                recorder.set_silence_duration_secs(s.vad_silence_secs);
                let effective_max = if s.unlimited_recording {
                    3600.0
                } else {
                    s.max_recording_secs
                };
                recorder.set_max_recording_secs(effective_max);
                recorder.set_preferred_device(s.preferred_audio_device.clone());
                // Wire the AppHandle so the VAD monitor thread can emit
                // `audio-level` and `vad-state` events to the floating toolbar.
                recorder.set_app_handle(app.handle().clone());
            }

            // Read the user's preferred hotkeys and register them (toggle + optional pause).
            let (combo, pause_combo) = {
                let state = app.state::<AppState>();
                let s = state
                    .settings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                (s.hotkey.clone(), s.pause_hotkey.clone())
            };
            setup_global_shortcuts(app.handle(), &combo, pause_combo.as_deref());
            let _ = setup_tray(app.handle());

            // Surface the one-shot key migration result (if any) to the frontend.
            // Only ever fires once per app launch — we `take()` the value here.
            let outcome = {
                let state = app.state::<AppState>();
                let mut guard = state
                    .migration_outcome
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                guard.take()
            };
            if let Some(outcome) = outcome {
                use settings::MigrationOutcome;
                match outcome {
                    MigrationOutcome::Migrated { providers } => {
                        let _ = app.emit(
                            "key-migration",
                            serde_json::json!({
                                "status": "migrated",
                                "providers": providers,
                            }),
                        );
                    }
                    MigrationOutcome::Failed { error } => {
                        let _ = app.emit(
                            "key-migration",
                            serde_json::json!({
                                "status": "failed",
                                "error": error,
                            }),
                        );
                    }
                    MigrationOutcome::NoOp => {}
                }
            }

            use tauri_plugin_autostart::ManagerExt;
            let autostart = app.autolaunch();
            let (autostart_wanted, always_on_top_wanted, close_notif_shown) = {
                let state = app.state::<AppState>();
                let s = state.settings.lock().unwrap_or_else(|e| e.into_inner());
                (s.autostart_enabled, s.always_on_top, s.close_notification_shown)
            };
            let autostart_active = autostart.is_enabled().unwrap_or(false);
            if autostart_wanted && !autostart_active {
                let _ = autostart.enable();
            } else if !autostart_wanted && autostart_active {
                let _ = autostart.disable();
            }

            let start_minimized = std::env::args().any(|a| a == "--minimized");

            let (idle_enabled, idle_saved_pos) = {
                let state = app.state::<AppState>();
                let s = state.settings.lock().unwrap_or_else(|e| e.into_inner());
                (s.idle_button_enabled, s.toolbar_position)
            };

            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_always_on_top(always_on_top_wanted);
                if start_minimized {
                    let _ = window.hide();
                    // Autostart launched us hidden — the #1 "I don't know it's
                    // running" fix: surface the idle button right away.
                    if idle_enabled {
                        show_idle_button_inner(app.handle(), idle_saved_pos);
                    }
                }
                let w = window.clone();
                let app_for_close = app.handle().clone();
                let notif_sent = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(close_notif_shown));
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        if !notif_sent.load(std::sync::atomic::Ordering::Relaxed) {
                            notif_sent.store(true, std::sync::atomic::Ordering::Relaxed);
                            let _ = w.emit("window-close-attempted", ());
                        }
                        let _ = w.hide();
                        // Closed to tray — keep an on-screen affordance so the
                        // user can still start dictation with one click.
                        let state = app_for_close.state::<AppState>();
                        let (enabled, saved_pos) = {
                            let s = state.settings.lock().unwrap_or_else(|e| e.into_inner());
                            (s.idle_button_enabled, s.toolbar_position)
                        };
                        if enabled {
                            show_idle_button_inner(&app_for_close, saved_pos);
                        }
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_recording,
            stop_recording,
            is_recording,
            pause_recording,
            resume_recording,
            is_paused,
            check_silence,
            check_timeout,
            set_vad_enabled,
            set_max_recording_secs,
            set_silence_duration_secs,
            set_preferred_audio_device,
            set_hotkey,
            set_pause_hotkey,
            stop_via_toolbar,
            transcribe,
            start_streaming_transcription,
            stop_streaming_transcription,
            mark_onboarding_complete,
            accept_terms,
            load_whisper_model,
            is_whisper_loaded,
            is_model_downloaded,
            download_model,
            delete_model,
            get_all_models_status,
            get_settings,
            update_settings,
            set_api_key,
            clear_api_key,
            test_api_key,
            inject_text,
            enhance_text,
            export_history,
            get_audio_devices,
            set_window_always_on_top,
            set_autostart_enabled,
            show_toolbar_window,
            hide_toolbar_window,
            set_toolbar_position,
            show_idle_button,
            set_idle_button_enabled,
            open_main_window,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
