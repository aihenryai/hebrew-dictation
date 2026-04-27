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
    #[serde(default)]
    pub deepgram_api_key: Option<String>,
    #[serde(default)]
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

pub fn load_settings() -> AppSettings {
    let path = get_settings_path();
    let mut settings = if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => AppSettings::default(),
        }
    } else {
        AppSettings::default()
    };

    // Auto-fill from environment variables if keys are not set
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

    settings
}

pub fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let dir = get_settings_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create settings dir: {}", e))?;
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    std::fs::write(get_settings_path(), json)
        .map_err(|e| format!("Failed to write settings: {}", e))?;
    Ok(())
}
