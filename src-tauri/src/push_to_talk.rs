//! Hold-to-talk: hold Ctrl+Win, speak, release - the text is typed.
//!
//! RegisterHotKey cannot bind a chord made only of modifiers, and a low-level
//! keyboard hook would sit in the path of every keystroke on the machine. So
//! the keys are polled (GetAsyncKeyState, about 60 times a second, and only
//! while the option is on). The decisions live in the pure `step` below so they
//! are unit-tested without a keyboard.
//!
//! Why Ctrl+Win: both sit next to each other in the bottom-left corner (one
//! hand), and holding them alone does nothing in Windows. No Alt, so no app
//! menu can open. Ctrl+Win+<key> shortcuts (virtual desktops, Narrator) pass
//! through the same two keys, which is what ARM_MS and the other-key rule are
//! for.

use std::sync::atomic::{AtomicBool, Ordering};

/// Set from settings at startup and by `set_push_to_talk_enabled`.
pub(crate) static ENABLED: AtomicBool = AtomicBool::new(false);

/// How long the chord must be held, with no other key, before recording
/// starts. A Ctrl+Win+<key> shortcut reaches its third key well within this.
pub(crate) const ARM_MS: u64 = 250;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct KeySample {
    /// Ctrl and Win are both down.
    pub chord_held: bool,
    /// Any other key is down (mouse buttons and our own synthetic keys excluded).
    pub other_key_down: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Phase {
    Idle,
    Arming { since_ms: u64 },
    Talking,
    /// The chord is part of some other shortcut: ignore it until released.
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PttEvent {
    Start,
    Stop,
    /// Another key joined after recording had started: it was a shortcut, not
    /// a dictation. The frontend discards what was recorded.
    Cancel,
}

pub(crate) fn step(phase: Phase, s: KeySample, now_ms: u64) -> (Phase, Option<PttEvent>) {
    match phase {
        Phase::Idle if s.chord_held && s.other_key_down => (Phase::Blocked, None),
        Phase::Idle if s.chord_held => (Phase::Arming { since_ms: now_ms }, None),
        Phase::Idle => (Phase::Idle, None),
        Phase::Arming { .. } if !s.chord_held => (Phase::Idle, None),
        Phase::Arming { .. } if s.other_key_down => (Phase::Blocked, None),
        Phase::Arming { since_ms } if now_ms.saturating_sub(since_ms) >= ARM_MS => {
            (Phase::Talking, Some(PttEvent::Start))
        }
        Phase::Arming { .. } => (phase, None),
        Phase::Talking if !s.chord_held => (Phase::Idle, Some(PttEvent::Stop)),
        Phase::Talking if s.other_key_down => (Phase::Blocked, Some(PttEvent::Cancel)),
        Phase::Talking => (Phase::Talking, None),
        Phase::Blocked if s.chord_held => (Phase::Blocked, None),
        Phase::Blocked => (Phase::Idle, None),
    }
}

#[cfg(windows)]
fn read_keys() -> KeySample {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LCONTROL, VK_LWIN, VK_RCONTROL, VK_RWIN,
    };
    let down = |vk: u16| unsafe { GetAsyncKeyState(vk as i32) } < 0;
    let chord_held = down(VK_CONTROL) && (down(VK_LWIN) || down(VK_RWIN));
    // 0x01-0x06 mouse buttons (clicking while talking is fine); the chord's own
    // keys; 0xE7 VK_PACKET (typed text) and 0xE8 (our menu mask, sent while
    // Win is held so releasing it doesn't open Start).
    let ignored = |vk: u16| {
        vk <= 0x06
            || [VK_CONTROL, VK_LCONTROL, VK_RCONTROL, VK_LWIN, VK_RWIN, 0xE7, 0xE8].contains(&vk)
    };
    let other_key_down = (0x07u16..=0xFE).any(|vk| !ignored(vk) && down(vk));
    KeySample { chord_held, other_key_down }
}

/// Start the key watcher. Idles cheaply while the option is off.
#[cfg(windows)]
pub(crate) fn spawn_watcher(app: tauri::AppHandle) {
    use tauri::Emitter;
    std::thread::spawn(move || {
        let clock = std::time::Instant::now();
        let mut phase = Phase::Idle;
        loop {
            if !ENABLED.load(Ordering::SeqCst) {
                phase = Phase::Idle;
                std::thread::sleep(std::time::Duration::from_millis(250));
                continue;
            }
            let (next, event) = step(phase, read_keys(), clock.elapsed().as_millis() as u64);
            phase = next;
            match event {
                Some(PttEvent::Start) => {
                    // While Win is still down: otherwise releasing it opens Start.
                    crate::injector::mask_lone_modifier_release();
                    let _ = app.emit("ptt-start", ());
                }
                Some(PttEvent::Stop) => {
                    let _ = app.emit("ptt-stop", ());
                }
                Some(PttEvent::Cancel) => {
                    let _ = app.emit("ptt-cancel", ());
                }
                None => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    });
}

#[cfg(not(windows))]
pub(crate) fn spawn_watcher(_app: tauri::AppHandle) {}

#[cfg(test)]
mod tests {
    use super::*;

    const CHORD: KeySample = KeySample { chord_held: true, other_key_down: false };
    const CHORD_PLUS: KeySample = KeySample { chord_held: true, other_key_down: true };
    const NOTHING: KeySample = KeySample { chord_held: false, other_key_down: false };

    fn run(samples: &[(u64, KeySample)]) -> Vec<(u64, PttEvent)> {
        let mut phase = Phase::Idle;
        let mut events = Vec::new();
        for &(t, s) in samples {
            let (next, e) = step(phase, s, t);
            phase = next;
            if let Some(e) = e {
                events.push((t, e));
            }
        }
        events
    }

    #[test]
    fn holding_the_chord_starts_after_the_arm_delay_and_release_stops() {
        let events = run(&[(0, CHORD), (100, CHORD), (250, CHORD), (2000, CHORD), (2016, NOTHING)]);
        assert_eq!(events, vec![(250, PttEvent::Start), (2016, PttEvent::Stop)]);
    }

    #[test]
    fn a_quick_tap_never_starts_recording() {
        assert!(run(&[(0, CHORD), (120, CHORD), (140, NOTHING)]).is_empty());
    }

    #[test]
    fn ctrl_win_plus_a_key_is_a_windows_shortcut_not_dictation() {
        // Ctrl+Win+Right (switch desktop): the arrow comes before the arm delay.
        assert!(run(&[(0, CHORD), (90, CHORD_PLUS), (300, CHORD), (600, CHORD), (700, NOTHING)]).is_empty());
    }

    #[test]
    fn a_key_after_recording_started_cancels_it() {
        let events = run(&[(0, CHORD), (260, CHORD), (900, CHORD_PLUS), (950, CHORD), (1000, NOTHING)]);
        assert_eq!(events, vec![(260, PttEvent::Start), (900, PttEvent::Cancel)]);
    }

    #[test]
    fn a_chord_that_began_with_another_key_down_is_ignored_until_released() {
        assert!(run(&[(0, CHORD_PLUS), (400, CHORD), (800, CHORD), (900, NOTHING)]).is_empty());
        // ...and the next clean hold works again.
        let events = run(&[(0, CHORD_PLUS), (100, NOTHING), (200, CHORD), (460, CHORD), (500, NOTHING)]);
        assert_eq!(events, vec![(460, PttEvent::Start), (500, PttEvent::Stop)]);
    }
}
