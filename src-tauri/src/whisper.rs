use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Maximum time allowed for a single transcription before timeout
const TRANSCRIBE_TIMEOUT_SECS: u64 = 180;

pub struct WhisperEngine {
    ctx: WhisperContext,
}

impl WhisperEngine {
    pub fn new(model_path: &Path) -> Result<Self, String> {
        let ctx = WhisperContext::new_with_params(
            model_path.to_str().ok_or("Invalid model path")?,
            WhisperContextParameters::default(),
        )
        .map_err(|e| format!("Failed to load Whisper model: {}", e))?;

        Ok(Self { ctx })
    }

    pub fn transcribe(&self, samples: &[f32], language: &str) -> Result<String, String> {
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| format!("Failed to create whisper state: {}", e))?;

        // Run transcription with timeout to prevent hanging on long audio
        let (tx, rx) = mpsc::channel();
        let samples_owned = samples.to_vec();
        let lang_owned = language.to_string();

        std::thread::spawn(move || {
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

            if lang_owned == "auto" {
                params.set_language(None);
            } else {
                params.set_language(Some(&lang_owned));
            }
            params.set_translate(false);
            params.set_no_timestamps(true);
            params.set_single_segment(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_special(false);
            params.set_print_timestamps(false);

            let result = state.full(params, &samples_owned);
            let text = match result {
                Ok(()) => {
                    let num_segments = state.full_n_segments();
                    let mut text = String::new();
                    for i in 0..num_segments {
                        if let Some(segment) = state.get_segment(i) {
                            if let Ok(segment_text) = segment.to_str_lossy() {
                                text.push_str(&segment_text);
                            }
                        }
                    }
                    Ok(text.trim().to_string())
                }
                Err(e) => Err(format!("Transcription failed: {}", e)),
            };
            let _ = tx.send(text);
        });

        match rx.recv_timeout(Duration::from_secs(TRANSCRIBE_TIMEOUT_SECS)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Err("התמלול חרג מזמן המקסימום (3 דקות). נסה הקלטה קצרה יותר או מודל קטן יותר.".to_string())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("Transcription thread terminated unexpectedly".to_string())
            }
        }
    }
}
