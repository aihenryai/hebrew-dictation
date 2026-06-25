# Spec — Batch Transcription (קבצים + הקלטות ארוכות) for Hebrew Dictation

- **תאריך:** 2026-06-22
- **גרסת יעד:** v2.10.0 (minor — פיצ'ר חדש)
- **סטטוס:** Design (ממתין ל-spec review + תוכנית מימוש)
- **פרויקט:** `AI-Tools/MCP-Dev/hebrew-dictation`
- **מחקר מקור:** workflow `batch-transcription-research` (7 סוכני מחקר + סינתזה), 2026-06-22

---

## 1. הבעיה והמטרה

כיום התוסף עושה **הכתבה קצרה בלבד**: Alt+D → דיבור → תמלול → הזרקה. אין דרך לתמלל **קובץ אודיו קיים** או **הקלטה ארוכה** (פגישה). המטרה: להוסיף **pipeline שני, אדיטיבי** (לא refactor) שמכסה:
1. **העלאת קובץ אודיו קיים** (mp3/m4a/wav/ogg/flac) → תמלול.
2. **הקלטת פגישה ארוכה באפליקציה** (30–90 דק') → עצירה → תמלול.

**אילוצים (החלטות הנרי):** 100% client-side, **bring-your-own-key** (Deepgram/Groq/מקומי), עברית-first, **בלי שרת**. המוצר נשאר חינמי.

**פלט גמיש:** אזור עריכה באפליקציה + ייצוא TXT/DOCX + אופציית הזרקה לשדה הפעיל + אופציית שמירה אוטומטית ליד הקובץ. (בלי סיכום AI — תמלול בלבד.)

**🔑 דרישה מפורשת (הנרי):** **שני הפיצ'רים — גם העלאת קבצים וגם הקלטה — חייבים לעבוד גם עם המודל המקומי (offline), לא רק בענן.** המסלול המקומי הוא first-class, לא deferred. משמעות מימוש: תיקון ה-timeout של whisper מוקדם ל-Phase 1, ומסלול מקומי נכלל כבר ב-MVP של העלאת הקבצים (ר' §10).

---

## 2. מה כבר קיים ומנוצל-מחדש

הפיצ'ר אדיטיבי. מנוצל מהקוד הקיים:

| מודול | שימוש ב-batch |
|---|---|
| `api_transcribe.rs::samples_to_wav` | בניית WAV PCM16 להעלאות API (נכון לפי המחקר) |
| `api_transcribe.rs::{ApiError, classify_status, classify_request_error}` | טקסונומיית שגיאות + מחרוזות עברית — לנצל, לא להמציא מחדש |
| `whisper.rs::WhisperEngine` | מנוע מקומי — **עם הסרת ה-timeout של 180ש'** (החוסם המקומי המרכזי) |
| `export.rs::{write_txt, write_docx, HistoryItem}` | ייצוא TXT/DOCX (כבר RTL+Arial+BOM נכון); תוצאת batch = `HistoryItem` יחיד |
| `injector.rs::inject_text` + command | "הזרקה לשדה הפעיל" — כבר פתור |
| `settings.rs` (`ApiProvider`, secure keys, `active_api_key`) | BYO-key, בחירת ספק — כבר פתור, בלי שרת |
| `audio.rs` (recorder, VAD, `set_vad_enabled(false)`, pause) | מסלול ההקלטה; מצב-פגישה מכבה VAD ומרים את התקרה |
| `model.rs` | ניהול מודלים מקומיים — כבר פתור |

**שום דבר לא דורש שרת.** כל מסלול במחקר שהצביע על שרת (Deepgram `callback=`, פרמטר `url=` מרוחק) נדחה במפורש כי הוא מפר את אילוץ ה-no-server.

---

## 3. אסטרטגיה לכל ספק (ההחלטה המרכזית)

| מסלול | chunking? | נימוק (מחקר) |
|---|---|---|
| **Deepgram Nova-3** (ברירת מחדל לפגישות ארוכות) | **לא** | תקרת 2GB מכסה 90 דק' (~173MB); תקרת processing ~10 דק', ו-Nova-3 מעבד 90 דק' ב<2 דק' — בקשה סינכרונית אחת |
| **Whisper מקומי** | **לא** | whisper.cpp מחלן 30ש' פנימית; buffer של 30–90 דק' = ~115–345MB, נכנס ל-RAM |
| **Groq turbo**, קובץ/הקלטה > ~13 דק' | **כן** | תקרת bytes 25MB(free)/100MB(dev); נכשל בשקט על קובץ גדול מדי |
| **Groq**, קליפ < ~13 דק' | לא | נכנס בבקשה אחת |

### Deepgram (פגישות ארוכות בענן)
- **בקשה סינכרונית אחת**, raw body (לא multipart), `Content-Type: audio/wav`, header `Authorization: Token <key>`.
- URL: `https://api.deepgram.com/v1/listen?model=nova-3&language=he&smart_format=true&punctuate=true&paragraphs=true&utterances=true` (paragraphs/utterances לשבירת פסקאות נכונה ב-90 דק').
- `language=multi` ל-code-switching עברית+אנגלית.
- client עם `.timeout(900s)` + retry-with-backoff על 503/504.
- עלות ~$0.26/שעה (90 דק' ≈ $0.39); מפתחות חדשים = $200 קרדיט.

### Groq turbo (קבצים קצרים / fast)
- מסלול multipart קיים (`transcribe_groq_inner`), `model=whisper-large-v3-turbo`, `language=he`.
- **chunking חובה** מעל ~13 דק': חלונות **5 דק' + חפיפה 3ש'**, תמלול **סדרתי** (turbo ~216x realtime), **תפירה + de-dupe** של החפיפה. backoff על 429.
- toggle אופציונלי `whisper-large-v3` ("דיוק גבוה").

### Whisper מקומי (פרטיות/offline)
- **`state.full()` יחיד** על כל ה-buffer — בלי chunking ידני.
- **חובה:** להסיר את ה-timeout של 180ש', להפוך ל-cancellable, לחווט progress per-segment.
- **ברירת מחדל = `small` (או `ivrit-` small), לא turbo** (turbo איטי יותר ב-CPU!). אזהרה מראש: 90 דק' = 1.5–4 שעות.

### כלל המוצר (UI)
בחירה דו-צירית: **"מהיר (ענן, המפתח שלך)"** מול **"פרטי (במכשיר, איטי)"**. נתב קטן `pick_batch_provider`: פגישה ארוכה בענן → Deepgram; קליפ קצר / רק מפתח Groq → Groq; רגיש/offline → מקומי.

---

## 4. פענוח קבצים (פיצ'ר ההעלאה)

**החלטה: Rust טהור — `symphonia` (decode) + `rubato` (resample) + helper של ~30 שורות.** לא ffmpeg.
- נימוק (מחקר): הכי קל ועמיד, אפס התקנה חיצונית, בלי sidecar, בלי GPL, תוספת מאות KB מול 50–100MB/arch ל-ffmpeg; אותה צורה כמו ה-pipeline הקיים (whisper-rs צריך f32 16kHz mono).
- **Cargo:** `symphonia = { version = "0.6", features = ["mp3","aac","isomp4","alac","vorbis","ogg","wav","flac"] }`, `rubato = "0.15"`. ⚠️ AAC/MP4 כבויים כברירת מחדל — שכחת `aac`+`isomp4` = `.m4a` נכשל בשקט בעוד WAV/MP3 עובדים. **לבדוק עם `.m4a` אמיתי מ-iPhone מוקדם.**
- **pipeline:** decode packets → planar f32 → **mono mixdown קודם, אז resample native→16000** בבלוקים קבועים (טיפול בבלוק חלקי אחרון). **streaming packet-by-packet** (לא materializing) כי 90 דק' = ~345MB ב-16kHz וה-native intermediate פי-3 transiently.
- **ffmpeg = fallback צר בלבד, נדחה לפאזה מאוחרת:** רק אם בדיקות מראות צורך ב-HE-AAC/SBR או קבצים פגומים. iPhone Voice Memos = AAC-LC ומפענח טוב.
- **מודול חדש:** `src-tauri/src/decode.rs` — `decode_file_to_16k_mono(path, on_progress) -> Result<Vec<f32>, String>`.

---

## 5. אסטרטגיית זיכרון להקלטה ארוכה (RAM מול קובץ זמני)

**החלטה: במצב-פגישה — streaming לקובץ זמני בדיסק תוך כדי הקלטה. המסלול הקיים ב-RAM נשאר להכתבה קצרה.**

המנגנון (מחקר):
1. **ה-CPAL callback מפסיק לדחוף ל-`Arc<Mutex<Vec<f32>>>`**; דוחף ל-**`ringbuf`** SPSC (או mpsc). (לעולם לא file I/O ב-callback בזמן-אמת — חוסם → `BufferUnderrun`.)
2. **thread כותב ייעודי** מנקז → temp WAV דרך `hound::WavWriter` ב-`BufWriter`, `flush()` כל ~5–10ש' כ-checkpoint (אחרי flush הקובץ ניתן לשחזור — מאבדים מקסימום 10ש', לא את כל הפגישה).
3. **rolling tail קטן (~1ש') ב-RAM** רק ל-VAD/level UI (ה-VAD thread כבר לא נועל את כל ה-buffer → מתקן גם lock-contention קיים).
4. **בעצירה:** לסגור את ה-WAV, אז resample-to-16k + (אופציונלי) compress פעם אחת להעלאה.

guardrails (כולם נוגעים בקבועים קיימים):
- **להרים `MAX_RECORDING_CEILING_SECS`** (כרגע 3600) במצב-פגישה (גייטד מאחורי המצב המפורש כדי שהכתבה רגילה תשמור על תקרת השעה).
- **לכבות VAD auto-stop** במצב ארוך דרך `set_vad_enabled(false)`; pause/resume הקיים עובד להפסקות.
- **קובץ זמני בתיקייה הפרטית** (`dirs::data_dir()/hebrew-dictation/tmp/`), שם ייחודי, **נמחק בהצלחה, נשמר בכשל** לשחזור. טיפול ב-disk-full.

> **פישוט ל-MVP:** ה-rewrite של ringbuf+writer-thread הוא הצורה הנכונה לטווח-ארוך אבל הכי מסוכן ל-`audio.rs` המוכח. ל-MVP אפשר לשחרר הקלטה ארוכה ע"י **הרמת התקרה ושמירה ב-RAM** עד ~60 דק' (~230MB, קביל ב-8GB+), ולעשות את ה-disk-streaming בפאזה 4. גייטד למצב opt-in עם אזהרת RAM.

---

## 6. Chunking + תפירה — רק היכן שצריך

**stitching ממומש פעם אחת** ב-`chunk.rs`, ובשימוש **רק** ע"י מסלול Groq:
- חלוקת `Vec<f32>` של 16kHz mono ל-**חלונות 5 דק' + חפיפה 3ש'** (אריתמטיקת אינדקסים טהורה — אין ffmpeg כי כבר מחזיקים mono PCM).
- תמלול כל חתיכה דרך `transcribe_groq_inner` (refactored לקבל `&[f32]`).
- **de-dupe חפיפה** בתפירה: longest-common-suffix/prefix על ~3ש' tokens.
- backoff על 429; progress per-chunk (`{chunk, total}`) = גם progress bar וגם partial-recovery.

---

## 7. טיפול בשגיאות + progress

### שגיאות — לנצל את הטקסונומיה הקיימת, להרחיב timeouts
- **כל שגיאות API דרך `ApiError` + Hebrew `Display`** — בלי מחרוזות חדשות ל-network/auth/credit/rate-limit.
- **לתקן את ה-timeout של 30ש'** (ב-`transcribe_groq_inner` ו-`transcribe_deepgram_inner`): client batch נפרד עם `.timeout(900s)` + retry על 503/504. הכתבה קצרה שומרת על timeout קצר.
- **שגיאות batch חדשות** (decode נכשל, disk full, HE-AAC לא נתמך, cancelled) → וריאנטים עבריים.
- **מקומי ארוך:** להחליף את 180ש' ב-**run ניתן-לביטול** (דגל `AtomicBool` נבדק ב-progress callback) — בלי timeout קבוע.

### progress — לפי תבנית ה-events הקיימת
האפליקציה כבר משתמשת ב-`app.emit` + `listen<T>` (`model-download-progress`, `audio-level`, `vad-state`). events חדשים:
- `batch-progress` → `{ stage: "decoding"|"uploading"|"transcribing"|"chunk"|"stitching", pct, chunkIndex?, chunkTotal? }`
- `batch-done` / `batch-error`.
- decode: ספירת packets; מקומי: `set_progress_callback` של whisper.cpp → `batch-progress`.

---

## 8. קבצים / commands / UI חדשים

### מודולי Rust חדשים
| קובץ | אחריות |
|---|---|
| `src-tauri/src/decode.rs` | symphonia+rubato → `decode_file_to_16k_mono`, emits `batch-progress{decoding}` |
| `src-tauri/src/chunk.rs` | `split_overlapping(&[f32], 5min, 3s)`, `stitch_dedupe(Vec<String>)` — Groq-only |
| `src-tauri/src/batch.rs` | אורקסטרטור: `pick_batch_provider`, decode/chunk/transcribe/stitch, progress, מחזיר טקסט. עוטף את client ה-900ש' |
| `src-tauri/src/recording_sink.rs` *(פאזה 4)* | ringbuf + hound writer-thread temp-WAV sink |

### refactors מינימליים-אדיטיביים
- `api_transcribe.rs`: לחלץ את `transcribe_groq_inner`/`transcribe_deepgram_inner` לקבל `reqwest::Client` + model/timeout מוגדרים. **חתימות ההכתבה הקצרה זהות.**
- `whisper.rs`: להוסיף `transcribe_long(&self, samples, lang, cancel, on_progress)` — בלי 180ש', עם progress. `transcribe` נשאר להכתבה קצרה.
- `audio.rs`: להוסיף `start_long_recording()` (VAD off, ceiling raised, → temp-WAV sink בפאזה 4); המתודות הקיימות נשמרות.

### Tauri commands חדשים (`lib.rs` `invoke_handler!`)
```
transcribe_file(path, opts: BatchOpts) -> Result<String>
start_long_recording() -> Result<()>
stop_long_recording_and_transcribe(opts) -> Result<String>
cancel_batch() -> Result<()>
save_transcript_next_to(audio_path, text, format) -> Result<String>
```
- `BatchOpts { mode: "cloud"|"local", provider?, model?, language, autosave, inject }`.
- `save_transcript_next_to` כותב `<audiofile>.he.txt`/`.docx` ליד המקור דרך `export::{write_txt, write_docx}` (עוטף תוצאה ב-`vec![HistoryItem{text, timestamp}]`).
- inject מנצל את `inject_text` הקיים; ייצוא ידני מנצל `export_history` + `tauri-plugin-dialog` הקיים.

### React UI (אדיטיבי ב-`App.tsx`, מנצל `App.css`)
פאנל חדש **"תמלול קובץ / פגישה"**:
1. שני כפתורים: **"העלה קובץ אודיו"** (file picker מסונן mp3/m4a/wav/ogg/flac) ו-**"הקלט פגישה"** (`start_long_recording`, טיימר חי + audio-level bar; "עצור ותמלל" → `stop_long_recording_and_transcribe`).
2. toggle מצב: "מהיר (ענן)" מול "פרטי (במכשיר)" → `BatchOpts.mode`.
3. progress component מאזין ל-`batch-progress` + כפתור **ביטול** → `cancel_batch`.
4. `<textarea dir="rtl">` editable עם התמלול.
5. שלוש פעולות: **"ייצא"** (TXT/DOCX), **"הזרק לחלון הפעיל"**, ו-checkbox **"שמור לצד הקובץ"** (רק ב-flow ההעלאה שיש בו source path).

---

## 9. גישות + המלצה

- **Approach A — "Cloud-batch first, reuse everything" (מומלץ):** Deepgram בקשה-אחת לארוך + Groq-chunked לקצר + מקומי כ-fallback. מנצל `samples_to_wav`/`ApiError`/`export.rs`/`inject_text` כמעט מילולית. מהיר לשחרר, גידול binary מינימלי, turnaround בשניות-דקות.
- **Approach B — "Local-first":** ברירת מחדל מקומי. אפס עלות/offline, אבל 90 דק' = 1.5–4 שעות ב-CPU → UX ברירת-מחדל גרוע. נכון כ-**אופציה**, לא כברירת מחדל.
- **Approach C — "ffmpeg sidecar":** כיסוי פורמטים מלא אבל 50–100MB/arch + GPL + חיכוך חתימה ב-macOS. נדחה כ-primary; fallback צר עתידי.

**המלצה: Approach A.**

---

## 10. סדר בנייה מדורג (MVP קודם)

**Phase 1 — MVP: העלאת קובץ, ענן + מקומי, בלי chunking**
1. `decode.rs` (symphonia+rubato, streaming, progress). לבדוק `.m4a` אמיתי מוקדם.
2. `whisper.rs`: **הסרת ה-timeout של 180ש'** + `transcribe_long(samples, lang, cancel, on_progress)` (ניתן-לביטול + progress). **הוקדם מ-Phase 3** כי תמלול מקומי של קובץ ארוך לא יכול לעבוד בלעדיו — וזו הדרישה המפורשת של הנרי.
3. `batch.rs` עם client 900ש' + `pick_batch_provider`; **שני מסלולים: Deepgram בקשה-אחת (ענן) + `whisper_long` (מקומי)** (מנצל `samples_to_wav`+`ApiError`+`WhisperEngine`).
4. command `transcribe_file` + פאנל העלאה React + **toggle "מהיר (ענן)" / "פרטי (במכשיר)"** + textarea editable + ייצוא (מנצל `export_history`) + הזרקה (מנצל `inject_text`) + ביטול (`cancel_batch`).
   → **לולאת "העלה קובץ → תמלול עברי (ענן או מקומי) → ערוך/ייצא/הזרק" שלמה**, בלי שינוי במקליט.

**Phase 2 — Groq chunking + שמירה-ליד-הקובץ + toggle דיוק**
4. `chunk.rs` (5דק'/3ש' overlap, stitch+de-dupe, progress per-chunk, 429 backoff).
5. command `save_transcript_next_to` + checkbox "שמור לצד הקובץ".
6. toggle `whisper-large-v3` ב-`BatchOpts`.

**Phase 3 — הקלטת פגישה ארוכה (record → stop → transcribe), ענן + מקומי**
7. הקלטת MVP על **RAM buffer עם תקרה מורמת (≤60 דק')** + מצב VAD-off + טיימר חי; מנצל `audio-level`.
8. תמלול ההקלטה דרך אותו `batch.rs` — **ענן או מקומי** (המסלול המקומי `whisper_long` כבר קיים מ-Phase 1; כאן רק מחברים אותו ל-flow ההקלטה). ברירת מחדל מקומי = `small`/`ivrit`, אזהרת משך (90 דק' = 1.5–4 שעות ב-CPU).

**Phase 4 — הקשחה (הצורות "הנכונות" מהמחקר)**
9. `recording_sink.rs`: ringbuf + hound writer-thread temp-WAV עם flush כל 5–10ש'; הרמת תקרה בטוחה; temp פרטי + cleanup-on-success/keep-on-failure.
10. Opus encode אופציונלי לפני העלאת Deepgram; fallback ffmpeg ל-HE-AAC בלבד.

---

## 11. בדיקות
- **decode.rs (unit):** WAV/MP3 → f32 16kHz mono באורך/ערכים צפויים; `.m4a` (AAC-LC) אמיתי; שגיאה ברורה על HE-AAC.
- **chunk.rs (unit):** `split_overlapping` גבולות נכונים + חפיפה; `stitch_dedupe` מסיר כפילויות בתפר; קלט ריק/קצר.
- **batch.rs (unit):** `pick_batch_provider` נתב נכון (ארוך→Deepgram, קצר/Groq-only→Groq, offline→local).
- **ידני (smoke):** קובץ mp3 קצר → ענן → טקסט נקי; פגישה 30 דק' → Deepgram בקשה אחת; ביטול באמצע; כשל רשת → הודעת עברית.

## 12. קבועים load-bearing לפעולה
- `whisper.rs`: להסיר `const TRANSCRIBE_TIMEOUT_SECS: u64 = 180;` + ה-`recv_timeout` שמחזיר "חרג מ-3 דקות" — מכשיל כל קובץ ארוך.
- `api_transcribe.rs`: `.timeout(Duration::from_secs(30))` → 900ש' ל-client ה-batch בלבד.
- `audio.rs`: `const MAX_RECORDING_CEILING_SECS: f32 = 3600.0;` להרים במצב ארוך.

## 13. עבודה עתידית
דיבור-לפי-דובר (diarization, נתמך ב-Deepgram), timestamps בייצוא, Opus encode, ffmpeg ל-HE-AAC, queue לכמה קבצים.
