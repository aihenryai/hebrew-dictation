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

/// Down-mix to mono and resample to 16kHz — identical to the mic's stop-path tail
/// (audio.rs:496-503), reusing the SAME linear-interpolation `resample` the mic
/// uses (spec §4.1: "choose one; NOT rubato"). Pure — unit-testable without audio.
pub(crate) fn resample_to_16k_mono(raw: &[f32], native_rate: u32, native_channels: u16) -> Vec<f32> {
    let mono = crate::audio::to_mono(raw, native_channels);
    if native_rate == 16000 {
        mono
    } else {
        crate::audio::resample(&mono, native_rate, 16000)
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

    #[test]
    fn resample_to_16k_mono_downmixes_and_downsamples() {
        // Stereo (2ch) @32kHz, L=+1.0 / R=-1.0 → per-frame average 0.0.
        // 8 interleaved samples = 4 frames → 4 mono @32k → 2 samples @16k (ratio 2.0).
        let stereo_32k = vec![1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
        let out = resample_to_16k_mono(&stereo_32k, 32000, 2);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|s| s.abs() < 1e-6));
    }

    #[test]
    fn resample_to_16k_mono_passthrough_when_already_16k_mono() {
        let mono_16k = vec![0.1, 0.2, 0.3, 0.4];
        let out = resample_to_16k_mono(&mono_16k, 16000, 1);
        assert_eq!(out, mono_16k);
    }
}
