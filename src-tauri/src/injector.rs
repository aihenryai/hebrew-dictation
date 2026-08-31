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

/// Type the text directly via `enigo.text()`. Avoids a known bug in enigo 0.2.1 on Windows
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
    enigo
        .text(text)
        .map_err(|e| format!("Text input error: {}", e))?;
    Ok(())
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
}
