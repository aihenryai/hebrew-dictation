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

/// Characters per `enigo.text()` call. enigo's Windows backend (0.2.1) builds
/// EVERY character in one call into a single array and fires it as ONE
/// `SendInput` syscall — for a long segment that's a burst of hundreds of
/// synthetic key events landing on the target in one shot. `SendInput` events
/// are dispatched through the RECEIVING app's own message pump; if that app is
/// mid-render (heavier controlled-input UIs — chat composers, Electron apps —
/// do real work per keystroke) when the burst arrives, Windows can drop the
/// events the pump wasn't ready for. Real report (Henry, 2026-09-09): typing
/// into Claude Desktop would start, then silently stop partway through longer
/// dictations — worse as the message grew, i.e. exactly as each burst got
/// bigger and the target's per-keystroke render cost climbed. Splitting into
/// small chunks with a short pause between them gives the target's message
/// pump repeated chances to catch up; ~30 chars × 8ms adds well under a
/// second even to a long paragraph, imperceptible after a multi-second
/// speech-to-text round trip.
const INJECT_CHUNK_CHARS: usize = 30;
const INJECT_CHUNK_DELAY: std::time::Duration = std::time::Duration::from_millis(8);

/// Type the text directly via `enigo.text()`, in small paced chunks (see
/// `INJECT_CHUNK_CHARS`). Also avoids a known bug in enigo 0.2.1 on Windows
/// where `Key::Unicode('v') + Ctrl` fails with "key state could not be converted to u32"
/// because `GetKeyState` returns negative values while any modifier is held. Typing the
/// characters as Unicode WM_CHAR events bypasses the modifier path entirely and works in
/// every text field we target (chat inputs, text editors, browsers).
pub fn inject_text(text: &str) -> Result<(), String> {
    // On macOS, keystroke injection is silently dropped without Accessibility
    // permission — bail out with guidance instead of typing nothing.
    #[cfg(target_os = "macos")]
    {
        if !accessibility_trusted() {
            return Err(accessibility_permission_hint().to_string());
        }
    }

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("Enigo init error: {}", e))?;

    for (i, piece) in chunk_chars(text, INJECT_CHUNK_CHARS).iter().enumerate() {
        if i > 0 {
            std::thread::sleep(INJECT_CHUNK_DELAY);
        }
        enigo
            .text(piece)
            .map_err(|e| format!("Text input error: {}", e))?;
    }
    Ok(())
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
    fn chunk_chars_never_panics_on_a_zero_size() {
        // size.max(1) guards this — must not divide/chunk by zero.
        let pieces = chunk_chars("שלום", 0);
        assert_eq!(pieces.concat(), "שלום");
    }
}
