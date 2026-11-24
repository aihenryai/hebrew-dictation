//! The refine pass, done while the user is still talking.
//!
//! A dictation that types its text at the end (hold-to-talk, or live typing
//! off) gets a second, more accurate pass with the provider's file model. That
//! model takes about a quarter of a second per second of audio (2026-11-20
//! probe, scribe_v3: 2 s for 8 s, 7 s for 30 s, 12-18 s for 60 s), and sending
//! the whole recording only after the stop made the wait at the end long, and
//! made long dictations hit the timeout and fall back to the live text.
//!
//! So the recording is cut at pauses into parts of a few seconds, and each part
//! is sent as soon as it is complete. At the stop only the last part is left to
//! transcribe; the wait no longer grows with the length of the dictation.

use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

const RATE: usize = 16_000;
/// A part is not cut before this much audio: shorter parts give the model too
/// little context. FLEURS sentences, on which the file model scored 6.5%, are
/// 8-12 s long.
const MIN_PART: usize = 6 * RATE;
/// Past this, the part is cut at its quietest moment even without a pause.
const MAX_PART: usize = 20 * RATE;
/// Energy is measured in 20 ms frames.
const FRAME: usize = RATE / 50;
/// A pause is this many quiet frames in a row (0.6 s). With 0.4 s, readers'
/// breaths inside a sentence were cut and split words across parts
/// ("All | Blacks"); 0.6 s with a lower quiet level cut those far less
/// (2026-11-20 bench, 8 FLEURS recordings of 16-46 s: parts and whole
/// recording within the model's run-to-run noise, 24-27% WER both).
const PAUSE_FRAMES: usize = 30;
/// A frame is quiet below this share of the part's loud level...
const QUIET_SHARE: f32 = 0.08;
/// ...and always below this RMS level (room noise is about -66 dBFS).
const QUIET_FLOOR: f32 = 0.002;
/// A part whose peak stays under this is silence, and is not sent.
const SILENT_PEAK: f32 = 0.005;

type Part = JoinHandle<Result<String, String>>;

/// Sends one part to the file model and returns its text.
pub(crate) type Transcriber = Arc<
    dyn Fn(Vec<f32>, String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>
        + Send
        + Sync,
>;

struct Inner {
    pending: Vec<f32>,
    parts: Vec<Part>,
    language: String,
}

pub(crate) struct ProgressiveRefine {
    inner: Mutex<Inner>,
    transcribe: Transcriber,
}

fn frame_rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()
}

/// Where to cut `audio` (a sample index), or None to keep collecting.
/// The cut lands in the middle of the first pause after `MIN_PART`; with no
/// pause by `MAX_PART`, at the quietest frame of the part's last quarter.
pub(crate) fn find_cut(audio: &[f32]) -> Option<usize> {
    if audio.len() < MIN_PART + PAUSE_FRAMES * FRAME {
        return None;
    }
    let rms: Vec<f32> = audio.chunks(FRAME).map(frame_rms).collect();
    // The loud level of the part: the 90th percentile frame, so one click
    // does not set it.
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let loud = sorted[sorted.len() * 9 / 10];
    let quiet = (loud * QUIET_SHARE).max(QUIET_FLOOR);
    let first = MIN_PART / FRAME;
    let mut run = 0;
    for (i, &r) in rms.iter().enumerate() {
        run = if r < quiet { run + 1 } else { 0 };
        if i >= first && run >= PAUSE_FRAMES {
            // 0.3 s back from the current frame: inside the pause, so both
            // parts start and end in quiet.
            return Some((i + 1 - PAUSE_FRAMES / 2) * FRAME);
        }
    }
    if audio.len() >= MAX_PART {
        let from = rms.len() * 3 / 4;
        let (best, _) = rms[from..]
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))?;
        return Some((from + best) * FRAME + FRAME / 2);
    }
    None
}

fn join_texts(texts: &[String]) -> String {
    texts
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

impl ProgressiveRefine {
    pub(crate) fn new(language: &str, transcribe: Transcriber) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                pending: Vec::new(),
                parts: Vec::new(),
                language: language.to_string(),
            }),
            transcribe,
        })
    }

    /// The language for parts sent from now on. A switch in the middle of a
    /// dictation means "auto", like the whole-recording pass did.
    pub(crate) fn set_language(&self, language: &str) {
        if let Ok(mut g) = self.inner.lock() {
            g.language = language.to_string();
        }
    }

    fn send(&self, part: Vec<f32>, language: String) -> Part {
        if crate::audio::is_effectively_silent(&part, SILENT_PEAK) {
            return tokio::spawn(async { Ok(String::new()) });
        }
        tokio::spawn((self.transcribe)(part, language))
    }

    /// New 16 kHz microphone audio. Sends every part that is complete.
    pub(crate) fn push(&self, chunk: &[f32]) {
        let Ok(mut g) = self.inner.lock() else { return };
        g.pending.extend_from_slice(chunk);
        while let Some(cut) = find_cut(&g.pending) {
            let rest = g.pending.split_off(cut);
            let part = std::mem::replace(&mut g.pending, rest);
            let lang = g.language.clone();
            let handle = self.send(part, lang);
            g.parts.push(handle);
        }
    }

    /// Sends what is left and returns the text of the whole dictation, or None
    /// if any part failed or took longer than `timeout` (the caller then keeps
    /// the live text).
    pub(crate) async fn finish(&self, timeout: impl Fn(usize) -> std::time::Duration) -> Option<String> {
        let (parts, tail_len) = {
            let mut g = self.inner.lock().ok()?;
            let tail = std::mem::take(&mut g.pending);
            let tail_len = tail.len();
            let lang = g.language.clone();
            let mut parts = std::mem::take(&mut g.parts);
            // Under half a second at the end is breath or the key click.
            if tail_len >= RATE / 2 {
                parts.push(self.send(tail, lang));
            }
            (parts, tail_len)
        };
        let deadline = tokio::time::Instant::now() + timeout(tail_len);
        let mut texts = Vec::with_capacity(parts.len());
        for part in parts {
            match tokio::time::timeout_at(deadline, part).await {
                Ok(Ok(Ok(t))) => texts.push(t),
                Ok(Ok(Err(e))) => {
                    eprintln!("refine part failed, keeping the live text: {e}");
                    return None;
                }
                Ok(Err(_)) | Err(_) => {
                    eprintln!("refine part timed out, keeping the live text");
                    return None;
                }
            }
        }
        let text = join_texts(&texts);
        (!text.is_empty()).then_some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(secs: f32, amp: f32) -> Vec<f32> {
        (0..(secs * RATE as f32) as usize)
            .map(|i| amp * (i as f32 * 0.07).sin())
            .collect()
    }

    fn quiet(secs: f32) -> Vec<f32> {
        vec![0.0005; (secs * RATE as f32) as usize]
    }

    #[test]
    fn short_audio_is_not_cut() {
        let mut a = tone(3.0, 0.3);
        a.extend(quiet(1.0));
        a.extend(tone(1.5, 0.3));
        assert_eq!(find_cut(&a), None);
    }

    #[test]
    fn cuts_in_the_first_pause_after_the_minimum() {
        // A pause at 3 s is too early, and a 0.4 s breath at 5 s too short;
        // the pause at 7 s is used.
        let mut a = tone(3.0, 0.3);
        a.extend(quiet(0.7));
        a.extend(tone(1.3, 0.3));
        a.extend(quiet(0.4));
        a.extend(tone(1.6, 0.3));
        a.extend(quiet(0.8));
        a.extend(tone(2.0, 0.3));
        let cut = find_cut(&a).expect("a cut");
        let secs = cut as f32 / RATE as f32;
        assert!((7.0..7.8).contains(&secs), "cut at {secs}");
        assert!(a[cut].abs() < 0.001, "cut lands in quiet");
    }

    #[test]
    fn quiet_speech_still_finds_its_pauses() {
        // Henry's quiet dictations sit around -40 dBFS.
        let mut a = tone(7.0, 0.01);
        a.extend(vec![0.00005; RATE]);
        a.extend(tone(1.0, 0.01));
        assert!(find_cut(&a).is_some());
    }

    #[test]
    fn no_pause_cuts_at_the_maximum() {
        let a = tone(21.0, 0.3);
        let cut = find_cut(&a).expect("forced cut");
        assert!(cut >= MAX_PART * 3 / 4 && cut <= a.len());
        assert_eq!(find_cut(&tone(19.0, 0.3)), None);
    }

    fn fake() -> (Transcriber, Arc<Mutex<Vec<(usize, String)>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let c = calls.clone();
        let t: Transcriber = Arc::new(move |part: Vec<f32>, lang: String| {
            let n = {
                let mut g = c.lock().unwrap();
                g.push((part.len(), lang));
                g.len()
            };
            Box::pin(async move {
                // The first part answers last: order must still hold.
                tokio::time::sleep(std::time::Duration::from_millis(if n == 1 { 60 } else { 5 })).await;
                Ok(format!("חלק{n}"))
            })
        });
        (t, calls)
    }

    #[tokio::test]
    async fn parts_are_sent_while_talking_and_joined_in_order() {
        let (t, calls) = fake();
        let p = ProgressiveRefine::new("he", t);
        let mut a = tone(7.0, 0.3);
        a.extend(quiet(0.8));
        a.extend(tone(7.0, 0.3));
        a.extend(quiet(0.8));
        for c in a.chunks(160) {
            p.push(c);
        }
        assert_eq!(calls.lock().unwrap().len(), 2, "both sentences sent before the stop");
        p.set_language("auto");
        p.push(&tone(2.0, 0.3));
        let text = p.finish(|_| std::time::Duration::from_secs(5)).await;
        assert_eq!(text.as_deref(), Some("חלק1 חלק2 חלק3"));
        let calls = calls.lock().unwrap();
        assert_eq!(calls[0].1, "he");
        assert_eq!(calls[2].1, "auto");
        let total: usize = calls.iter().map(|c| c.0).sum();
        assert_eq!(total, a.len() + 2 * RATE, "no audio lost or repeated");
    }

    #[tokio::test]
    async fn silence_and_a_short_tail_are_not_sent() {
        let (t, calls) = fake();
        let p = ProgressiveRefine::new("he", t);
        p.push(&quiet(8.0));
        p.push(&vec![0.0; RATE]);
        let text = p.finish(|_| std::time::Duration::from_secs(5)).await;
        assert_eq!(text, None);
        let p = ProgressiveRefine::new("he", fake().0);
        p.push(&tone(0.4, 0.3));
        assert_eq!(p.finish(|_| std::time::Duration::from_secs(5)).await, None);
        assert!(calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_failed_part_keeps_the_live_text() {
        let t: Transcriber = Arc::new(|_p, _l| Box::pin(async { Err("500".to_string()) }));
        let p = ProgressiveRefine::new("he", t);
        p.push(&tone(3.0, 0.3));
        assert_eq!(p.finish(|_| std::time::Duration::from_secs(5)).await, None);
    }

    #[tokio::test]
    async fn a_slow_part_times_out() {
        let t: Transcriber = Arc::new(|_p, _l| {
            Box::pin(async {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                Ok("x".to_string())
            })
        });
        let p = ProgressiveRefine::new("he", t);
        p.push(&tone(3.0, 0.3));
        assert_eq!(p.finish(|_| std::time::Duration::from_millis(50)).await, None);
    }
}
