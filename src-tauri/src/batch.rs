//! Pure, testable batch-transcription routing + options. Orchestration (decode,
//! cloud/local dispatch, progress, cancel) lives in lib.rs where AppState is reachable.

use serde::Deserialize;

/// Options sent from the frontend for a batch transcription.
#[derive(Debug, Clone, Deserialize)]
pub struct BatchOpts {
    /// "cloud" | "local". Defaults from the user's transcription mode on the UI side.
    pub mode: String,
    #[serde(default = "default_language")]
    pub language: String,
    /// Reserved for a future "inject on completion" toggle; the UI handles inject in Phase 1.
    #[serde(default)]
    #[allow(dead_code)]
    pub inject: bool,
}

/// Which audio source a batch recording captures. This is a DIFFERENT axis from
/// `BatchOpts.mode` (cloud/local): `mode` picks the transcription engine, `source`
/// picks what is recorded. Named `source` precisely so it never collides with
/// `mode`. Defaults to `Mic` (existing behavior, zero regression when the frontend
/// omits it). Spec §3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingSource {
    /// Existing cpal microphone path (mono).
    #[default]
    Mic,
    /// WASAPI loopback of the default render device (Windows-only).
    System,
    /// Mic + system captured together, interleaved to stereo for multichannel.
    Call,
}

fn default_language() -> String {
    "he".to_string()
}

/// Sentinel error string for user cancellation. The frontend shows it as a calm
/// notice, NOT an error toast (mirrors export_history's "הייצוא בוטל").
pub const CANCELLED: &str = "בוטל";

/// Phase 1 routing: cloud → Deepgram single request; local → whisper.
/// (Groq cloud + chunking is Phase 2.)
#[derive(Debug, PartialEq, Eq)]
pub enum BatchRoute {
    CloudDeepgram,
    Local,
}

pub fn pick_batch_route(mode: &str) -> BatchRoute {
    match mode {
        "local" => BatchRoute::Local,
        _ => BatchRoute::CloudDeepgram,
    }
}

/// Call mode is Deepgram-only (multichannel), so it needs a Deepgram key even when
/// `BatchOpts.mode="local"`: with a key present Call transparently forces cloud;
/// with no key at all we fail fast — BEFORE recording — with a guiding message,
/// rather than capturing audio that can't be transcribed. Spec §6.
pub fn ensure_call_deepgram_available(has_deepgram_key: bool) -> Result<(), String> {
    if has_deepgram_key {
        Ok(())
    } else {
        Err("מצב שיחה דורש מפתח Deepgram. הוסף אותו בהגדרות.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_local_and_cloud() {
        assert_eq!(pick_batch_route("local"), BatchRoute::Local);
        assert_eq!(pick_batch_route("cloud"), BatchRoute::CloudDeepgram);
        // Unknown/empty mode defaults to cloud.
        assert_eq!(pick_batch_route("whatever"), BatchRoute::CloudDeepgram);
    }

    #[test]
    fn recording_source_deserializes_and_defaults_to_mic() {
        use serde_json::from_str;
        // Frontend sends lowercase strings for the source toggle.
        assert_eq!(from_str::<RecordingSource>("\"mic\"").unwrap(), RecordingSource::Mic);
        assert_eq!(from_str::<RecordingSource>("\"system\"").unwrap(), RecordingSource::System);
        assert_eq!(from_str::<RecordingSource>("\"call\"").unwrap(), RecordingSource::Call);
        // Zero-regression default: an absent/legacy `source` must fall back to Mic.
        assert_eq!(RecordingSource::default(), RecordingSource::Mic);
    }

    #[test]
    fn call_requires_a_deepgram_key() {
        // Key present → Call proceeds (via cloud) EVEN when BatchOpts.mode="local".
        assert!(ensure_call_deepgram_available(true).is_ok());
        // No key at all → a guiding Hebrew error, raised BEFORE recording starts.
        let err = ensure_call_deepgram_available(false).unwrap_err();
        assert!(err.contains("Deepgram"));
        assert!(err.contains("שיחה"));
    }
}
