//! Secure storage wrapper around the `keyring` crate.
//!
//! Service name `"hebrew-dictation"` + entry-per-provider (`"deepgram"` / `"groq"`).
//! On Windows: Windows Credential Manager (DPAPI). On macOS: Keychain.
//! On Linux: Secret Service / kwallet (not currently a build target, but supported).
//!
//! Errors are mapped to Hebrew strings ready for the webview.

use keyring::Entry;

const SERVICE: &str = "hebrew-dictation";

pub fn save_key(provider: &str, key: &str) -> Result<(), String> {
    let entry = Entry::new(SERVICE, provider)
        .map_err(|e| format!("שגיאת גישה לאחסון מאובטח: {}", e))?;
    entry
        .set_password(key)
        .map_err(|e| format!("נכשלה שמירת המפתח באחסון מאובטח: {}", e))
}

pub fn load_key(provider: &str) -> Result<Option<String>, String> {
    let entry = Entry::new(SERVICE, provider)
        .map_err(|e| format!("שגיאת גישה לאחסון מאובטח: {}", e))?;
    match entry.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("נכשלה קריאת מפתח מהאחסון המאובטח: {}", e)),
    }
}

pub fn delete_key(provider: &str) -> Result<(), String> {
    let entry = Entry::new(SERVICE, provider)
        .map_err(|e| format!("שגיאת גישה לאחסון מאובטח: {}", e))?;
    match entry.delete_credential() {
        Ok(_) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()), // already absent — idempotent
        Err(e) => Err(format!("נכשלה מחיקת מפתח מהאחסון המאובטח: {}", e)),
    }
}
