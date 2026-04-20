mod api_transcribe;
mod audio;
mod injector;
mod model;
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

fn transcribe_local(state: &State<AppState>, samples: &[f32], lang: &str) -> Result<String, String> {
    let engine = state.whisper_engine.lock().map_err(|e| e.to_string())?;
    match engine.as_ref() {
        Some(engine) => engine.transcribe(samples, lang),
        None => Err("המודל המקומי לא טעון — הורד מודל Whisper מההגדרות לפני תמלול מקומי".to_string()),
    }
}

#[tauri::command]
async fn transcribe(state: State<'_, AppState>, samples: Vec<f32>, language: Option<String>) -> Result<String, String> {
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

    // Open WebSocket connection. If this fails, we surface the error and nothing is started.
    let session = streaming::StreamingSession::start(&api_key, &lang, app.clone()).await?;

    // Channel to pipe audio chunks from the CPAL callback (sync) to an async dispatcher.
    let (audio_tx, mut audio_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<f32>>();

    let session_for_task = session.clone();
    let dispatch_task = tokio::spawn(async move {
        while let Some(chunk) = audio_rx.recv().await {
            if let Err(e) = session_for_task.send_audio_pcm16(&chunk).await {
                eprintln!("streaming send error: {}", e);
                break;
            }
        }
    });

    // Attach the chunk callback to the recorder BEFORE starting so chunks flow immediately.
    {
        let recorder = state.recorder.lock().map_err(|e| e.to_string())?;
        let tx_for_cb = audio_tx.clone();
        recorder.set_chunk_callback(move |chunk: &[f32]| {
            let _ = tx_for_cb.send(chunk.to_vec());
        });
    }

    // Start audio recording in a scoped block so the MutexGuard is not held across .await below.
    let start_err = {
        let mut recorder = state.recorder.lock().map_err(|e| e.to_string())?;
        recorder.start_recording().err()
    };
    if let Some(e) = start_err {
        {
            let recorder = state.recorder.lock().map_err(|e| e.to_string())?;
            recorder.clear_chunk_callback();
        }
        drop(audio_tx);
        dispatch_task.abort();
        let _ = session.stop().await;
        return Err(e);
    }

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
    // Stop audio recording (drops the chunk callback activity).
    {
        let mut recorder = state.recorder.lock().map_err(|e| e.to_string())?;
        recorder.clear_chunk_callback();
        let _ = recorder.stop_recording();
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

    let engine = whisper::WhisperEngine::new(&model_path)?;
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
    // Preserve existing API keys unless the caller explicitly sends a new non-empty value.
    let mut merged = new_settings;
    if merged.openai_api_key.as_ref().is_none_or(|k| k.is_empty()) {
        merged.openai_api_key = s.openai_api_key.clone();
    }
    if merged.deepgram_api_key.as_ref().is_none_or(|k| k.is_empty()) {
        merged.deepgram_api_key = s.deepgram_api_key.clone();
    }
    settings::save_settings(&merged)?;
    *s = merged;
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

/// Show the floating toolbar at the bottom-center of the active monitor
/// and hide the main window (remembered for restore on hide).
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
    let toolbar_enabled = {
        let s = state.settings.lock().map_err(|e| e.to_string())?;
        s.floating_toolbar_enabled
    };

    let should_hide_main = toolbar_enabled || streaming;
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

    if toolbar_enabled {
        // Bottom-center of the monitor containing main (fallback: primary).
        let monitor = main
            .as_ref()
            .and_then(|w| w.current_monitor().ok().flatten())
            .or_else(|| toolbar.primary_monitor().ok().flatten());

        if let Some(mon) = monitor {
            let scale = mon.scale_factor();
            let mon_size = mon.size();
            let mon_pos = mon.position();
            let logical_w = mon_size.width as f64 / scale;
            let logical_h = mon_size.height as f64 / scale;
            let logical_x = mon_pos.x as f64 / scale;
            let logical_y = mon_pos.y as f64 / scale;

            let toolbar_w = 360.0_f64;
            let toolbar_h = 56.0_f64;
            let x = logical_x + (logical_w - toolbar_w) / 2.0;
            let y = logical_y + logical_h - toolbar_h - 80.0;
            let _ = toolbar.set_position(tauri::LogicalPosition::new(x, y));
        }

        let _ = toolbar.set_always_on_top(true);
        let _ = toolbar.show();
    }

    if let Some(w) = &main {
        let _ = w.hide();
    }

    Ok(())
}

/// Hide the floating toolbar and restore the main window if it was visible
/// before the toolbar took over.
#[tauri::command]
fn hide_toolbar_window(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    if let Some(t) = app.get_webview_window("toolbar") {
        let _ = t.hide();
    }

    if state
        .main_was_visible_before_toolbar
        .swap(false, Ordering::Relaxed)
    {
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.show();
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

fn setup_global_shortcuts(app: &AppHandle) {
    let toggle_shortcut: Shortcut = "alt+d".parse().unwrap();

    let app_handle = app.clone();
    app.global_shortcut()
        .on_shortcut(toggle_shortcut, move |_app, shortcut, event| {
            if event.state == ShortcutState::Pressed {
                // Emit event without showing/focusing the window — keeps focus in the text field
                let _ = app_handle.emit("hotkey-pressed", shortcut.to_string());
            }
        })
        .unwrap_or_else(|e| {
            eprintln!("Failed to register shortcut: {}", e);
        });
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
        .manage(AppState {
            recorder: Mutex::new(AudioRecorder::new()),
            whisper_engine: Mutex::new(None),
            settings: Mutex::new(settings::load_settings()),
            streaming: tokio::sync::Mutex::new(None),
            main_was_visible_before_toolbar: AtomicBool::new(false),
        })
        .setup(|app| {
            setup_global_shortcuts(app.handle());
            let _ = setup_tray(app.handle());

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

            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_always_on_top(always_on_top_wanted);
                if start_minimized {
                    let _ = window.hide();
                }
                let w = window.clone();
                let notif_sent = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(close_notif_shown));
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        if !notif_sent.load(std::sync::atomic::Ordering::Relaxed) {
                            notif_sent.store(true, std::sync::atomic::Ordering::Relaxed);
                            let _ = w.emit("window-close-attempted", ());
                        }
                        let _ = w.hide();
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_recording,
            stop_recording,
            is_recording,
            check_silence,
            check_timeout,
            set_vad_enabled,
            set_max_recording_secs,
            transcribe,
            start_streaming_transcription,
            stop_streaming_transcription,
            mark_onboarding_complete,
            load_whisper_model,
            is_whisper_loaded,
            is_model_downloaded,
            download_model,
            delete_model,
            get_all_models_status,
            get_settings,
            update_settings,
            test_api_key,
            inject_text,
            get_audio_devices,
            set_window_always_on_top,
            set_autostart_enabled,
            show_toolbar_window,
            hide_toolbar_window,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
