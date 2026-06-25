use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

/// Maximum time allowed for a single transcription before timeout
const TRANSCRIBE_TIMEOUT_SECS: u64 = 180;

pub struct WhisperEngine {
    ctx: WhisperContext,
    /// Model name (e.g. "small", "ivrit-large-v3-turbo"). Used to enforce
    /// language overrides for models with degraded language detection.
    model_name: String,
}

impl WhisperEngine {
    pub fn new(model_path: &Path, model_name: String) -> Result<Self, String> {
        let ctx = WhisperContext::new_with_params(
            model_path.to_str().ok_or("Invalid model path")?,
            WhisperContextParameters::default(),
        )
        .map_err(|e| format!("Failed to load Whisper model: {}", e))?;

        Ok(Self { ctx, model_name })
    }

    pub fn transcribe(&self, samples: &[f32], language: &str) -> Result<String, String> {
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| format!("Failed to create whisper state: {}", e))?;

        // ivrit.ai models had their language-detection capability degraded during
        // training and the model card explicitly requires the language token to
        // be set to Hebrew. Override "auto" / any non-"he" input for these models.
        let effective_lang = if self.model_name.starts_with("ivrit-") {
            "he".to_string()
        } else {
            language.to_string()
        };

        // Run transcription with timeout to prevent hanging on long audio
        let (tx, rx) = mpsc::channel();
        let samples_owned = samples.to_vec();
        let lang_owned = effective_lang;

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

    /// Create a fresh per-run state for a long transcription, returning it plus the
    /// model name. The caller locks the engine only long enough to call this, then
    /// runs `run_long_transcription` on the returned state OFF the AppState lock —
    /// so a multi-hour batch never blocks short dictation / model management.
    pub fn create_long_state(&self) -> Result<(WhisperState, String), String> {
        let state = self
            .ctx
            .create_state()
            .map_err(|e| format!("Failed to create whisper state: {}", e))?;
        Ok((state, self.model_name.clone()))
    }
}

/// Run a long, cancellable transcription on a pre-created state. Holds NO external
/// lock (the caller already dropped the engine mutex). No fixed timeout — stops via
/// `cancel` (whisper.cpp polls the abort callback before each compute step).
/// `on_progress` receives overall percent 0–100 (whisper-rs progress is percent,
/// NOT per-segment — spec §14.1-E).
pub fn run_long_transcription<F: FnMut(i32) + 'static>(
    mut state: WhisperState,
    model_name: &str,
    samples: &[f32],
    language: &str,
    cancel: Arc<AtomicBool>,
    on_progress: F,
) -> Result<String, String> {
    // ivrit.ai models require the language token forced to Hebrew.
    let effective_lang = if model_name.starts_with("ivrit-") {
        "he".to_string()
    } else {
        language.to_string()
    };

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    if effective_lang == "auto" {
        params.set_language(None);
    } else {
        params.set_language(Some(&effective_lang));
    }
    params.set_translate(false);
    params.set_no_timestamps(true);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);

    params.set_progress_callback_safe(on_progress);
    let cancel_for_abort = cancel.clone();
    params.set_abort_callback_safe(move || cancel_for_abort.load(Ordering::Relaxed));

    let full_res = state.full(params, samples);
    // If the user cancelled, report it cleanly regardless of how full() returned.
    if cancel.load(Ordering::Relaxed) {
        return Err("בוטל".to_string());
    }
    full_res.map_err(|e| format!("Transcription failed: {}", e))?;

    let n = state.full_n_segments();
    let mut text = String::new();
    for i in 0..n {
        if let Some(segment) = state.get_segment(i) {
            if let Ok(s) = segment.to_str_lossy() {
                text.push_str(&s);
            }
        }
    }
    Ok(text.trim().to_string())
}
