use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// VAD (Voice Activity Detection) configuration
/// Threshold tuned for multi-channel mics that interleave mostly-silent channels.
/// 4-channel mics dilute per-frame RMS by √4 compared to mono, so we set this low.
const SILENCE_THRESHOLD: f32 = 0.002;
/// Default silence-to-stop duration. Configurable from Settings via `set_silence_duration_secs`.
const DEFAULT_SILENCE_DURATION_SECS: f32 = 4.5;
const MIN_SPEECH_DURATION_SECS: f32 = 0.5;
const VAD_CHECK_INTERVAL_MS: u64 = 100;

/// Default maximum recording duration in seconds. Configurable from Settings via
/// `set_max_recording_secs`. Effective ceiling is enforced in the setter.
const DEFAULT_MAX_RECORDING_SECS: f32 = 60.0;
/// Hard ceiling for any recording, even when the user picks "unlimited" — protects
/// RAM and keeps API costs bounded. 1 hour is more than any reasonable dictation.
const MAX_RECORDING_CEILING_SECS: f32 = 3600.0;

/// Callback invoked from the CPAL audio thread with 16kHz mono f32 samples.
/// Used by streaming mode to push chunks to a WebSocket writer.
pub type ChunkCallback = Arc<dyn Fn(&[f32]) + Send + Sync + 'static>;

pub struct AudioRecorder {
    samples: Arc<Mutex<Vec<f32>>>,
    is_recording: Arc<Mutex<bool>>,
    device_sample_rate: Arc<Mutex<u32>>,
    device_channels: Arc<Mutex<u16>>,
    /// Signal the stream thread to stop
    stream_stop_tx: Option<std::sync::mpsc::Sender<()>>,
    /// Handle to the stream-owning thread
    stream_thread: Option<std::thread::JoinHandle<()>>,
    /// Signals that silence was detected (for auto-stop)
    pub silence_detected: Arc<Mutex<bool>>,
    /// Signals that max recording time was reached
    pub timeout_reached: Arc<Mutex<bool>>,
    /// Whether VAD auto-stop is enabled
    vad_enabled: Arc<Mutex<bool>>,
    /// Handle to the VAD monitoring thread
    vad_thread: Option<std::thread::JoinHandle<()>>,
    /// Max recording duration (configurable per mode)
    max_recording_secs: Arc<Mutex<f32>>,
    /// Silence duration (in seconds) before VAD auto-stop fires.
    silence_duration_secs: Arc<Mutex<f32>>,
    /// Preferred input device name (None = system default).
    preferred_device: Arc<Mutex<Option<String>>>,
    /// When true, the recorder discards incoming audio (samples are not appended,
    /// and the streaming callback is not invoked). VAD timeout/silence checks also
    /// pause so the user can take an indefinite break without losing the buffer.
    is_paused: Arc<Mutex<bool>>,
    /// Optional callback — when set, each audio chunk is also forwarded (16kHz mono) to the callback.
    chunk_callback: Arc<Mutex<Option<ChunkCallback>>>,
    /// AppHandle used by the VAD monitor thread to emit `audio-level` and `vad-state`
    /// events to the floating toolbar. Set once at app setup; `None` in tests / standalone.
    app_handle: Arc<Mutex<Option<AppHandle>>>,
}

impl AudioRecorder {
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            is_recording: Arc::new(Mutex::new(false)),
            device_sample_rate: Arc::new(Mutex::new(16000)),
            device_channels: Arc::new(Mutex::new(1)),
            stream_stop_tx: None,
            stream_thread: None,
            silence_detected: Arc::new(Mutex::new(false)),
            timeout_reached: Arc::new(Mutex::new(false)),
            vad_enabled: Arc::new(Mutex::new(true)),
            vad_thread: None,
            max_recording_secs: Arc::new(Mutex::new(DEFAULT_MAX_RECORDING_SECS)),
            silence_duration_secs: Arc::new(Mutex::new(DEFAULT_SILENCE_DURATION_SECS)),
            preferred_device: Arc::new(Mutex::new(None)),
            is_paused: Arc::new(Mutex::new(false)),
            chunk_callback: Arc::new(Mutex::new(None)),
            app_handle: Arc::new(Mutex::new(None)),
        }
    }

    /// Wire an AppHandle so the VAD monitor thread can emit `audio-level`
    /// and `vad-state` events. Called once during app setup.
    pub fn set_app_handle(&self, handle: AppHandle) {
        if let Ok(mut g) = self.app_handle.lock() {
            *g = Some(handle);
        }
    }

    /// Set a callback that receives each incoming audio chunk, converted to 16kHz mono f32.
    /// Invoked from the CPAL audio thread — keep the callback non-blocking.
    pub fn set_chunk_callback<F>(&self, cb: F)
    where
        F: Fn(&[f32]) + Send + Sync + 'static,
    {
        if let Ok(mut guard) = self.chunk_callback.lock() {
            *guard = Some(Arc::new(cb));
        }
    }

    pub fn clear_chunk_callback(&self) {
        if let Ok(mut guard) = self.chunk_callback.lock() {
            *guard = None;
        }
    }

    pub fn set_max_recording_secs(&self, secs: f32) {
        if let Ok(mut max) = self.max_recording_secs.lock() {
            *max = secs.clamp(1.0, MAX_RECORDING_CEILING_SECS);
        }
    }

    /// Configure the silence-to-stop duration in seconds. Clamped to [0.5, 30].
    pub fn set_silence_duration_secs(&self, secs: f32) {
        if let Ok(mut s) = self.silence_duration_secs.lock() {
            *s = secs.clamp(0.5, 30.0);
        }
    }

    /// Choose a preferred input device by name. Pass `None` to fall back to the
    /// system default. The change applies to the next recording — does not interrupt
    /// an active stream.
    pub fn set_preferred_device(&self, device: Option<String>) {
        if let Ok(mut d) = self.preferred_device.lock() {
            *d = device.filter(|s| !s.is_empty());
        }
    }

    pub fn start_recording(&mut self) -> Result<(), String> {
        // Re-entrancy backstop (root cause of C1/H1/H3): never start over a live
        // stream. A second start would `samples.clear()` the in-flight buffer and
        // spawn a duplicate VAD thread (the old one never sees is_recording=false,
        // so it never exits). The command-layer guards should prevent reaching here;
        // this enforces it at the source.
        if self.is_recording() {
            return Err("הקלטה כבר פעילה — עצור אותה לפני התחלת הקלטה חדשה".to_string());
        }

        // Query device info on the main thread (this is safe)
        let host = cpal::default_host();
        let preferred = self
            .preferred_device
            .lock()
            .ok()
            .and_then(|g| g.clone());
        let device = match preferred {
            Some(name) => host
                .input_devices()
                .ok()
                .and_then(|mut devs| {
                    devs.find(|d| d.name().ok().as_deref() == Some(name.as_str()))
                })
                .or_else(|| host.default_input_device())
                .ok_or("No input device available")?,
            None => host
                .default_input_device()
                .ok_or("No input device available")?,
        };

        let default_config = device
            .default_input_config()
            .map_err(|e| format!("No default input config: {}", e))?;

        let native_rate = default_config.sample_rate().0;
        let native_channels = default_config.channels();

        {
            let mut rate = self.device_sample_rate.lock().map_err(|e| e.to_string())?;
            *rate = native_rate;
            let mut ch = self.device_channels.lock().map_err(|e| e.to_string())?;
            *ch = native_channels;
        }

        {
            let mut samples = self.samples.lock().map_err(|e| e.to_string())?;
            samples.clear();
        }
        {
            let mut silence = self.silence_detected.lock().map_err(|e| e.to_string())?;
            *silence = false;
        }
        {
            let mut timeout = self.timeout_reached.lock().map_err(|e| e.to_string())?;
            *timeout = false;
        }

        {
            let mut recording = self.is_recording.lock().map_err(|e| e.to_string())?;
            *recording = true;
        }

        // Create and own the stream in a dedicated thread (avoids unsafe Send)
        let samples_clone = self.samples.clone();
        let is_recording_clone = self.is_recording.clone();
        let is_paused_clone = self.is_paused.clone();
        let chunk_callback_clone = self.chunk_callback.clone();
        // Capture the chosen device name so the stream-owner thread picks the same one.
        // `Device` itself isn't `Send`, so we re-resolve by name inside the thread.
        let chosen_device_name = device.name().ok();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        let handle = std::thread::spawn(move || {
            let host = cpal::default_host();
            let device = match chosen_device_name.as_ref() {
                Some(name) => host
                    .input_devices()
                    .ok()
                    .and_then(|mut devs| {
                        devs.find(|d| d.name().ok().as_deref() == Some(name.as_str()))
                    })
                    .or_else(|| host.default_input_device()),
                None => host.default_input_device(),
            };
            let device = match device {
                Some(d) => d,
                None => {
                    let _ = ready_tx.send(Err("No input device".to_string()));
                    return;
                }
            };

            let config = cpal::StreamConfig {
                channels: native_channels,
                sample_rate: cpal::SampleRate(native_rate),
                buffer_size: cpal::BufferSize::Default,
            };

            let samples = samples_clone;
            let is_recording = is_recording_clone;
            let is_paused = is_paused_clone;
            let chunk_callback = chunk_callback_clone;

            let stream = match device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let is_rec = is_recording.lock().map(|r| *r).unwrap_or(false);
                    let paused = is_paused.lock().map(|p| *p).unwrap_or(false);

                    // While paused, drop the audio — neither buffer nor stream callback.
                    // VAD also skips pause time so the user can resume after a break.
                    if paused {
                        return;
                    }

                    // Local-mode samples rely on is_recording for trimming the tail.
                    if is_rec {
                        if let Ok(mut s) = samples.lock() {
                            s.extend_from_slice(data);
                        }
                    }

                    // Streaming-mode: deliver tail audio even after is_recording=false so the
                    // last ~10-30ms WASAPI buffer reaches Deepgram before the stream is dropped.
                    let cb_opt = chunk_callback.lock().ok().and_then(|g| g.clone());
                    if let Some(cb) = cb_opt {
                        let mono = to_mono(data, native_channels);
                        let resampled = if native_rate == 16000 {
                            mono
                        } else {
                            resample(&mono, native_rate, 16000)
                        };
                        cb(&resampled);
                    }
                },
                |err| {
                    eprintln!("Audio stream error: {}", err);
                },
                None,
            ) {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("Failed to build input stream: {}", e)));
                    return;
                }
            };

            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(format!("Failed to start stream: {}", e)));
                return;
            }

            // Signal that the stream is ready
            let _ = ready_tx.send(Ok(()));

            // Block until told to stop — stream is dropped when this thread exits
            let _ = rx.recv();
            drop(stream);
        });

        // Wait for the stream thread to confirm it started
        let result = ready_rx
            .recv()
            .map_err(|_| "Stream thread exited before signaling".to_string())?;
        result?;

        self.stream_stop_tx = Some(tx);
        self.stream_thread = Some(handle);

        self.start_vad_monitor(native_rate, native_channels);

        Ok(())
    }

    fn start_vad_monitor(&mut self, sample_rate: u32, channels: u16) {
        let samples = self.samples.clone();
        let is_recording = self.is_recording.clone();
        let silence_detected = self.silence_detected.clone();
        let timeout_reached = self.timeout_reached.clone();
        let vad_enabled = self.vad_enabled.clone();
        let max_recording_secs = self.max_recording_secs.clone();
        let silence_duration_secs = self.silence_duration_secs.clone();
        let is_paused = self.is_paused.clone();
        let app_handle = self
            .app_handle
            .lock()
            .ok()
            .and_then(|g| g.clone());

        let samples_per_check =
            (sample_rate as f32 * (VAD_CHECK_INTERVAL_MS as f32 / 1000.0)) as usize
                * channels as usize;
        let min_speech_samples =
            (MIN_SPEECH_DURATION_SECS * sample_rate as f32) as usize * channels as usize;

        let handle = std::thread::spawn(move || {
            let mut silence_start: Option<Instant> = None;
            let mut had_speech = false;
            // Throttle UI emits — VAD wakes every 100ms but we don't need to spam
            // the webview that often. ~10 emits/sec is plenty for a smooth bar.
            let mut last_level_emit = Instant::now();
            let mut last_vad_state: Option<&'static str> = None;
            let mut last_vad_emit = Instant::now();

            loop {
                std::thread::sleep(std::time::Duration::from_millis(VAD_CHECK_INTERVAL_MS));

                if let Ok(recording) = is_recording.lock() {
                    if !*recording {
                        break;
                    }
                } else {
                    break;
                }

                // Skip the entire VAD/timeout pass while paused — the user is
                // taking a break, don't auto-stop or count time against them.
                if is_paused.lock().map(|p| *p).unwrap_or(false) {
                    silence_start = None; // reset silence on resume
                    // Emit a 0 level so the bar visibly drops while paused.
                    if let Some(app) = &app_handle {
                        if last_level_emit.elapsed() >= Duration::from_millis(100) {
                            let _ = app.emit("audio-level", 0.0_f32);
                            last_level_emit = Instant::now();
                        }
                    }
                    continue;
                }

                let (total_samples, rms) = {
                    if let Ok(samples_guard) = samples.lock() {
                        let total = samples_guard.len();
                        if total < samples_per_check {
                            continue;
                        }
                        let start = total.saturating_sub(samples_per_check);
                        // For multi-channel mics, many channels are silent — compute RMS on
                        // channel 0 only (stride by `channels`) to avoid diluting the signal.
                        let rms = calculate_rms_strided(&samples_guard[start..], channels as usize);
                        (total, rms)
                    } else {
                        continue;
                    }
                };

                // Emit volume level (audio-level) — throttled to ~80ms cadence.
                if let Some(app) = &app_handle {
                    if last_level_emit.elapsed() >= Duration::from_millis(80) {
                        // Normalize RMS to a 0..1 range that "looks right" on a bar.
                        // 0.15 RMS is a strong shout — clamp at that and scale linearly.
                        let level = (rms / 0.15).clamp(0.0, 1.0);
                        let _ = app.emit("audio-level", level);
                        last_level_emit = Instant::now();
                    }
                }

                // Check max recording timeout (always active, regardless of VAD)
                let current_max = max_recording_secs.lock().map(|m| *m).unwrap_or(DEFAULT_MAX_RECORDING_SECS);
                let max_samples = (current_max * sample_rate as f32) as usize * channels as usize;
                if total_samples >= max_samples {
                    if let Ok(mut t) = timeout_reached.lock() {
                        *t = true;
                    }
                    break;
                }

                // VAD silence detection (only if enabled)
                let vad_on = vad_enabled.lock().map(|v| *v).unwrap_or(true);
                if !vad_on {
                    // Surface that VAD is off so the toolbar shows the right indicator.
                    if let Some(app) = &app_handle {
                        if last_vad_state != Some("off") || last_vad_emit.elapsed() >= Duration::from_secs(2) {
                            let _ = app.emit(
                                "vad-state",
                                serde_json::json!({
                                    "state": "speaking",
                                    "silent_secs": 0.0,
                                    "silence_total": 0.0,
                                    "vad_off": true,
                                }),
                            );
                            last_vad_state = Some("off");
                            last_vad_emit = Instant::now();
                        }
                    }
                    continue;
                }

                if rms > SILENCE_THRESHOLD {
                    silence_start = None;
                    if total_samples >= min_speech_samples {
                        had_speech = true;
                    }
                } else if silence_start.is_none() {
                    silence_start = Some(Instant::now());
                }

                // VAD state emit (throttled to 500ms).
                let silence_total = silence_duration_secs
                    .lock()
                    .map(|s| *s)
                    .unwrap_or(DEFAULT_SILENCE_DURATION_SECS);
                if let Some(app) = &app_handle {
                    let (state_str, silent_secs) = match silence_start {
                        Some(s) => ("silent", s.elapsed().as_secs_f32()),
                        None => ("speaking", 0.0),
                    };
                    let force = last_vad_state != Some(state_str);
                    if force || last_vad_emit.elapsed() >= Duration::from_millis(500) {
                        let _ = app.emit(
                            "vad-state",
                            serde_json::json!({
                                "state": state_str,
                                "silent_secs": silent_secs,
                                "silence_total": silence_total,
                                "vad_off": false,
                            }),
                        );
                        last_vad_state = Some(state_str);
                        last_vad_emit = Instant::now();
                    }
                }

                if had_speech {
                    if let Some(start) = silence_start {
                        if start.elapsed().as_secs_f32() >= silence_total {
                            if let Ok(mut silence) = silence_detected.lock() {
                                *silence = true;
                            }
                            break;
                        }
                    }
                }
            }
        });

        self.vad_thread = Some(handle);
    }

    pub fn stop_recording(&mut self) -> Result<Vec<f32>, String> {
        {
            let mut recording = self.is_recording.lock().map_err(|e| e.to_string())?;
            *recording = false;
        }
        // Reset pause state so the next recording starts clean.
        if let Ok(mut p) = self.is_paused.lock() {
            *p = false;
        }

        if let Some(handle) = self.vad_thread.take() {
            let _ = handle.join();
        }

        // Signal the stream thread to stop and drop the stream on its own thread
        if let Some(tx) = self.stream_stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.stream_thread.take() {
            let _ = handle.join();
        }

        let raw_samples = self.samples.lock().map_err(|e| e.to_string())?.clone();
        let native_rate = *self.device_sample_rate.lock().map_err(|e| e.to_string())?;
        let native_channels = *self.device_channels.lock().map_err(|e| e.to_string())?;

        let mono = to_mono(&raw_samples, native_channels);

        let target_rate = 16000u32;
        if native_rate == target_rate {
            Ok(mono)
        } else {
            Ok(resample(&mono, native_rate, target_rate))
        }
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.lock().map(|r| *r).unwrap_or(false)
    }

    pub fn is_silence_detected(&self) -> bool {
        self.silence_detected.lock().map(|s| *s).unwrap_or(false)
    }

    pub fn is_timeout_reached(&self) -> bool {
        self.timeout_reached.lock().map(|t| *t).unwrap_or(false)
    }

    pub fn set_vad_enabled(&self, enabled: bool) {
        if let Ok(mut vad) = self.vad_enabled.lock() {
            *vad = enabled;
        }
    }

    /// Pause / resume sample capture without dropping the accumulated buffer.
    /// While paused, the CPAL callback discards incoming audio and the VAD
    /// monitor skips silence/timeout checks.
    pub fn set_paused(&self, paused: bool) {
        if let Ok(mut p) = self.is_paused.lock() {
            *p = paused;
        }
    }

    pub fn is_paused(&self) -> bool {
        self.is_paused.lock().map(|p| *p).unwrap_or(false)
    }
}

pub(crate) fn to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let ch = channels as usize;
    samples
        .chunks(ch)
        .map(|frame| frame.iter().sum::<f32>() / ch as f32)
        .collect()
}

/// RMS over a single channel of an interleaved audio buffer.
/// `stride` is the device channel count — 1 for mono, 4 for an array mic, etc.
/// We sample only channel 0 to avoid silent channels diluting the signal.
fn calculate_rms_strided(samples: &[f32], stride: usize) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let stride = stride.max(1);
    let mut sum_sq: f32 = 0.0;
    let mut count: usize = 0;
    for &s in samples.iter().step_by(stride) {
        sum_sq += s * s;
        count += 1;
    }
    if count == 0 {
        return 0.0;
    }
    (sum_sq / count as f32).sqrt()
}

/// Peak absolute amplitude of the buffer (0.0..~1.0).
pub fn peak_amplitude(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0_f32, |max, &s| max.max(s.abs()))
}

/// True when the buffer is effectively silent (peak below `threshold`) — i.e. the
/// microphone captured nothing (muted, disabled, or no OS permission). A muted mic
/// yields ≈0; even quiet speech peaks well above 0.02, so a 0.01 threshold cleanly
/// separates "mic captured nothing" from real (if quiet) audio. Lets the UI tell
/// the user to check the mic instead of silently returning no transcript.
pub fn is_effectively_silent(samples: &[f32], threshold: f32) -> bool {
    peak_amplitude(samples) < threshold
}

#[cfg(test)]
mod silence_helper_tests {
    use super::*;

    #[test]
    fn empty_buffer_is_silent() {
        assert!(is_effectively_silent(&[], 0.01));
    }

    #[test]
    fn zeroed_buffer_is_silent() {
        assert!(is_effectively_silent(&vec![0.0f32; 16000], 0.01));
    }

    #[test]
    fn tiny_noise_below_threshold_is_silent() {
        assert!(is_effectively_silent(&vec![0.002f32; 16000], 0.01));
    }

    #[test]
    fn speech_level_peak_is_not_silent() {
        let mut buf = vec![0.0f32; 16000];
        buf[100] = -0.3; // a speech-like transient (abs() handles the sign)
        assert!(!is_effectively_silent(&buf, 0.01));
        assert!((peak_amplitude(&buf) - 0.3).abs() < 1e-6);
    }
}

pub(crate) fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || from_rate == 0 || samples.is_empty() {
        return samples.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (samples.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_idx = i as f64 * ratio;
        let idx = src_idx as usize;
        let frac = (src_idx - idx as f64) as f32;

        let sample = if idx + 1 < samples.len() {
            samples[idx] * (1.0 - frac) + samples[idx + 1] * frac
        } else {
            samples[idx.min(samples.len() - 1)]
        };
        output.push(sample);
    }

    output
}

/// Interleave a mono mic buffer and a mono system buffer into a stereo (2-channel)
/// buffer: L = mic, R = system, laid out per-frame as [L0, R0, L1, R1, …]. The
/// shorter side is padded with silence (0.0) to the longer length, so the result is
/// always `2 * max(mic.len(), system.len())` samples. Used by Call mode to keep
/// channel 0 ("me") and channel 1 ("them") separated for Deepgram multichannel.
pub fn interleave_stereo(mic: &[f32], system: &[f32]) -> Vec<f32> {
    let max_len = mic.len().max(system.len());
    let mut out = Vec::with_capacity(max_len * 2);
    for i in 0..max_len {
        out.push(mic.get(i).copied().unwrap_or(0.0)); // L = mic
        out.push(system.get(i).copied().unwrap_or(0.0)); // R = system
    }
    out
}

#[cfg(test)]
mod interleave_stereo_tests {
    use super::*;

    #[test]
    fn equal_lengths_interleave_l_mic_r_system() {
        // L = mic, R = system, frame-interleaved: [L0, R0, L1, R1].
        let mic = [0.1f32, 0.2];
        let system = [0.3f32, 0.4];
        assert_eq!(
            interleave_stereo(&mic, &system),
            vec![0.1f32, 0.3, 0.2, 0.4]
        );
    }

    #[test]
    fn mic_longer_pads_system_with_silence() {
        // system is shorter → its missing R samples are silence (0.0).
        let mic = [0.1f32, 0.2, 0.5];
        let system = [0.3f32];
        assert_eq!(
            interleave_stereo(&mic, &system),
            vec![0.1f32, 0.3, 0.2, 0.0, 0.5, 0.0]
        );
    }

    #[test]
    fn system_longer_pads_mic_with_silence() {
        // mic is shorter → its missing L samples are silence (0.0).
        let mic = [0.1f32];
        let system = [0.3f32, 0.4];
        assert_eq!(
            interleave_stereo(&mic, &system),
            vec![0.1f32, 0.3, 0.0, 0.4]
        );
    }

    #[test]
    fn empty_inputs_yield_empty() {
        assert!(interleave_stereo(&[], &[]).is_empty());
        // One side empty still pads the other to a full stereo frame.
        assert_eq!(interleave_stereo(&[], &[0.5f32]), vec![0.0f32, 0.5]);
    }
}
