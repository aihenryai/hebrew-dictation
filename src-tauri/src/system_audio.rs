//! System-audio (WASAPI loopback) recorder — Windows only. Mirrors `AudioRecorder`:
//! captures the default *render* device via loopback, resamples the native rate
//! (48k/44.1k) down to 16kHz mono, and exposes start/stop. Spec §4.1.

use std::sync::{Arc, Mutex};

/// WASAPI loopback recorder for system output audio. Field set is intentionally
/// minimal here; the capture buffer / native-format / thread-handle fields are
/// added when start/stop land. Re-entrancy is guarded per-recorder, independent
/// of the mic's `AudioRecorder` (spec §4.1: separate `AppState.system_recorder`).
pub struct SystemAudioRecorder {
    is_recording: Arc<Mutex<bool>>,
}

impl SystemAudioRecorder {
    pub fn new() -> Self {
        Self {
            is_recording: Arc::new(Mutex::new(false)),
        }
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.lock().map(|r| *r).unwrap_or(false)
    }
}

#[cfg(test)]
mod system_audio_tests {
    use super::*;

    #[test]
    fn new_recorder_is_idle() {
        let rec = SystemAudioRecorder::new();
        assert!(!rec.is_recording());
    }
}
