use enigo::{Enigo, Keyboard, Settings};

// macOS: synthesizing keystrokes (enigo → CGEvent) is silently dropped unless
// the app is a trusted Accessibility client. Unlike the microphone, no Info.plist
// key can grant this — the user must enable it manually — so we detect it and
// return actionable guidance instead of typing nothing.
//
// Discoverability caveat that shaped this file: the plain `AXIsProcessTrusted`
// is QUERY-ONLY. It never shows the system dialog and never registers the app
// in System Settings → Privacy & Security → Accessibility — and because we
// return Err before enigo posts any CGEvent, macOS's own automatic consent
// prompt never fires either. A fresh install therefore has NO path to discover
// the permission. `prompt_accessibility_if_needed` (called once at startup)
// uses AXIsProcessTrustedWithOptions with kAXTrustedCheckOptionPrompt, which
// both shows the dialog and lists the app in the pane.
#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(
        options: core_foundation::dictionary::CFDictionaryRef,
    ) -> bool;
    static kAXTrustedCheckOptionPrompt: core_foundation::string::CFStringRef;
}

#[cfg(target_os = "macos")]
fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// True when keystroke injection can work. Always true off-macOS so the
/// frontend can call it unconditionally.
pub fn is_accessibility_trusted() -> bool {
    #[cfg(target_os = "macos")]
    {
        accessibility_trusted()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// macOS: show the system Accessibility-permission dialog if the app is not
/// yet trusted, and register it in the Privacy & Security → Accessibility
/// pane either way. Returns the current trust state. No-op (true) elsewhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn prompt_accessibility_if_needed() -> bool {
    #[cfg(target_os = "macos")]
    {
        use core_foundation::base::TCFType;
        use core_foundation::boolean::CFBoolean;
        use core_foundation::dictionary::CFDictionary;
        use core_foundation::string::CFString;
        unsafe {
            // wrap_under_get_rule retains the framework-owned constant, so the
            // CFString drop releases OUR retain, never the framework's.
            let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
            let options = CFDictionary::from_CFType_pairs(&[(
                key.as_CFType(),
                CFBoolean::true_value().as_CFType(),
            )]);
            AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Guidance surfaced when injection can't run for lack of macOS Accessibility
/// permission. Pure + always compiled so it's unit-testable on any host; only
/// actually shown on macOS (see `inject_text`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn accessibility_permission_hint() -> &'static str {
    "לא ניתן להקליד את הטקסט — חסרה הרשאת נגישות. אשרו את \"הכתבה בעברית\" תחת הגדרות המערכת ← פרטיות ואבטחה ← נגישות, ואז נסו שוב."
}

// ---------------------------------------------------------------------------
// Foreground-window guard (Windows)
//
// enigo does NOT error when no text field has focus — the synthetic keystrokes
// simply go nowhere and `inject_text` still returns Ok. That made the single
// most damaging failure mode of this app invisible: the user dictates, the
// transcript is recorded as successful, and nothing appears on screen.
//
// These helpers give us the one signal that was missing: is the window that
// currently owns the foreground OUR window? If it is, typing would land in our
// own webview, so we refuse and say so instead of silently losing the text.
//
// Strictly read-only Win32. We never call SetForegroundWindow/AttachThreadInput
// — see the note in Cargo.toml for why stealing focus back is not the fix.
// ---------------------------------------------------------------------------

/// How long to wait for Windows to promote the previously-active window after
/// we hide ours. Was a flat 80ms sleep; polling returns as soon as the
/// foreground actually flips (usually 10-30ms) and still covers a loaded
/// machine where 80ms was never enough.
pub(crate) const FOREGROUND_RELEASE_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(400);
#[cfg_attr(not(windows), allow(dead_code))]
const FOREGROUND_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);
/// The flat wait this file used everywhere before polling existed. Still the
/// only option off-Windows, where we have no foreground query to poll.
#[cfg_attr(windows, allow(dead_code))]
const LEGACY_DEFOCUS_WAIT: std::time::Duration = std::time::Duration::from_millis(80);

/// Is the foreground window one of ours?
///
/// `None` means "can't tell" — the Win32 call failed, or there is no foreground
/// window at all (a normal transient state during window switches). On Windows
/// wait for a real external target; typing during that gap silently loses text.
#[cfg(windows)]
pub(crate) fn foreground_is_ours() -> Option<bool> {
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };

    // SAFETY: all three are read-only queries with no pointer aliasing beyond
    // `pid`, which is a live stack local for the duration of the call.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid: u32 = 0;
        if GetWindowThreadProcessId(hwnd, &mut pid) == 0 || pid == 0 {
            return None;
        }
        Some(pid == GetCurrentProcessId())
    }
}

#[cfg(not(windows))]
pub(crate) fn foreground_is_ours() -> Option<bool> {
    None
}

/// Poll `ready` every `poll` until it is true or `deadline` elapses; returns
/// whether it became true. The predicate is injected so the loop itself is
/// unit-testable without touching the OS (pass `Duration::ZERO` for `poll`).
#[cfg_attr(not(windows), allow(dead_code))]
fn wait_until(ready: impl Fn() -> bool, deadline: std::time::Duration, poll: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        if ready() {
            return true;
        }
        if start.elapsed() >= deadline {
            return false;
        }
        std::thread::sleep(poll);
    }
}

/// Wait until the foreground window is no longer ours. True when the foreground
/// belongs to someone else. A transient null foreground is not ready yet.
pub(crate) fn wait_for_foreground_release(deadline: std::time::Duration) -> bool {
    #[cfg(windows)]
    {
        wait_until(
            || foreground_ready(foreground_is_ours()),
            deadline,
            FOREGROUND_POLL_INTERVAL,
        )
    }
    #[cfg(not(windows))]
    {
        // Nothing to poll here — keep the historical flat wait so the OS still
        // gets its beat to promote the previously-active window.
        std::thread::sleep(LEGACY_DEFOCUS_WAIT.min(deadline));
        true
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn foreground_ready(ours: Option<bool>) -> bool {
    ours == Some(false)
}

/// Shown when we were about to type into our own window. Actionable on purpose:
/// the user's next move is to click the field they want, not to file a bug.
pub(crate) fn foreground_stolen_hint() -> &'static str {
    "הטקסט לא הוקלד - חלון האפליקציה תפס את המיקוד. לחצו על התיבה שאליה תרצו להכתיב ונסו שוב."
}

/// Pace Unicode input and recheck the target between bounded batches. This
/// limits how much text can be misdirected if focus changes during SendInput;
/// success from SendInput alone does not prove a target editor accepted text.
const INJECT_CHUNK_CHARS: usize = 30;
const INJECT_CHUNK_DELAY: std::time::Duration = std::time::Duration::from_millis(8);

/// Type the text directly via `enigo.text()`, in small paced chunks (see
/// `INJECT_CHUNK_CHARS`). Also avoids a known bug in enigo 0.2.1 on Windows
/// where `Key::Unicode('v') + Ctrl` fails with "key state could not be converted to u32"
/// because `GetKeyState` returns negative values while any modifier is held. Typing the
/// characters with KEYEVENTF_UNICODE bypasses that key-lookup path. Target
/// editors still need to support Unicode input; do not assume universal support.
pub fn inject_text(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }
    // On macOS, keystroke injection is silently dropped without Accessibility
    // permission — bail out with guidance instead of typing nothing.
    #[cfg(target_os = "macos")]
    {
        if !accessibility_trusted() {
            return Err(accessibility_permission_hint().to_string());
        }
    }

    // Last line of defence. The callers in lib.rs hide our windows and wait for
    // the foreground to flip; if it never did, typing here would put the user's
    // dictation into our own webview and report success. Refuse instead.
    if foreground_is_ours() == Some(true) {
        return Err(foreground_stolen_hint().to_string());
    }

    #[cfg(windows)]
    let target = {
        use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        if !wait_for_foreground_release(FOREGROUND_RELEASE_TIMEOUT) {
            return Err("לא נמצאה תיבת יעד פעילה. הטקסט נשמר בתמלול; לחצו על תיבת הטקסט ונסו שוב.".into());
        }
        // Do not synthesize text while Alt+D (or another modifier shortcut)
        // is still held. SendInput does not reset physical keyboard state.
        if !wait_until(modifiers_released, std::time::Duration::from_secs(2), FOREGROUND_POLL_INTERVAL) {
            return Err("ההקלדה נעצרה כי מקש קיצור עדיין לחוץ. שחררו את המקשים ונסו שוב; הטקסט נשמר בתמלול.".into());
        }
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_null() || foreground_is_ours() != Some(false) {
            return Err(foreground_stolen_hint().into());
        }
        hwnd
    };

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("Enigo init error: {}", e))?;

    for (i, piece) in plan_pieces(text, INJECT_CHUNK_CHARS).iter().enumerate() {
        if i > 0 {
            std::thread::sleep(INJECT_CHUNK_DELAY);
        }
        #[cfg(windows)]
        if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() } != target
            || !modifiers_released()
        {
            return Err("החלון או מצב המקלדת השתנו בזמן ההקלדה. ההקלדה נעצרה; התמלול המלא נשמר באפליקציה.".into());
        }
        match piece {
            Piece::Text(run) => enigo
                .text(run)
                .map_err(|e| format!("Text input error: {}", e))?,
            Piece::LineBreak => type_line_break(&mut enigo)?,
        }
    }
    Ok(())
}

/// One unit of typing: a run of ordinary characters, or a line break.
#[derive(Debug, PartialEq)]
enum Piece {
    Text(String),
    LineBreak,
}

/// Turn `text` into what `inject_text` actually types. Line breaks must never
/// reach `enigo.text()`: its Windows backend handles '\n' (and '\t') with an
/// early `return`, pressing a bare Enter and silently dropping every other
/// character of that call — the chunk's text before the newline was buffered
/// but never sent. In a chat composer (Claude, WhatsApp) that bare Enter also
/// SENDS the half-typed message. So line breaks become their own piece, typed
/// as Shift+Enter; tabs become spaces (a Tab key would move focus out of the
/// field mid-dictation); other control characters are dropped.
fn plan_pieces(text: &str, size: usize) -> Vec<Piece> {
    let normalized: String = text
        .replace("\r\n", "\n")
        .chars()
        .filter_map(|c| match c {
            '\r' | '\u{2028}' | '\u{2029}' => Some('\n'),
            '\t' => Some(' '),
            '\n' => Some('\n'),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect();
    let mut pieces = Vec::new();
    for (i, line) in normalized.split('\n').enumerate() {
        if i > 0 {
            pieces.push(Piece::LineBreak);
        }
        pieces.extend(chunk_chars(line, size).into_iter().map(Piece::Text));
    }
    pieces
}

/// Shift+Enter: a new line in chat composers (where a bare Enter sends), and
/// still a new line in editors and documents.
fn type_line_break(enigo: &mut Enigo) -> Result<(), String> {
    use enigo::{Direction, Key};
    enigo
        .key(Key::Shift, Direction::Press)
        .map_err(|e| format!("Text input error: {}", e))?;
    let enter = enigo.key(Key::Return, Direction::Click);
    // Release Shift even if Enter failed, or the user's keyboard stays shifted.
    let release = enigo.key(Key::Shift, Direction::Release);
    enter
        .and(release)
        .map_err(|e| format!("Text input error: {}", e))?;
    // The between-pieces guard reads the async key state, which can trail our
    // own Shift release by a moment. Wait for it rather than abort on it.
    #[cfg(windows)]
    if !wait_until(modifiers_released, std::time::Duration::from_millis(500), FOREGROUND_POLL_INTERVAL) {
        return Err("ההקלדה נעצרה כי מקש קיצור עדיין לחוץ. שחררו את המקשים ונסו שוב; הטקסט נשמר בתמלול.".into());
    }
    Ok(())
}

/// Unassigned virtual-key code (the same "menu mask" AutoHotkey has used by
/// default since 2017): pressing it has no meaning in any application.
#[cfg(windows)]
const MENU_MASK_VK: u16 = 0xE8;

/// Call when an Alt/Win global shortcut fires, while the user still holds it.
///
/// RegisterHotKey swallows the letter's key-down, so the focused app sees
/// Alt go down and come back up with nothing in between: a "lone Alt". Windows
/// opens the menu bar on that, and apps copy the rule — Claude Desktop pops its
/// application menu (its `before-input-event` tracker opens on an Alt key-up
/// that followed an Alt key-down with no other key-down). The menu then grabs
/// the keyboard and swallows the dictation. One press of an unassigned key
/// while Alt is still down makes it an Alt combination again. Win gets the
/// same treatment (a lone Win opens the Start menu).
#[cfg(windows)]
pub(crate) fn mask_lone_modifier_release() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        VK_LWIN, VK_MENU, VK_RWIN,
    };
    let held = |vk: u16| unsafe { GetAsyncKeyState(vk as i32) } < 0;
    if !(held(VK_MENU) || held(VK_LWIN) || held(VK_RWIN)) {
        // Already released (or a Ctrl/Shift shortcut): nothing to mask, and a
        // stray key press would be all we'd add.
        return;
    }
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: MENU_MASK_VK, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    unsafe {
        SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(not(windows))]
pub(crate) fn mask_lone_modifier_release() {}

#[cfg(windows)]
fn modifiers_released() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN,
    };
    [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
        .iter()
        .all(|key| unsafe { GetAsyncKeyState(*key as i32) >= 0 })
}

/// Split `text` into pieces of at most `size` Unicode scalar values (`char`s),
/// preserving order and content exactly — `pieces.concat() == text` always.
/// Pure so the chunk boundaries are unit-tested without touching the OS.
fn chunk_chars(text: &str, size: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(size.max(1))
        .map(|c| c.iter().collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_gap_is_not_permission_to_type() {
        assert!(!foreground_ready(None));
        assert!(!foreground_ready(Some(true)));
        assert!(foreground_ready(Some(false)));
        let observations = std::cell::RefCell::new(
            [Some(true), None, None, Some(false)].into_iter()
        );
        assert!(wait_until(
            || foreground_ready(observations.borrow_mut().next().unwrap()),
            std::time::Duration::from_secs(1),
            std::time::Duration::ZERO,
        ));
        assert!(observations.borrow_mut().next().is_none());
    }

    #[test]
    fn accessibility_hint_points_to_the_macos_pane() {
        let hint = accessibility_permission_hint();
        assert!(hint.contains("נגישות"), "must name the Accessibility pane");
        assert!(hint.contains("הגדרות המערכת"), "must name macOS System Settings");
        assert!(!hint.contains("Windows"), "must not send a Mac user to Windows");
    }

    #[test]
    fn chunk_chars_reassembles_to_the_exact_original_hebrew_text() {
        let text = "אני לא רואה אפשרות לעדכן בתוכנה, זה משפט ארוך יותר משלושים תווים";
        let pieces = chunk_chars(text, 30);
        assert_eq!(pieces.concat(), text);
        assert!(pieces.len() > 1, "a long segment must actually split");
        for p in &pieces[..pieces.len() - 1] {
            assert_eq!(p.chars().count(), 30);
        }
    }

    #[test]
    fn chunk_chars_short_text_is_a_single_chunk() {
        let pieces = chunk_chars("שלום", 30);
        assert_eq!(pieces, vec!["שלום".to_string()]);
    }

    #[test]
    fn chunk_chars_empty_text_produces_no_chunks() {
        assert!(chunk_chars("", 30).is_empty());
    }

    #[test]
    fn chunk_chars_exact_multiple_has_no_trailing_empty_chunk() {
        // 6 chars, size 3 -> exactly two chunks, not three.
        let pieces = chunk_chars("abcdef", 3);
        assert_eq!(pieces, vec!["abc".to_string(), "def".to_string()]);
    }

    #[test]
    fn wait_until_returns_immediately_when_already_ready() {
        let calls = std::cell::Cell::new(0);
        let got = wait_until(
            || {
                calls.set(calls.get() + 1);
                true
            },
            std::time::Duration::from_secs(5),
            std::time::Duration::ZERO,
        );
        assert!(got);
        assert_eq!(calls.get(), 1, "must not poll again once ready");
    }

    #[test]
    fn wait_until_polls_until_the_predicate_flips() {
        let calls = std::cell::Cell::new(0);
        let got = wait_until(
            || {
                calls.set(calls.get() + 1);
                calls.get() >= 3
            },
            std::time::Duration::from_secs(5),
            std::time::Duration::ZERO,
        );
        assert!(got);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn wait_until_gives_up_at_the_deadline_instead_of_hanging() {
        let got = wait_until(
            || false,
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
        );
        assert!(!got, "a predicate that never flips must return false, not spin");
    }

    #[test]
    fn foreground_stolen_hint_tells_the_user_what_to_do() {
        let hint = foreground_stolen_hint();
        assert!(hint.contains("לחצו"), "must tell the user to click the target field");
        assert!(
            !hint.contains("error") && !hint.contains("Error"),
            "user-facing text stays Hebrew"
        );
    }

    /// The foreground probe must not classify another process as our own.
    #[test]
    fn unknown_foreground_never_reads_as_ours() {
        assert_ne!(
            foreground_is_ours(),
            Some(true),
            "in a test process there is no foreground window of ours to find"
        );
    }

    fn typed(pieces: &[Piece]) -> String {
        pieces
            .iter()
            .map(|p| match p {
                Piece::Text(s) => s.as_str(),
                Piece::LineBreak => "\n",
            })
            .collect()
    }

    #[test]
    fn line_breaks_never_reach_enigo_text() {
        // enigo 0.2.1 on Windows returns early at the first '\n' of a text()
        // call: one bare Enter, and every other character of the call is lost.
        let text = "דובר 1: שלום לכולם\n\nדובר 2: תודה רבה, מתחילים\tעכשיו";
        let pieces = plan_pieces(text, 30);
        for p in &pieces {
            if let Piece::Text(s) = p {
                assert!(!s.contains(['\n', '\r', '\t']), "control char left in {s:?}");
            }
        }
        assert_eq!(
            pieces.iter().filter(|p| **p == Piece::LineBreak).count(),
            2,
            "a blank line between paragraphs is two line breaks"
        );
        assert_eq!(typed(&pieces), text.replace('\t', " "), "nothing may be lost");
    }

    #[test]
    fn carriage_returns_fold_into_single_line_breaks() {
        assert_eq!(typed(&plan_pieces("א\r\nב\rג\u{2029}ד", 30)), "א\nב\nג\nד");
    }

    #[test]
    fn stray_control_characters_are_dropped_not_typed() {
        // A null byte makes enigo fail the whole call; others type garbage.
        assert_eq!(typed(&plan_pieces("שלום\u{0}\u{7}עולם", 30)), "שלוםעולם");
    }

    #[test]
    fn long_lines_are_still_chunked_between_line_breaks() {
        let line = "א".repeat(65);
        let pieces = plan_pieces(&format!("{line}\n{line}"), 30);
        // 30 + 30 + 5, a break, 30 + 30 + 5.
        assert_eq!(pieces.len(), 7);
        assert_eq!(pieces[3], Piece::LineBreak);
        assert_eq!(typed(&pieces), format!("{line}\n{line}"));
    }

    #[test]
    fn text_without_line_breaks_types_exactly_as_before() {
        let text = "אני לא רואה אפשרות לעדכן בתוכנה, זה משפט ארוך יותר משלושים תווים ";
        let expected: Vec<Piece> = chunk_chars(text, 30).into_iter().map(Piece::Text).collect();
        assert_eq!(plan_pieces(text, 30), expected);
    }

    #[test]
    fn chunk_chars_never_panics_on_a_zero_size() {
        // size.max(1) guards this — must not divide/chunk by zero.
        let pieces = chunk_chars("שלום", 0);
        assert_eq!(pieces.concat(), "שלום");
    }
}
