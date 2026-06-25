//! Decode an arbitrary audio file to 16 kHz mono f32 (the format whisper-rs and
//! the WAV-for-API path both consume). Pure Rust: symphonia 0.6 decode +
//! rubato 3 resample. No ffmpeg. Cancellable per-packet; reports 0–100 progress.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use symphonia::core::audio::GenericAudioBufferRef; // .spec()/.frames()/.copy_to_vec_interleaved are inherent — do NOT import the `Audio` trait (unused → warning → fails under -D warnings)
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

const TARGET_RATE: u32 = 16_000;

/// Public entry point: file path → 16 kHz mono f32.
pub fn decode_file_to_16k_mono(
    path: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u8),
) -> Result<Vec<f32>, String> {
    let (mono, native_rate) = decode_file_to_mono_f32(path, cancel, &mut on_progress)?;
    if mono.is_empty() {
        return Ok(mono);
    }
    resample_to_16k(&mono, native_rate)
}

/// Decode to mono f32 at the file's native sample rate.
fn decode_file_to_mono_f32(
    path: &Path,
    cancel: &AtomicBool,
    on_progress: &mut impl FnMut(u8),
) -> Result<(Vec<f32>, u32), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("פתיחת הקובץ נכשלה: {}", e))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    // symphonia 0.6: Probe::probe (NOT format); options by value; returns the reader.
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|_| "פורמט אודיו לא נתמך או קובץ פגום".to_string())?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| "לא נמצא ערוץ אודיו בקובץ".to_string())?;
    let track_id = track.id;
    let total_frames: Option<u64> = track.num_frames; // may be None (VBR/streamed)

    let audio_params = match track.codec_params.as_ref() {
        Some(CodecParameters::Audio(a)) => a,
        _ => return Err("הקובץ אינו אודיו נתמך".to_string()),
    };
    let native_rate = audio_params
        .sample_rate
        .ok_or_else(|| "קצב דגימה לא ידוע".to_string())?;

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(audio_params, &AudioDecoderOptions::default())
        .map_err(|_| "הקודק של הקובץ לא נתמך (ייתכן HE-AAC — נתמך רק AAC-LC)".to_string())?;

    let mut mono: Vec<f32> = Vec::new();
    let mut scratch: Vec<f32> = Vec::new();
    let mut decoded_frames: u64 = 0;
    let mut last_pct: u8 = 0;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("בוטל".to_string());
        }
        // EOF in 0.6 = Ok(None), NOT an UnexpectedEof IoError.
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(Error::ResetRequired) => {
                return Err("הקובץ דורש reset של הזרם — לא נתמך".to_string())
            }
            Err(e) => return Err(format!("קריאת הקובץ נכשלה: {}", e)),
        };
        if packet.track_id != track_id {
            // NOTE: track_id is a public FIELD on symphonia 0.6 Packet, not a method.
            continue;
        }
        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                let frames = audio_buf.frames() as u64;
                let chans = audio_buf.spec().channels().count().max(1);
                scratch.clear();
                // Handles S16/S32/F32/... internally → interleaved f32.
                audio_buf.copy_to_vec_interleaved::<f32>(&mut scratch);
                if chans == 1 {
                    mono.extend_from_slice(&scratch);
                } else {
                    mono.reserve(scratch.len() / chans);
                    for frame in scratch.chunks_exact(chans) {
                        let sum: f32 = frame.iter().copied().sum();
                        mono.push(sum / chans as f32);
                    }
                }
                decoded_frames += frames;
                if let Some(total) = total_frames {
                    if total > 0 {
                        let pct = ((decoded_frames * 100) / total).min(100) as u8;
                        if pct != last_pct {
                            last_pct = pct;
                            on_progress(pct);
                        }
                    }
                }
            }
            // Per-packet decode/IO hiccups are recoverable — skip and continue.
            Err(Error::DecodeError(_)) => continue,
            Err(Error::IoError(_)) => continue,
            Err(e) => return Err(format!("פענוח נכשל: {}", e)),
        }
    }
    Ok((mono, native_rate))
}

/// Resample mono f32 to 16 kHz. Short-circuits when already 16 kHz.
fn resample_to_16k(mono: &[f32], src_rate: u32) -> Result<Vec<f32>, String> {
    if src_rate == TARGET_RATE {
        return Ok(mono.to_vec());
    }
    if mono.is_empty() {
        return Ok(Vec::new());
    }

    const CHUNK: usize = 1024;
    let mut resampler = Fft::<f32>::new(
        src_rate as usize,
        TARGET_RATE as usize,
        CHUNK,
        2, // sub_chunks
        1, // mono
        FixedSync::Input,
    )
    .map_err(|e| format!("אתחול resampler נכשל: {}", e))?;

    // process_all_into_buffer drives the chunk loop, the final partial block, the
    // delay-trim, and the zero-pad flush internally (tested in rubato) — so we don't
    // hand-roll the offset/flush bookkeeping. Size the output via the matching helper.
    let out_cap = resampler.process_all_needed_output_len(mono.len());
    let mut out = vec![0.0f32; out_cap];

    let input =
        InterleavedSlice::new(mono, 1, mono.len()).map_err(|e| format!("input adapter: {}", e))?;
    let mut output = InterleavedSlice::new_mut(&mut out, 1, out_cap)
        .map_err(|e| format!("output adapter: {}", e))?;

    let (_in_len, out_len) = resampler
        .process_all_into_buffer(&input, &mut output, mono.len(), None)
        .map_err(|e| format!("resample: {}", e))?;

    out.truncate(out_len);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_transcribe::samples_to_wav;
    use std::sync::atomic::AtomicBool;

    fn write_temp_wav(name: &str, samples: &[f32], rate: u32) -> std::path::PathBuf {
        let wav = samples_to_wav(samples, rate);
        let mut p = std::env::temp_dir();
        p.push(format!("hd_decode_{}.wav", name));
        std::fs::write(&p, wav).unwrap();
        p
    }

    #[test]
    fn passthrough_16k_mono() {
        let samples: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let p = write_temp_wav("16k", &samples, 16_000);
        let cancel = AtomicBool::new(false);
        let out = decode_file_to_16k_mono(&p, &cancel, |_| {}).unwrap();
        // ~1s of 16k audio; small tolerance for decoder framing.
        assert!((out.len() as i64 - 16_000).abs() < 200, "got {}", out.len());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn downsample_48k_to_16k_length() {
        let samples: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.02).sin() * 0.5).collect();
        let p = write_temp_wav("48k", &samples, 48_000);
        let cancel = AtomicBool::new(false);
        let out = decode_file_to_16k_mono(&p, &cancel, |_| {}).unwrap();
        // 1s @48k → ~16000 @16k, within resampler latency tolerance.
        assert!((out.len() as i64 - 16_000).abs() < 800, "got {}", out.len());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_file_errors() {
        let mut p = std::env::temp_dir();
        p.push("hd_decode_garbage.bin");
        std::fs::write(&p, b"not audio at all").unwrap();
        let cancel = AtomicBool::new(false);
        assert!(decode_file_to_16k_mono(&p, &cancel, |_| {}).is_err());
        let _ = std::fs::remove_file(&p);
    }
}
