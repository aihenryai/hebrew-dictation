# 🎤 הכתבה בעברית — Hebrew Voice Dictation

**הכתבה קולית בעברית מכל מקום במחשב — חינמי וקוד פתוח.**

> by [BinTech AI — הנרי שטאובר](https://taplink.cc/henry.ai)

![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)
![Platform: Windows](https://img.shields.io/badge/platform-Windows-0078d7.svg)
![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%20v2-orange.svg)

---

## מה זה?

אפליקציית Windows שמאפשרת **הכתבה קולית בעברית מכל מקום במחשב** בלחיצה על `Alt+D`.

- לחץ `Alt+D` → דבר בעברית → הטקסט מוקלד אוטומטית בשדה הפעיל
- עובד בכל תוכנה: Word, Gmail, WhatsApp Web, Slack, ועוד
- רץ ברקע — גם כשהחלון סגור

## שני מצבים

| מצב | מה צריך | יתרונות |
|-----|---------|---------|
| **☁️ API** | מפתח Deepgram/Groq (חינם, ללא כרטיס אשראי) | מהיר ומדויק מאוד |
| **💻 מקומי** | הורדת מודל Whisper (75MB-1.5GB) | פרטיות מלאה, ללא אינטרנט |

**מצב אוטומטי** (ברירת מחדל): API עם גיבוי מקומי כשאין חיבור.

## התקנה

### אפשרות 1: הורדת מתקין (מומלץ)

> 🚧 קישור להורדה יתעדכן בקרוב

### אפשרות 2: בנייה מהקוד

**דרישות:**
- [Node.js](https://nodejs.org/) 18+
- [Rust](https://rustup.rs/) 1.75+
- [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)

```bash
git clone https://github.com/aihenryai/hebrew-dictation.git
cd hebrew-dictation
npm install
npm run tauri build
```

המתקין ייווצר ב-`src-tauri/target/release/bundle/nsis/`.

## הגדרת API (אופציונלי)

לתמלול מהיר יותר, צור מפתח Deepgram חינם:

1. היכנס ל-[deepgram.com](https://deepgram.com)
2. צור חשבון (כולל $200 קרדיט חינם)
3. לחץ **Create API Key**
4. הדבק את המפתח בהגדרות האפליקציה

## מודלים מקומיים

| מודל | גודל | RAM נדרש | איכות |
|------|-------|----------|-------|
| tiny | 75 MB | 400 MB | בסיסית |
| base | 142 MB | 700 MB | סבירה |
| small | 466 MB | 1.5 GB | טובה (מומלץ) |
| medium | 1.5 GB | 3.5 GB | גבוהה |
| large-v3-turbo | 1.5 GB | 6 GB | הגבוהה ביותר לעברית |

## טכנולוגיות

- **[Tauri v2](https://v2.tauri.app/)** — framework לאפליקציות desktop
- **[whisper-rs](https://github.com/tazz4843/whisper-rs)** — Whisper.cpp bindings for Rust
- **React 19** + TypeScript — ממשק משתמש
- **[Deepgram Nova-3](https://deepgram.com/)** / **[Groq Whisper Turbo](https://groq.com/)** — API תמלול בענן (ללא כרטיס אשראי)

## תרומה לפרויקט

Pull requests ברוכים! לפני שליחת PR:

```bash
# וודא שהקוד מתקמפל
npm run tauri build

# או בנפרד
npx tsc --noEmit          # Frontend
cd src-tauri && cargo check  # Rust
```

## Privacy & Data Flow

A short fingerprint of where data goes — useful for security review or before deploying in an organization.

| What | Where it lives | When it leaves your machine |
|------|----------------|-----------------------------|
| **Audio recording** | RAM only (during Alt+D press) | Sent to the transcription provider you chose (Deepgram / Groq), then discarded. **In local Whisper mode — never leaves the machine.** |
| **API key** (Deepgram / Groq) | `%APPDATA%/hebrew-dictation/settings.json` (local file) | Only sent to the matching provider as a Bearer token in transcription requests. **Never sent to BinTech or any other third party.** |
| **Settings** (hotkey, provider choice, model) | Same `settings.json` | Never leaves the machine. |
| **History** (last transcribed lines) | Same `settings.json`, capped at recent items | Never leaves the machine. |
| **Update check** | Outbound HTTPS to `github.com/aihenryai/hebrew-dictation/releases/latest/download/latest.json` | One request at app start. Disable by blocking the host in your firewall. |

**No telemetry, no analytics, no crash reporting.** The app does not call home, does not log usage, and does not collect identifiers. The only outbound connections during normal operation are (1) the transcription provider you selected and (2) the GitHub update check above.

The microphone is engaged **only while Alt+D is held**. There is no always-on listening mode. You can verify this via the OS-level microphone indicator (Windows / macOS).

**Trademarks:** Deepgram and Groq are trademarks of their respective owners. The app supports those services via their public APIs but is not endorsed by, affiliated with, or sponsored by them.

## רישיון

[MIT](./LICENSE) — חופשי לשימוש, שינוי והפצה.

---

**נבנה על ידי [הנרי שטאובר / BinTech AI](https://taplink.cc/henry.ai)**

📧 henrystauber22@gmail.com | 🎥 [YouTube @AIWithHenry](https://youtube.com/@AIWithHenry)
