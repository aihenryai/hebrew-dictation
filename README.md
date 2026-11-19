# 🎤 הכתבה בעברית — Hebrew Voice Dictation

**הכתבה קולית בעברית מכל מקום במחשב — חינמי וקוד פתוח.**

### **[⬇️ הורדה והתקנה → bintechai.com/hebrew-dictation](https://bintechai.com/hebrew-dictation)**

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
| **☁️ API** | מפתח ElevenLabs, Deepgram או Groq (חינם, ללא כרטיס אשראי) | מהיר ומדויק מאוד |
| **💻 מקומי** | הורדת מודל Whisper (75MB-1.5GB) | פרטיות מלאה, ללא אינטרנט |

**מצב אוטומטי** (ברירת מחדל): API עם גיבוי מקומי כשאין חיבור.

## התקנה

### אפשרות 1: הורדת מתקין (מומלץ)

**⬇️ הורדה והתקנה דרך האתר → [bintechai.com/hebrew-dictation](https://bintechai.com/hebrew-dictation)**

או הורדה ישירה מ-GitHub:
- **Windows** (גרסה אחרונה): [עמוד ה-Releases](https://github.com/aihenryai/hebrew-dictation/releases/latest), הקובץ `hebrew-dictation-v…-x64.exe`
- **macOS** (גרסה אחרונה): [עמוד ה-Releases](https://github.com/aihenryai/hebrew-dictation/releases/latest), הקובץ `hebrew-dictation-v…-aarch64.dmg` לשבב Apple (M1 ומעלה), או `hebrew-dictation-v…-x64.dmg` למחשבי Intel. קיצור ההכתבה במק: ⌥ Option+D.

דאבל-קליק להתקנה — לא דורש הרשאות מנהל. עדכונים מגיעים אוטומטית דרך האפליקציה.

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

לתמלול המדויק ביותר בעברית, צור מפתח ElevenLabs:

1. היכנס ל-[elevenlabs.io](https://elevenlabs.io/app/settings/api-keys)
2. צור חשבון (יש מכסה חינמית קטנה, ואחריה כ-0.22 דולר לשעת הכתבה)
3. לחץ **Create API Key** ותן הרשאת **Speech to Text**
4. הדבק את המפתח בהגדרות האפליקציה, תחת ״ספק API״

בבדיקה שלנו על הקלטות עבריות, ElevenLabs Scribe עשה בערך חצי מהטעויות של Deepgram, בקבצים ובהכתבה סימולטנית.
חלופה עם קרדיט פתיחה גדול: [Deepgram](https://deepgram.com) (200 דולר חינם).

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
- **[ElevenLabs Scribe](https://elevenlabs.io/speech-to-text)** / **[Deepgram Nova-3](https://deepgram.com/)** / **[Groq Whisper Turbo](https://groq.com/)** — API תמלול בענן (ללא כרטיס אשראי)

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
| **Dictation audio** | RAM only, from the moment recording starts until it stops | Sent to the transcription provider you chose (ElevenLabs / Deepgram / Groq), with Deepgram's model-training opt-out set. ElevenLabs offers no such switch outside its enterprise plans, so its own data policy applies. **In local Whisper mode it never leaves the machine.** |
| **Meeting and long recordings** | A temporary WAV file in the system temp folder while it is transcribed | Deleted once the transcript is ready, and any leftover is removed the next time the app starts. Sent to ElevenLabs or Deepgram only when the files screen is set to cloud. |
| **API keys** (ElevenLabs / Deepgram / Groq) | The operating system's credential store (Windows Credential Manager / macOS Keychain) | Only sent to the matching provider with transcription requests. **Never sent to BinTech or any other third party.** An `ELEVENLABS_API_KEY`, `DEEPGRAM_API_KEY` or `GROQ_API_KEY` environment variable, if present, is used as well. |
| **Settings** (hotkey, provider choice, model) | `settings.json` in the app's data folder | Never leaves the machine. |
| **History** (recent transcripts) | A local file in the app's data folder | Never leaves the machine. |
| **Smart cleanup** (optional, off by default) | - | When turned on, the transcribed **text** is sent to Groq for rewriting, in local mode too. |
| **Update check** | Outbound HTTPS to `github.com/aihenryai/hebrew-dictation/releases/latest/download/latest.json` | At app start and every few hours while it runs. Disable by blocking the host in your firewall. |
| **Optional downloads** | Whisper models, and the narration engine if you install it | Downloaded once from their public hosts when you ask for them. |

**No telemetry, no analytics, no crash reporting.** The app does not call home, does not log usage, and does not collect identifiers.

The microphone is engaged **from the moment you start a recording until you stop it** (or, in hold-to-talk, while the keys are held). There is no always-on listening mode. A meeting recording keeps running when the window is closed to the tray, until you stop it. You can verify this via the OS-level microphone indicator (Windows / macOS).

**Trademarks:** ElevenLabs, Deepgram and Groq are trademarks of their respective owners. The app supports those services via their public APIs but is not endorsed by, affiliated with, or sponsored by them.

## רישיון

[MIT](./LICENSE) — חופשי לשימוש, שינוי והפצה.

---

**נבנה על ידי [הנרי שטאובר / BinTech AI](https://taplink.cc/henry.ai)**

📧 henrystauber22@gmail.com | 🎥 [YouTube @AIWithHenry](https://youtube.com/@AIWithHenry)
