//! System-audio (WASAPI loopback) recorder — Windows only. Mirrors `AudioRecorder`:
//! captures the default *render* device via loopback, resamples the native rate
//! (48k/44.1k) down to 16kHz mono, and exposes start/stop. Spec §4.1.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use wasapi::{initialize_mta, DeviceEnumerator, Direction, StreamMode};

pub struct SystemAudioRecorder {
    samples: Arc<Mutex<Vec<f32>>>,
    is_recording: Arc<Mutex<bool>>,
    /// Native capture format, published by the capture thread; read by stop().
    native_rate: Arc<Mutex<u32>>,
    native_channels: Arc<Mutex<u16>>,
    /// Owns the WASAPI objects (not `Send`) — joined on stop.
    capture_thread: Option<std::thread::JoinHandle<()>>,
}

impl SystemAudioRecorder {
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            is_recording: Arc::new(Mutex::new(false)),
            native_rate: Arc::new(Mutex::new(48000)),
            native_channels: Arc::new(Mutex::new(2)),
            capture_thread: None,
        }
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.lock().map(|r| *r).unwrap_or(false)
    }

    /// Start WASAPI loopback capture of the default render device. Returns an error
    /// (mode unavailable — spec §6) if the device can't be bound; the mic is unaffected.
    /// Method name mirrors `AudioRecorder::start_recording` (spec §4.1 shorthand `start()`).
    pub fn start_recording(&mut self) -> Result<(), String> {
        // Re-entrancy backstop — mirror AudioRecorder (audio.rs:135). Per-recorder.
        if self.is_recording() {
            return Err("הקלטה כבר פעילה — עצור אותה לפני התחלת הקלטה חדשה".to_string());
        }

        {
            let mut buf = self.samples.lock().map_err(|e| e.to_string())?;
            buf.clear();
        }
        {
            let mut rec = self.is_recording.lock().map_err(|e| e.to_string())?;
            *rec = true;
        }

        let samples = self.samples.clone();
        let is_recording = self.is_recording.clone();
        let native_rate = self.native_rate.clone();
        let native_channels = self.native_channels.clone();
        // Surface loopback bind failures back to the caller before returning.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        let handle = std::thread::spawn(move || {
            // WASAPI/COM objects are not Send — create and own them entirely here.
            // initialize_mta() returns an HRESULT; `.ok()` → windows Result, then is_err().
            if initialize_mta().ok().is_err() {
                let _ = ready_tx.send(Err("COM init (MTA) failed".to_string()));
                return;
            }
            // Loopback source = the default RENDER (playback) device. In wasapi 0.23
            // `get_default_device` is a method on `DeviceEnumerator`, not a free fn.
            let device = match DeviceEnumerator::new().and_then(|e| e.get_default_device(&Direction::Render)) {
                Ok(d) => d,
                Err(e) => { let _ = ready_tx.send(Err(format!("No default render device: {e}"))); return; }
            };
            let mut audio_client = match device.get_iaudioclient() {
                Ok(c) => c,
                Err(e) => { let _ = ready_tx.send(Err(format!("get_iaudioclient failed: {e}"))); return; }
            };
            // Native shared-mode mix format — 32-bit float, 48k/44.1k, usually stereo.
            let format = match audio_client.get_mixformat() {
                Ok(f) => f,
                Err(e) => { let _ = ready_tx.send(Err(format!("get_mixformat failed: {e}"))); return; }
            };
            let rate = format.get_samplespersec();
            let channels = format.get_nchannels();
            // get_device_period replaces the deprecated get_periods; (default, min) in hns.
            let (default_period, _min_period) = match audio_client.get_device_period() {
                Ok(p) => p,
                Err(e) => { let _ = ready_tx.send(Err(format!("get_device_period failed: {e}"))); return; }
            };
            // ...initialized for CAPTURE. In wasapi 0.23 the Render-device + Direction::Capture
            // mismatch IS the loopback selector — no ShareMode/period/bool args anymore.
            // Shared, event-driven, autoconvert:true so `format` is honored as-is.
            let mode = StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: default_period,
            };
            if let Err(e) = audio_client.initialize_client(&format, &Direction::Capture, &mode) {
                let _ = ready_tx.send(Err(format!("loopback initialize_client failed: {e}")));
                return;
            }
            let h_event = match audio_client.set_get_eventhandle() {
                Ok(h) => h,
                Err(e) => { let _ = ready_tx.send(Err(format!("set_get_eventhandle failed: {e}"))); return; }
            };
            // `read_from_device_to_deque` takes `&self` → non-mut binding is fine.
            let capture_client = match audio_client.get_audiocaptureclient() {
                Ok(c) => c,
                Err(e) => { let _ = ready_tx.send(Err(format!("get_audiocaptureclient failed: {e}"))); return; }
            };

            // Publish the native format so stop() resamples correctly.
            if let Ok(mut r) = native_rate.lock() { *r = rate; }
            if let Ok(mut c) = native_channels.lock() { *c = channels; }

            if let Err(e) = audio_client.start_stream() {
                let _ = ready_tx.send(Err(format!("start_stream failed: {e}")));
                return;
            }
            // Bind succeeded — unblock the caller.
            let _ = ready_tx.send(Ok(()));

            let mut raw: VecDeque<u8> = VecDeque::new();
            loop {
                if is_recording.lock().map(|r| !*r).unwrap_or(true) {
                    break;
                }
                // Wake on the next buffer or re-check the stop flag within ~100ms.
                let _ = h_event.wait_for_event(100);
                // read_from_device_to_deque -> Result<BufferInfo, _>; .is_err() still applies.
                if capture_client.read_from_device_to_deque(&mut raw).is_err() {
                    break;
                }
                if !raw.is_empty() {
                    let bytes: Vec<u8> = raw.drain(..).collect();
                    if let Ok(mut buf) = samples.lock() {
                        // Shared-mode mix format is 32-bit IEEE float, interleaved.
                        for frame in bytes.chunks_exact(4) {
                            buf.push(f32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]));
                        }
                    }
                }
            }
            let _ = audio_client.stop_stream();
        });

        // Wait for the capture thread to confirm the loopback bind succeeded.
        let ready = ready_rx
            .recv()
            .unwrap_or_else(|_| Err("Capture thread exited before signaling".to_string()));
        if let Err(e) = ready {
            // Loopback bind failed (spec §6): reset is_recording so the recorder stays
            // RECOVERABLE — otherwise the re-entrancy guard rejects every retry until an
            // unpaired stop. The mic (separate AudioRecorder) is unaffected.
            if let Ok(mut rec) = self.is_recording.lock() {
                *rec = false;
            }
            let _ = handle.join();
            return Err(e);
        }

        self.capture_thread = Some(handle);
        Ok(())
    }

    /// Stop capture and return the buffer as 16kHz mono f32 — mirrors
    /// `AudioRecorder::stop_recording` (audio.rs:470-504); spec §4.1 shorthand `stop()`.
    pub fn stop_recording(&mut self) -> Result<Vec<f32>, String> {
        {
            let mut rec = self.is_recording.lock().map_err(|e| e.to_string())?;
            *rec = false;
        }
        if let Some(handle) = self.capture_thread.take() {
            let _ = handle.join();
        }
        let raw = self.samples.lock().map_err(|e| e.to_string())?.clone();
        let rate = *self.native_rate.lock().map_err(|e| e.to_string())?;
        let channels = *self.native_channels.lock().map_err(|e| e.to_string())?;
        Ok(resample_to_16k_mono(&raw, rate, channels))
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

    #[test]
    #[ignore = "MANUAL-VERIFY on Windows: play audio on the default render device, then run with --ignored"]
    fn loopback_captures_playing_audio() {
        let mut rec = SystemAudioRecorder::new();
        rec.start_recording()
            .expect("loopback should bind the default render device");
        std::thread::sleep(std::time::Duration::from_secs(3));
        let samples = rec
            .stop_recording()
            .expect("stop should return 16kHz mono samples");
        // ~3s of 16kHz mono ≈ 48000 samples; assert a non-trivial (>1s) capture.
        assert!(samples.len() > 16000, "expected >1s of audio, got {}", samples.len());
        assert!(!rec.is_recording());
    }
}
