use enigo::{Direction, Enigo, Key, Keyboard, Settings};

pub fn inject_text(text: &str, _method: &InjectionMethod) -> Result<(), String> {
    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| format!("Enigo init error: {}", e))?;

    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("Clipboard error: {}", e))?;

    // Save existing clipboard text (images/other formats are not preserved)
    let old_clipboard = clipboard.get_text().ok();

    clipboard
        .set_text(text)
        .map_err(|e| format!("Clipboard set error: {}", e))?;

    std::thread::sleep(std::time::Duration::from_millis(250));

    enigo
        .key(Key::Control, Direction::Press)
        .map_err(|e| format!("Key error: {}", e))?;
    enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|e| format!("Key error: {}", e))?;
    enigo
        .key(Key::Control, Direction::Release)
        .map_err(|e| format!("Key error: {}", e))?;

    // Wait for paste to complete then restore clipboard
    std::thread::sleep(std::time::Duration::from_millis(500));
    if let Some(old_text) = old_clipboard {
        if clipboard.set_text(&old_text).is_err() {
            eprintln!("Warning: failed to restore previous clipboard content");
        }
    }

    Ok(())
}

pub enum InjectionMethod {
    Clipboard,
}
