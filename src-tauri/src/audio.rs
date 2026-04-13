use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// VAD (Voice Activity Detection) configuration
const SILENCE_THRESHOLD: f32 = 0.008;
const SILENCE_DURATION_SECS: f32 = 4.5;
const MIN_SPEECH_DURATION_SECS: f32 = 0.5;
const VAD_CHECK_INTERVAL_MS: u64 = 100;

/// Default maximum recording duration in seconds (local mode)
const DEFAULT_MAX_RECORDING_SECS: f32 = 60.0;

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
        }
    }

    pub fn set_max_recording_secs(&self, secs: f32) {
        if let Ok(mut max) = self.max_recording_secs.lock() {
            *max = secs.clamp(1.0, 300.0);
        }
    }

    pub fn start_recording(&mut self) -> Result<(), String> {
        // Query device info on the main thread (this is safe)
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or("No input device available")?;

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
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        let handle = std::thread::spawn(move || {
            let host = cpal::default_host();
            let device = match host.default_input_device() {
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

            let stream = match device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if let Ok(recording) = is_recording.lock() {
                        if *recording {
                            if let Ok(mut s) = samples.lock() {
                                s.extend_from_slice(data);
                            }
                        }
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

        let samples_per_check =
            (sample_rate as f32 * (VAD_CHECK_INTERVAL_MS as f32 / 1000.0)) as usize
                * channels as usize;
        let min_speech_samples =
            (MIN_SPEECH_DURATION_SECS * sample_rate as f32) as usize * channels as usize;

        let handle = std::thread::spawn(move || {
            let mut silence_start: Option<Instant> = None;
            let mut had_speech = false;

            loop {
                std::thread::sleep(std::time::Duration::from_millis(VAD_CHECK_INTERVAL_MS));

                if let Ok(recording) = is_recording.lock() {
                    if !*recording {
                        break;
                    }
                } else {
                    break;
                }

                let (total_samples, rms) = {
                    if let Ok(samples_guard) = samples.lock() {
                        let total = samples_guard.len();
                        if total < samples_per_check {
                            continue;
                        }
                        let start = total.saturating_sub(samples_per_check);
                        let rms = calculate_rms(&samples_guard[start..]);
                        (total, rms)
                    } else {
                        continue;
                    }
                };

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
                if let Ok(enabled) = vad_enabled.lock() {
                    if !*enabled {
                        continue;
                    }
                }

                if rms > SILENCE_THRESHOLD {
                    silence_start = None;
                    if total_samples >= min_speech_samples {
                        had_speech = true;
                    }
                } else if silence_start.is_none() {
                    silence_start = Some(Instant::now());
                }

                if had_speech {
                    if let Some(start) = silence_start {
                        if start.elapsed().as_secs_f32() >= SILENCE_DURATION_SECS {
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

        let mono = if native_channels > 1 {
            let ch = native_channels as usize;
            raw_samples
                .chunks(ch)
                .map(|frame| frame.iter().sum::<f32>() / ch as f32)
                .collect()
        } else {
            raw_samples
        };

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
}

fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
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
