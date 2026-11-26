//! Borrowing the mic from Henry's lecture recorder.
//!
//! The lecture recorder (claude-dev/AI-Tools/lecture-recorder/lecture_capture.py) holds the microphone in WASAPI
//! *exclusive* mode, because in shared mode the Intel Smart Sound filter on Henry's laptop wipes his voice. While it
//! holds it, no other app can open the mic, so dictation during a lecture failed to start (26.11.2026).
//!
//! The handshake is two files in `~/.lecture-recorder`:
//! - `mic-yield` (`{"pid": .., "at": ..}`) exists while this app uses the mic. The recorder checks it every 100 ms,
//!   closes its exclusive stream, keeps recording in shared mode, and writes `mic-yield.ack` with the same text.
//! - We wait up to `WAIT` for that ack (only when a recording is running: `current.json` exists), and the caller
//!   retries the open a few times in case the recorder was slow.
//!
//! The flag is removed when the last user of the mic stops (`Claim` is dropped after the stream). A flag left by a
//! crash is ignored and deleted by the recorder, since its pid is no longer alive. Nothing happens on a machine
//! without `~/.lecture-recorder`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const WAIT: Duration = Duration::from_millis(400);
pub(crate) const RETRY_DELAY: Duration = Duration::from_millis(300);
pub(crate) const RETRIES: u32 = 3;

static HOLDERS: AtomicUsize = AtomicUsize::new(0);
static LOCK: Mutex<()> = Mutex::new(());

fn state_dir() -> Option<PathBuf> {
    let d = dirs::home_dir()?.join(".lecture-recorder");
    d.is_dir().then_some(d)
}

/// Held for as long as a mic stream is open; dropping the last one gives the mic back to the recorder.
pub(crate) struct Claim {
    recording: bool,
    counted: bool,
}

impl Claim {
    pub(crate) fn new() -> Self {
        let Some(dir) = state_dir() else {
            return Claim {
                recording: false,
                counted: false,
            };
        };
        let _g = LOCK.lock();
        let recording = dir.join("current.json").exists();
        if HOLDERS.fetch_add(1, Ordering::SeqCst) > 0 {
            return Claim {
                recording,
                counted: true,
            }; // the flag is already up and acknowledged
        }
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let text = format!("{{\"pid\": {}, \"at\": {}}}", std::process::id(), at);
        if std::fs::write(dir.join("mic-yield"), &text).is_err() {
            return Claim {
                recording,
                counted: true,
            };
        }
        if recording {
            let ack = dir.join("mic-yield.ack");
            let start = Instant::now();
            while start.elapsed() < WAIT {
                if std::fs::read_to_string(&ack)
                    .map(|t| t == text)
                    .unwrap_or(false)
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        Claim {
            recording,
            counted: true,
        }
    }

    /// A lecture recording is running, so a failed open is worth retrying after `RETRY_DELAY`.
    pub(crate) fn recorder_active(&self) -> bool {
        self.recording
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if !self.counted {
            return;
        }
        let _g = LOCK.lock();
        if HOLDERS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)) == Ok(1) {
            if let Some(dir) = state_dir() {
                let _ = std::fs::remove_file(dir.join("mic-yield"));
            }
        }
    }
}
