use enigo::{Enigo, Keyboard, Settings};

// macOS: synthesizing keystrokes (enigo → CGEvent) is silently dropped unless
// the app is a trusted Accessibility client. Unlike the microphone, no Info.plist
// key can grant this — the user must enable it manually — so we detect it and
// return actionable guidance instead of typing nothing.
#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[cfg(target_os = "macos")]
fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
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
pub fn inject_text(text: &str, _method: &InjectionMethod) -> Result<(), String> {
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

pub enum InjectionMethod {
    Clipboard,
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
