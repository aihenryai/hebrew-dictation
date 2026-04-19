use enigo::{Enigo, Keyboard, Settings};

/// Type the text directly via `enigo.text()`. Avoids a known bug in enigo 0.2.1 on Windows
/// where `Key::Unicode('v') + Ctrl` fails with "key state could not be converted to u32"
/// because `GetKeyState` returns negative values while any modifier is held. Typing the
/// characters as Unicode WM_CHAR events bypasses the modifier path entirely and works in
/// every text field we target (chat inputs, text editors, browsers).
pub fn inject_text(text: &str, _method: &InjectionMethod) -> Result<(), String> {
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
