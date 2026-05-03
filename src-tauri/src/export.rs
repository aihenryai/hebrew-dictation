//! History export — write the user's dictation history to a TXT or DOCX file.
//!
//! The frontend stores history in component state (not persisted to disk) so the
//! commands here accept the items as input rather than reading from a file.

use docx_rs::{AlignmentType, Docx, Paragraph, Run, RunFonts};
use serde::Deserialize;
use std::fs::File;
use std::io::Write;
use std::path::Path;

/// One transcription entry as the frontend keeps it.
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryItem {
    pub text: String,
    /// Optional ISO-8601 timestamp string from the frontend (`new Date().toISOString()`).
    /// We keep it as a string so we don't take a tz dependency just for display.
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// Write history as plain UTF-8 text. A BOM is prepended so Notepad / Word
/// recognize the file as UTF-8 instead of guessing the legacy Windows codepage
/// and showing Hebrew as garbled question marks.
pub fn write_txt(path: &Path, items: &[HistoryItem]) -> Result<(), String> {
    let mut file = File::create(path).map_err(|e| format!("שגיאה ביצירת קובץ: {}", e))?;
    // UTF-8 BOM
    file.write_all(&[0xEF, 0xBB, 0xBF])
        .map_err(|e| format!("שגיאה בכתיבה: {}", e))?;

    let header = format!(
        "הכתבה בעברית — היסטוריית תמלול\nתאריך ייצוא: {}\nסה\"כ פריטים: {}\n\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M"),
        items.len()
    );
    file.write_all(header.as_bytes())
        .map_err(|e| format!("שגיאה בכתיבה: {}", e))?;

    for (idx, item) in items.iter().enumerate() {
        let timestamp = item.timestamp.as_deref().unwrap_or("");
        let line = if timestamp.is_empty() {
            format!("[{}]\n{}\n\n---\n\n", idx + 1, item.text)
        } else {
            format!("[{} · {}]\n{}\n\n---\n\n", idx + 1, timestamp, item.text)
        };
        file.write_all(line.as_bytes())
            .map_err(|e| format!("שגיאה בכתיבה: {}", e))?;
    }

    Ok(())
}

/// Write history as a Word document. Uses `docx-rs` and emits each entry as
/// an RTL-aligned paragraph so Hebrew renders correctly without Word having
/// to guess.
pub fn write_docx(path: &Path, items: &[HistoryItem]) -> Result<(), String> {
    let mut doc = Docx::new();

    // Title
    doc = doc.add_paragraph(
        Paragraph::new()
            .align(AlignmentType::Right)
            .add_run(
                Run::new()
                    .add_text("הכתבה בעברית — היסטוריית תמלול")
                    .size(36)
                    .bold()
                    .fonts(RunFonts::new().east_asia("Arial").ascii("Arial").cs("Arial")),
            ),
    );

    // Metadata line
    let meta = format!(
        "תאריך ייצוא: {}   ·   סה\"כ פריטים: {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M"),
        items.len()
    );
    doc = doc.add_paragraph(
        Paragraph::new()
            .align(AlignmentType::Right)
            .add_run(
                Run::new()
                    .add_text(meta)
                    .size(20)
                    .italic()
                    .fonts(RunFonts::new().east_asia("Arial").ascii("Arial").cs("Arial")),
            ),
    );

    // Empty paragraph as visual separator
    doc = doc.add_paragraph(Paragraph::new());

    for (idx, item) in items.iter().enumerate() {
        let header_text = match item.timestamp.as_deref() {
            Some(ts) if !ts.is_empty() => format!("פריט {} · {}", idx + 1, ts),
            _ => format!("פריט {}", idx + 1),
        };

        doc = doc.add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Right)
                .add_run(
                    Run::new()
                        .add_text(header_text)
                        .size(22)
                        .bold()
                        .fonts(RunFonts::new().east_asia("Arial").ascii("Arial").cs("Arial")),
                ),
        );

        // Body — split on newlines so multi-paragraph dictations render right.
        for line in item.text.split('\n') {
            doc = doc.add_paragraph(
                Paragraph::new()
                    .align(AlignmentType::Right)
                    .add_run(
                        Run::new()
                            .add_text(line)
                            .size(24)
                            .fonts(RunFonts::new().east_asia("Arial").ascii("Arial").cs("Arial")),
                    ),
            );
        }

        // Spacer between items
        doc = doc.add_paragraph(Paragraph::new());
    }

    let file = File::create(path).map_err(|e| format!("שגיאה ביצירת קובץ: {}", e))?;
    doc.build()
        .pack(file)
        .map_err(|e| format!("שגיאה בכתיבת DOCX: {}", e))?;

    Ok(())
}
