use std::ffi::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperError,
    WhisperState,
};

/// Translate a `WhisperError` to a Hebrew message with an actionable next step.
/// The raw `WhisperError` Display is English ("Generic whisper error. Error code: -6")
/// which leaves users with nothing to act on. Keep messages concrete: WHAT went wrong
/// + WHAT to do next, in Henry's tone (direct, practical).
fn whisper_error_to_he(e: &WhisperError) -> String {
    match e {
        // The whisper.cpp encode step failed (out of memory / GPU/CPU backend issue / corrupt model file).
        // Common causes on Henry's audience: not enough free RAM, or a partly-downloaded model file.
        WhisperError::GenericError(code) => format!(
            "התמלול המקומי נכשל (קוד שגיאה {code}). נסה: (1) לסגור תוכנות פתוחות לפנות זיכרון, (2) לבחור מודל קטן יותר בהגדרות, או (3) למחוק את המודל בהגדרות ולהוריד אותו מחדש (ייתכן שההורדה הקודמת נקטעה)."
        ),
        WhisperError::NoSamples => {
            "הקובץ ריק או קצר מדי לתמלול.".to_string()
        }
        WhisperError::FailedToCreateState => {
            "טעינת המודל המקומי נכשלה. נסה לסגור תוכנות פתוחות (לפנות זיכרון) או לבחור מודל קטן יותר בהגדרות.".to_string()
        }
        WhisperError::InvalidText => {
            "המרת טקסט לטוקנים נכשלה במודל המקומי. בחר מודל אחר בהגדרות.".to_string()
        }
        WhisperError::InvalidThreadCount => {
            "תצורת מספר ה-threads לא תקינה. דווח על כך בכתובת henrystauber22@gmail.com.".to_string()
        }
        WhisperError::InvalidUtf8 { .. } => {
            "התמלול הופק טקסט לא תקין (UTF-8). נסה שוב, ואם זה חוזר — נסה מודל אחר.".to_string()
        }
        // Anything else from the underlying library — surface a clear Hebrew umbrella
        // and include the technical detail in parens for Henry to triage if needed.
        other => format!(
            "התמלול המקומי נכשל. נסה לבחור מודל אחר בהגדרות. (פרטים טכניים: {other})"
        ),
    }
}

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
            model_path
                .to_str()
                .ok_or_else(|| "נתיב המודל אינו תקין (תווים לא נתמכים).".to_string())?,
            WhisperContextParameters::default(),
        )
        .map_err(|e| {
            format!(
                "טעינת המודל המקומי נכשלה. ייתכן שקובץ המודל פגום או לא הורד עד הסוף — נסה למחוק אותו בהגדרות ולהוריד מחדש. (פרטים טכניים: {e})"
            )
        })?;

        Ok(Self { ctx, model_name })
    }

    pub fn transcribe(&self, samples: &[f32], language: &str) -> Result<String, String> {
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| format!("יצירת מצב התמלול המקומי נכשלה. נסה לטעון מחדש את המודל בהגדרות. (פרטים טכניים: {e})"))?;

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
                Err(e) => Err(whisper_error_to_he(&e)),
            };
            let _ = tx.send(text);
        });

        match rx.recv_timeout(Duration::from_secs(TRANSCRIBE_TIMEOUT_SECS)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Err("התמלול חרג מזמן המקסימום (3 דקות). נסה הקלטה קצרה יותר או מודל קטן יותר.".to_string())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(
                "תהליך התמלול הסתיים באופן בלתי צפוי. נסה שוב; אם זה חוזר, החלף מודל בהגדרות.".to_string(),
            ),
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
            .map_err(|e| format!("יצירת מצב התמלול המקומי נכשלה. נסה לטעון מחדש את המודל בהגדרות. (פרטים טכניים: {e})"))?;
        Ok((state, self.model_name.clone()))
    }
}

/// Global abort flag for the single in-flight local batch transcription.
///
/// whisper-rs 0.16's `set_abort_callback_safe` is BUGGY: its trampoline is
/// parameterized by the closure type `F` while the stored `user_data` actually
/// points to a `Box<dyn FnMut()->bool>` (the progress wrapper gets this right;
/// the abort one doesn't). So it reads garbage, returns a spurious `true`, and
/// whisper aborts the encode → `full()` returns `GenericError(-6)` ("failed to
/// encode"). We avoid that API entirely and use the raw `set_abort_callback` with
/// a plain C function reading this static — no `user_data` box needed because
/// only one batch runs at a time (guarded by `batch_in_progress`).
static LOCAL_ABORT: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn local_abort_callback(_user_data: *mut c_void) -> bool {
    LOCAL_ABORT.load(Ordering::Relaxed)
}

/// Trip the abort flag so the in-flight local transcription stops at the next
/// whisper.cpp checkpoint. Called from `cancel_batch`.
pub fn request_local_abort() {
    LOCAL_ABORT.store(true, Ordering::Relaxed);
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
) -> Result<(String, Vec<crate::srt::TimedSegment>), String> {
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
    // SRT needs real segment timing (was `true`) — see spec's "Open risk to
    // verify at runtime, with fallback" if this regresses accuracy/speed.
    params.set_no_timestamps(false);
    // whisper.cpp's own SRT-chunking mechanism (same as its `--max-len` CLI
    // flag): caps each segment's length and never cuts mid-word, so
    // get_segment/start_timestamp/end_timestamp below already come back
    // pre-chunked into short, readable cues — no token-level API needed.
    params.set_max_len(42);
    params.set_split_on_word(true);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);

    params.set_progress_callback_safe(on_progress);
    // Raw abort callback reading the module static (the safe wrapper is broken in
    // 0.16 — see LOCAL_ABORT). Reset the flag before the run; `cancel_batch` trips it
    // via `request_local_abort()`. No user_data needed (single in-flight batch).
    LOCAL_ABORT.store(false, Ordering::Relaxed);
    unsafe {
        params.set_abort_callback(Some(local_abort_callback));
    }

    let full_res = state.full(params, samples);
    // If the user cancelled, report it cleanly regardless of how full() returned.
    let cancelled = cancel.load(Ordering::Relaxed) || LOCAL_ABORT.load(Ordering::Relaxed);
    LOCAL_ABORT.store(false, Ordering::Relaxed);
    if cancelled {
        return Err("בוטל".to_string());
    }
    full_res.map_err(|e| whisper_error_to_he(&e))?;

    let n = state.full_n_segments();
    let mut text = String::new();
    let mut segments = Vec::new();
    for i in 0..n {
        if let Some(segment) = state.get_segment(i) {
            if let Ok(s) = segment.to_str_lossy() {
                let trimmed = s.trim();
                // Skipping whitespace-only segments here also intentionally affects
                // the plain-text output (not just `segments`) — a whitespace-only
                // segment previously contributed nothing meaningful anyway, so this
                // just avoids inserting an extra space into `text` at its position.
                if !trimmed.is_empty() {
                    text.push_str(&s);
                    // start_timestamp/end_timestamp are in centiseconds (10ms units).
                    let start_raw = segment.start_timestamp();
                    let end_raw = segment.end_timestamp();
                    if start_raw < 0 || end_raw < 0 {
                        eprintln!(
                            "whisper: negative segment timestamp (start={start_raw}, end={end_raw}) at segment {i}, clamping to 0"
                        );
                    }
                    segments.push(crate::srt::TimedSegment {
                        text: trimmed.to_string(),
                        start_ms: (start_raw.max(0) as u64) * 10,
                        end_ms: (end_raw.max(0) as u64) * 10,
                        // Local whisper has no diarization — speaker is always unknown.
                        speaker: None,
                    });
                }
            }
        }
    }
    Ok((text.trim().to_string(), segments))
}
