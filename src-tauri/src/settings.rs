use serde::{Deserialize, Deserializer, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionMode {
    /// Use user's own API key (Deepgram / Groq).
    Api,
    /// Local whisper-rs — offline, full privacy.
    Local,
    /// Default — try API first, fall back to local if no key or connection fails.
    AutoFallback,
}

impl Default for TranscriptionMode {
    fn default() -> Self {
        Self::AutoFallback
    }
}

/// Saved screen position of the floating toolbar window. Lets the user drag the
/// toolbar to where they want it and have it stick across recording sessions.
/// Logical (DPI-independent) coordinates relative to the virtual screen origin.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ToolbarPosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApiProvider {
    Deepgram,
    Groq,
}

impl Default for ApiProvider {
    fn default() -> Self {
        Self::Deepgram
    }
}

/// Custom deserializer that maps unknown / legacy variants (e.g. "open_ai" from v2.3.x) to
/// the default Deepgram so older settings.json files don't fail to load after the OpenAI
/// provider was removed in v2.4.0.
impl<'de> Deserialize<'de> for ApiProvider {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "deepgram" => Ok(ApiProvider::Deepgram),
            "groq" => Ok(ApiProvider::Groq),
            _ => Ok(ApiProvider::default()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub transcription_mode: TranscriptionMode,
    #[serde(default)]
    pub api_provider: ApiProvider,
    /// Cache only — persisted in OS-secure storage (Credential Manager / Keychain), never in JSON.
    #[serde(skip)]
    pub deepgram_api_key: Option<String>,
    /// Cache only — persisted in OS-secure storage (Credential Manager / Keychain), never in JSON.
    #[serde(skip)]
    pub groq_api_key: Option<String>,
    #[serde(default = "default_preferred_model")]
    pub preferred_model: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_true")]
    pub vad_enabled: bool,
    #[serde(default)]
    pub onboarding_completed: bool,
    #[serde(default)]
    pub terms_accepted: bool,
    #[serde(default)]
    pub close_notification_shown: bool,
    #[serde(default = "default_true")]
    pub always_on_top: bool,
    #[serde(default = "default_true")]
    pub autostart_enabled: bool,
    #[serde(default)]
    pub streaming_enabled: bool,
    #[serde(default = "default_true")]
    pub floating_toolbar_enabled: bool,
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    /// Optional second global shortcut for Pause/Resume during active recording.
    /// `None` = feature disabled. Default `Some("alt+p")`.
    #[serde(default = "default_pause_hotkey")]
    pub pause_hotkey: Option<String>,
    #[serde(default = "default_silence_duration_secs")]
    pub vad_silence_secs: f32,
    #[serde(default = "default_max_recording_secs")]
    pub max_recording_secs: f32,
    #[serde(default)]
    pub unlimited_recording: bool,
    #[serde(default)]
    pub preferred_audio_device: Option<String>,
    /// Last position the user dragged the floating toolbar to. `None` =
    /// fall back to the default bottom-center placement on the active monitor.
    #[serde(default)]
    pub toolbar_position: Option<ToolbarPosition>,
    /// Play a short tone when recording starts / stops. Default true.
    /// Helps users know the mic actually opened — especially when the user is
    /// in another app and can't see the floating toolbar appear.
    #[serde(default = "default_true")]
    pub audio_feedback_enabled: bool,
    /// Show a small always-floating circular button when the main window is
    /// hidden (autostart / closed-to-tray). One click starts dictation — the
    /// main discoverability fix for non-technical users who don't know the app
    /// is running. Default false (opt-in via wizard / settings).
    #[serde(default)]
    pub idle_button_enabled: bool,
    /// Loudness of the audio-feedback tones, 0.0–1.0. Default 0.6.
    #[serde(default = "default_audio_volume")]
    pub audio_volume: f32,
    /// Opt-in smart cleanup of the transcript via Groq Llama before injection.
    /// Default false — preserves existing behavior + privacy.
    #[serde(default)]
    pub enhance_enabled: bool,
    /// Cleanup profile. Default "he_general". Unknown values fall back via EnhanceMode::from_str.
    #[serde(default = "default_enhance_mode")]
    pub enhance_mode: String,
}

/// Settings sent to the webview — API keys are redacted to booleans.
#[derive(Debug, Clone, Serialize)]
pub struct RedactedSettings {
    pub transcription_mode: TranscriptionMode,
    pub api_provider: ApiProvider,
    pub has_deepgram_key: bool,
    pub has_groq_key: bool,
    pub preferred_model: String,
    pub language: String,
    pub vad_enabled: bool,
    pub onboarding_completed: bool,
    pub terms_accepted: bool,
    pub close_notification_shown: bool,
    pub always_on_top: bool,
    pub autostart_enabled: bool,
    pub streaming_enabled: bool,
    pub floating_toolbar_enabled: bool,
    pub hotkey: String,
    pub pause_hotkey: Option<String>,
    pub vad_silence_secs: f32,
    pub max_recording_secs: f32,
    pub unlimited_recording: bool,
    pub preferred_audio_device: Option<String>,
    pub toolbar_position: Option<ToolbarPosition>,
    pub audio_feedback_enabled: bool,
    pub idle_button_enabled: bool,
    pub audio_volume: f32,
    pub enhance_enabled: bool,
    pub enhance_mode: String,
}

impl AppSettings {
    pub fn redacted(&self) -> RedactedSettings {
        RedactedSettings {
            transcription_mode: self.transcription_mode.clone(),
            api_provider: self.api_provider.clone(),
            has_deepgram_key: self.deepgram_api_key.as_ref().is_some_and(|k| !k.is_empty()),
            has_groq_key: self.groq_api_key.as_ref().is_some_and(|k| !k.is_empty()),
            preferred_model: self.preferred_model.clone(),
            language: self.language.clone(),
            vad_enabled: self.vad_enabled,
            onboarding_completed: self.onboarding_completed,
            terms_accepted: self.terms_accepted,
            close_notification_shown: self.close_notification_shown,
            always_on_top: self.always_on_top,
            autostart_enabled: self.autostart_enabled,
            streaming_enabled: self.streaming_enabled,
            floating_toolbar_enabled: self.floating_toolbar_enabled,
            hotkey: self.hotkey.clone(),
            pause_hotkey: self.pause_hotkey.clone(),
            vad_silence_secs: self.vad_silence_secs,
            max_recording_secs: self.max_recording_secs,
            unlimited_recording: self.unlimited_recording,
            preferred_audio_device: self.preferred_audio_device.clone(),
            toolbar_position: self.toolbar_position,
            audio_feedback_enabled: self.audio_feedback_enabled,
            idle_button_enabled: self.idle_button_enabled,
            audio_volume: self.audio_volume,
            enhance_enabled: self.enhance_enabled,
            enhance_mode: self.enhance_mode.clone(),
        }
    }
}

fn default_preferred_model() -> String {
    "small".to_string()
}

fn default_language() -> String {
    "he".to_string()
}

fn default_true() -> bool {
    true
}

fn default_hotkey() -> String {
    "alt+d".to_string()
}

fn default_pause_hotkey() -> Option<String> {
    Some("alt+p".to_string())
}

fn default_silence_duration_secs() -> f32 {
    4.5
}

fn default_max_recording_secs() -> f32 {
    60.0
}

fn default_audio_volume() -> f32 {
    0.6
}

fn default_enhance_mode() -> String {
    "he_general".to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            transcription_mode: TranscriptionMode::default(),
            api_provider: ApiProvider::default(),
            deepgram_api_key: None,
            groq_api_key: None,
            preferred_model: default_preferred_model(),
            language: default_language(),
            vad_enabled: true,
            onboarding_completed: false,
            terms_accepted: false,
            close_notification_shown: false,
            always_on_top: true,
            autostart_enabled: true,
            streaming_enabled: true,
            floating_toolbar_enabled: true,
            hotkey: default_hotkey(),
            pause_hotkey: default_pause_hotkey(),
            vad_silence_secs: default_silence_duration_secs(),
            max_recording_secs: default_max_recording_secs(),
            unlimited_recording: false,
            preferred_audio_device: None,
            toolbar_position: None,
            audio_feedback_enabled: true,
            idle_button_enabled: false,
            audio_volume: default_audio_volume(),
            enhance_enabled: false,
            enhance_mode: default_enhance_mode(),
        }
    }
}

impl AppSettings {
    pub fn active_api_key(&self) -> Option<&str> {
        match self.api_provider {
            ApiProvider::Deepgram => self.deepgram_api_key.as_deref(),
            ApiProvider::Groq => self.groq_api_key.as_deref(),
        }
    }
}

fn get_settings_dir() -> PathBuf {
    let app_data = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    app_data.join("hebrew-dictation")
}

fn get_settings_path() -> PathBuf {
    get_settings_dir().join("settings.json")
}

/// Result of the one-shot key migration that runs on every app launch.
/// `Migrated` and `Failed` carry information for the frontend to surface.
#[derive(Debug, Clone)]
pub enum MigrationOutcome {
    /// No legacy keys were found — nothing to migrate.
    NoOp,
    /// Legacy keys were found in JSON, successfully copied to keyring, JSON cleaned.
    Migrated { providers: Vec<&'static str> },
    /// Legacy keys were found but writing them to keyring failed. JSON is left intact
    /// so the user can keep working — a banner will prompt them to re-enter the keys.
    Failed { error: String },
}

pub struct LoadResult {
    pub settings: AppSettings,
    pub migration: MigrationOutcome,
}

pub fn load_settings() -> LoadResult {
    let path = get_settings_path();

    // Read the raw JSON value so we can detect legacy plaintext keys that are no
    // longer mapped on AppSettings (the fields are now #[serde(skip)]).
    let raw_json: Option<serde_json::Value> = if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    } else {
        None
    };

    let mut settings: AppSettings = raw_json
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    // Backward-compat: if onboarding was completed in a previous version, the
    // user already accepted the terms once via the wizard (or skipped them in
    // a pre-2.4.0 install but is now grandfathered). Don't keep prompting on
    // every upgrade — auto-mark as accepted.
    if settings.onboarding_completed && !settings.terms_accepted {
        settings.terms_accepted = true;
    }

    // Backward-compat: "auto" was an old language option that doesn't map to a
    // valid Deepgram/Groq value. Migrate to "he" (Hebrew default).
    if settings.language == "auto" {
        settings.language = "he".to_string();
    }

    // 1) Load existing keys from keyring (works on fresh installs and post-migration).
    //    Errors are logged (the keyring backend may be unavailable on locked-down
    //    systems / antivirus blocking DPAPI) so we have a trail when users report
    //    "key disappeared between launches".
    match crate::secure_keys::load_key("deepgram") {
        Ok(Some(k)) if !k.is_empty() => settings.deepgram_api_key = Some(k),
        Ok(_) => {}
        Err(e) => eprintln!("[settings] keyring read failed for deepgram: {}", e),
    }
    match crate::secure_keys::load_key("groq") {
        Ok(Some(k)) if !k.is_empty() => settings.groq_api_key = Some(k),
        Ok(_) => {}
        Err(e) => eprintln!("[settings] keyring read failed for groq: {}", e),
    }

    // 2) Detect legacy keys present in JSON (only present on pre-2.6.0 installs).
    let legacy_deepgram = raw_json
        .as_ref()
        .and_then(|v| v.get("deepgram_api_key"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let legacy_groq = raw_json
        .as_ref()
        .and_then(|v| v.get("groq_api_key"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let migration = if legacy_deepgram.is_some() || legacy_groq.is_some() {
        let mut migrated_providers: Vec<&'static str> = Vec::new();
        let mut migration_error: Option<String> = None;

        if let Some(k) = &legacy_deepgram {
            // If the key is already in keyring, don't overwrite — keyring is the truth.
            if settings.deepgram_api_key.is_none() {
                if let Err(e) = crate::secure_keys::save_key("deepgram", k) {
                    migration_error = Some(e);
                } else {
                    settings.deepgram_api_key = Some(k.clone());
                    migrated_providers.push("deepgram");
                }
            }
        }
        if migration_error.is_none() {
            if let Some(k) = &legacy_groq {
                if settings.groq_api_key.is_none() {
                    if let Err(e) = crate::secure_keys::save_key("groq", k) {
                        migration_error = Some(e);
                    } else {
                        settings.groq_api_key = Some(k.clone());
                        migrated_providers.push("groq");
                    }
                }
            }
        }

        if let Some(err) = migration_error {
            // Don't touch the JSON — the user still has working keys there.
            MigrationOutcome::Failed { error: err }
        } else {
            // Rewrite settings.json so the legacy fields disappear (#[serde(skip)] handles it).
            if let Err(e) = save_settings(&settings) {
                eprintln!("warning: failed to rewrite settings.json after migration: {}", e);
            }
            MigrationOutcome::Migrated { providers: migrated_providers }
        }
    } else {
        MigrationOutcome::NoOp
    };

    // 3) Env var fallback — runtime override only, never written to keyring.
    if settings.deepgram_api_key.is_none() {
        if let Ok(key) = std::env::var("DEEPGRAM_API_KEY") {
            if !key.is_empty() {
                settings.deepgram_api_key = Some(key);
            }
        }
    }
    if settings.groq_api_key.is_none() {
        if let Ok(key) = std::env::var("GROQ_API_KEY") {
            if !key.is_empty() {
                settings.groq_api_key = Some(key);
            }
        }
    }

    LoadResult { settings, migration }
}

pub fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let dir = get_settings_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create settings dir: {}", e))?;
    // #[serde(skip)] on the key fields ensures they never reach the file.
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    std::fs::write(get_settings_path(), json)
        .map_err(|e| format!("Failed to write settings: {}", e))?;
    Ok(())
}
