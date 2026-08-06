//! Talks to an already-running `piper1-gpl` HTTP sidecar over localhost.
//! Process lifecycle (spawning that sidecar) lives in `narration_process.rs`;
//! this module only knows how to build its launch args and call its API.

/// Build the argv for `python -m piper.http_server`, given the voice model
/// name, bind host, and port. Pure. `--host` is security-mandatory (spec §3):
/// piper1-gpl's HTTP server defaults to `0.0.0.0` (all network interfaces),
/// so omitting this flag would expose the synthesis endpoint to the LAN.
pub fn build_server_args(model_name: &str, host: &str, port: u16) -> Vec<String> {
    vec![
        "-m".to_string(),
        "piper.http_server".to_string(),
        "-m".to_string(),
        model_name.to_string(),
        "--host".to_string(),
        host.to_string(),
        "--port".to_string(),
        port.to_string(),
    ]
}

/// Minimal structural check that `bytes` looks like a WAV file: starts with
/// the "RIFF" and "WAVE" magic bytes and has more than just the 44-byte
/// canonical header. NOT full WAV validation — this exists to reject "the
/// sidecar returned an HTML error page" or an empty body, not to validate
/// audio content or codec details.
pub fn looks_like_valid_wav(bytes: &[u8]) -> bool {
    bytes.len() > 44 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_server_args_includes_host_flag_explicitly() {
        let args = build_server_args("he_IL-saspeech-medium", "127.0.0.1", 5758);
        assert_eq!(
            args,
            vec![
                "-m", "piper.http_server",
                "-m", "he_IL-saspeech-medium",
                "--host", "127.0.0.1",
                "--port", "5758",
            ]
        );
        let host_idx = args.iter().position(|a| a == "--host").unwrap();
        assert_eq!(args[host_idx + 1], "127.0.0.1");
    }

    #[test]
    fn looks_like_valid_wav_accepts_real_header() {
        let mut fake_wav = b"RIFF".to_vec();
        fake_wav.extend_from_slice(&[0u8; 4]); // chunk size, don't care
        fake_wav.extend_from_slice(b"WAVE");
        fake_wav.extend_from_slice(&[0u8; 100]); // pretend audio data
        assert!(looks_like_valid_wav(&fake_wav));
    }

    #[test]
    fn looks_like_valid_wav_rejects_empty_body() {
        assert!(!looks_like_valid_wav(&[]));
    }

    #[test]
    fn looks_like_valid_wav_rejects_html_error_page() {
        let html = b"<html><body>500 Internal Server Error</body></html>";
        assert!(!looks_like_valid_wav(html));
    }

    #[test]
    fn looks_like_valid_wav_rejects_truncated_header() {
        assert!(!looks_like_valid_wav(b"RIFF\x00\x00\x00\x00"));
    }
}
