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
    /// Mic + system captured together, interleaved to stereo for multichannel (cloud/Deepgram).
    CallCloud,
    /// Mic + system captured together, MIXED to one mono buffer and transcribed
    /// LOCALLY (whisper). No speaker separation. Privacy: audio never leaves the machine.
    CallLocal,
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

/// `CallLocal` ("פגישה מקומית") transcribes with the local whisper engine, so a
/// model must be downloaded. Fail fast — BEFORE recording — with a guiding message,
/// mirroring `ensure_call_deepgram_available`. NOTE: this checks the model *file* on
/// disk; the engine must also be loaded in memory (checked later at transcribe time),
/// so a narrow "downloaded but not loaded" window remains — the same pre-existing gap
/// as the Mic + local batch path, accepted for v1 (spec §4.6).
pub fn ensure_local_meeting_model_available(has_local_model: bool) -> Result<(), String> {
    if has_local_model {
        Ok(())
    } else {
        Err("פגישה מקומית דורשת מודל מקומי מורד. הורד אותו בהגדרות.".to_string())
    }
}

/// Which physical recorders a batch `source` drives, as `(uses_mic, uses_system)`.
/// Pure decision table (spec §3, §4.6) so the Mic/System/Call routing is unit-tested
/// and can't silently regress — `start_recorders_for_source` in lib.rs keys off it,
/// making this the single source of truth for "what does each source capture".
pub fn recorders_for_source(source: RecordingSource) -> (bool, bool) {
    match source {
        RecordingSource::Mic => (true, false),
        RecordingSource::System => (false, true),
        RecordingSource::CallCloud => (true, true),
        RecordingSource::CallLocal => (true, true),
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
        assert_eq!(from_str::<RecordingSource>("\"callcloud\"").unwrap(), RecordingSource::CallCloud);
        assert_eq!(from_str::<RecordingSource>("\"calllocal\"").unwrap(), RecordingSource::CallLocal);
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

    #[test]
    fn recorders_for_source_maps_each_variant() {
        // (uses_mic, uses_system) — the routing table the lib.rs start/stop wiring
        // keys off. Locked down so a Mic/System/Call mis-route fails HERE.
        assert_eq!(recorders_for_source(RecordingSource::Mic), (true, false));
        assert_eq!(recorders_for_source(RecordingSource::System), (false, true));
        // Both meeting sources drive BOTH recorders: CallCloud → stereo/multichannel,
        // CallLocal → mixed to mono.
        assert_eq!(recorders_for_source(RecordingSource::CallCloud), (true, true));
        assert_eq!(recorders_for_source(RecordingSource::CallLocal), (true, true));
    }

    #[test]
    fn local_meeting_requires_a_downloaded_model() {
        // Model present → proceed.
        assert!(ensure_local_meeting_model_available(true).is_ok());
        // No local model → a guiding Hebrew error, raised BEFORE recording starts.
        let err = ensure_local_meeting_model_available(false).unwrap_err();
        assert!(err.contains("מודל מקומי"));
    }
}
