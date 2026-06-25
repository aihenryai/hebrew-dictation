# Spec — Batch Transcription (קבצים + הקלטות ארוכות) for Hebrew Dictation

- **תאריך:** 2026-06-22
- **גרסת יעד:** v2.10.0 (minor — פיצ'ר חדש)
- **סטטוס:** Design rev2 — spec-review הוחל 2026-06-25 (ר' §14, מחייב/גובר). מוכן ל-writing-plans.
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
- ⚠️ **בוטל (ר' §14.1-A):** `language=multi` **לא** תומך בעברית (nova-3 multilingual = 10 שפות, עברית לא ביניהן). לכל batch בעברית: **תמיד `language=he`**.
- client עם `.timeout(900s)` + retry-with-backoff על 503/504.
- עלות ~$0.26/שעה (90 דק' ≈ $0.39); מפתחות חדשים = $200 קרדיט.

### Groq turbo (קבצים קצרים / fast)
- מסלול multipart קיים (`transcribe_groq_inner`), `model=whisper-large-v3-turbo`, `language=he`.
- **chunking חובה** מעל ~13 דק': חלונות **5 דק' + חפיפה 3ש'**, תמלול **סדרתי** (turbo ~216x realtime), **תפירה + de-dupe** של החפיפה. backoff על 429.
- toggle אופציונלי `whisper-large-v3` ("דיוק גבוה").

### Whisper מקומי (פרטיות/offline)
- **`state.full()` יחיד** על כל ה-buffer — בלי chunking ידני.
- **חובה:** להסיר את ה-timeout של 180ש', להפוך ל-cancellable, לחווט progress per-segment.
- ⚠️ **תוקן (ר' §14.1-B):** "ivrit small" **לא קיים** — יש רק `ivrit-large-v3-turbo`. ברירת מחדל מקומית ל-batch = **המודל שכבר נבחר להכתבה** (`settings.preferred_model`), עם אזהרת משך (90 דק' = 1.5–4 שעות ב-CPU). `small` גנרי כ-opt-in "מהיר, דיוק עברית נמוך". (הערה: turbo אכן איטי יותר מ-small ב-CPU — ציר מהירות נפרד מציר דיוק.)

### כלל המוצר (UI)
בחירה דו-צירית: **"מהיר (ענן, המפתח שלך)"** מול **"פרטי (במכשיר, איטי)"**. נתב קטן `pick_batch_provider`: פגישה ארוכה בענן → Deepgram; קליפ קצר / רק מפתח Groq → Groq; רגיש/offline → מקומי.

---

## 4. פענוח קבצים (פיצ'ר ההעלאה)

**החלטה: Rust טהור — `symphonia` (decode) + `rubato` (resample) + helper של ~30 שורות.** לא ffmpeg.
- נימוק (מחקר): הכי קל ועמיד, אפס התקנה חיצונית, בלי sidecar, בלי GPL, תוספת מאות KB מול 50–100MB/arch ל-ffmpeg; אותה צורה כמו ה-pipeline הקיים (whisper-rs צריך f32 16kHz mono).
- **Cargo:** `symphonia = { version = "0.6", features = ["mp3","aac","isomp4","alac","vorbis","ogg","wav","flac"] }`, `rubato = "0.15"`. ⚠️ AAC/MP4 כבויים כברירת מחדל — שכחת `aac`+`isomp4` = `.m4a` נכשל בשקט בעוד WAV/MP3 עובדים. **לבדוק עם `.m4a` אמיתי מ-iPhone מוקדם.**
- **pipeline:** decode packets → **המרה ל-f32 דרך `SampleBuffer<f32>`** (פלט הדקודר תלוי-codec: WAV 16-bit→S16, FLAC→S32, MP3/AAC→F32 — אסור להניח f32, ר' §14.2-K) → **mono mixdown קודם, אז resample native→16000** בבלוקים קבועים (frames בגודל קבוע + flush/zero-pad ל-frame האחרון אחרת ה-tail נופל, ר' §14.2-L). **streaming packet-by-packet** (לא materializing) — לעולם לא לאסוף את כל ה-native rate: buffer של 90 דק' 48kHz stereo = ~2GB, רק ה-Vec<f32> הסופי ב-16k mono (~345MB) materialized.
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

---

## 14. תיקוני spec-review (2026-06-25) — מחייב, גובר על §1–§13

> סקירת spec רב-סוכנית (5 עדשות: דיוק-קוד, היתכנות Rust, API ספקים, שלמות/UX, אדוורסרי) מול הקוד האמיתי + תיעוד הספקים. **Verdict: `minor_fixes_then_plan`.** כל טענות הסימבולים אומתו מול הקוד. במקום שסעיף זה סותר טקסט קודם — **סעיף זה מנצח.**

### 14.1 קריטי (חובה לפני תכנון)

**(A) Deepgram `language=multi` לא תומך בעברית — להסיר code-switching.**
nova-3 multilingual = 10 שפות בלבד (en/es/fr/de/hi/ru/pt/ja/it/nl), עברית לא ביניהן (אומת בתיעוד Deepgram 2026-06-25). לכל batch בעברית: **תמיד `language=he` מונולינגואלי.** מבטל את `multi` ב-`BatchOpts.language`. לתקן הערה ישנה: `api_transcribe.rs:210` + `VALID_LANGUAGES` (:250-252). code-switching עתידי → רק המסלול המקומי (ivrit), לא Deepgram.

**(B) מודל מקומי 'ivrit small' לא קיים.**
`model.rs` מכיל מודל ivrit אחד בלבד: `ivrit-large-v3-turbo`. 'small' גנרי = 'מאוזן', לא מכוון-עברית. **ברירת מחדל מקומית ל-batch = `settings.preferred_model`** (המודל שכבר נבחר להכתבה), עם אזהרת משך מפורשת. `small` גנרי = opt-in "מהיר, דיוק עברית נמוך". להסיר כל "ivrit small" מ-§3/§10. בקופי ה-UI: ציר מהירות נפרד מציר דיוק-עברית.

**(C) הרשאת `dialog:allow-open` חסרה.**
`capabilities/default.json` מעניק רק `dialog:allow-save`. להוסיף `"dialog:allow-open"` כשלב מפורש ב-Phase 1. ה-path נפתח בצד Rust ע"י symphonia → **לא** צריך `fs:allow-read-file`, רק dialog-open.

**(D) מודל קונקרנטיות — חובה להגדיר (אחרת רגרסיה להכתבה הקצרה).**
`AppState.whisper_engine` = `std::sync::Mutex` (lib.rs:28); `transcribe_local` נועל לכל הריצה (lib.rs:229-232). batch מקומי 90 דק' (1.5-4 ש') שמחזיק את ה-Mutex → חוסם כל Alt+D, `load_whisper_model`, `delete_model` לשעות. אינווריאנטים מחייבים:
- `transcribe_long` **לא** מחזיק את ה-Mutex של `whisper_engine` לאורך `state.full()`. להוציא `Arc<WhisperContext>` מה-Mutex (`create_state` נותן state לכל ריצה) ולהריץ בלי נעילת AppState; *או* gate מאחורי `batch_in_progress: AtomicBool` + הכתבה קצרה מציגה "תמלול ארוך פעיל — המתן".
- `stop_long_recording_and_transcribe`: לנעול את ה-recorder **רק** ל-stop+drain, לשחרר, ואז decode/resample/transcribe **מחוץ** לנעילה. command **async** עם עבודת CPU כבדה ב-`spawn_blocking` (כבר בשימוש streaming.rs:156). אחרת = חזרה לבאג #4 (freeze של הסרגל הצף).

**(E) ביטול — מעוצב רק למקומי; הענן והמכניקה חסרים.**
- ענן: `.timeout(900s)` לא מבטל. צריך **tokio cancellation token / abort handle** שה-orchestrator עושה עליו `select!`, כך ש-`cancel_batch` מפיל מיידית את ה-future של ה-HTTP.
- מקומי: `FullParams::set_abort_callback_safe(move || cancel.load())` (אומת ב-whisper-rs 0.16) — להסיר את `recv_timeout` לגמרי. הערה: `set_progress_callback_safe` = **אחוז כולל (i32)**, לא per-segment — לתקן ניסוח §7/§10.
- סמנטיקת ביטול לכל שלב: upload → drop request, בלי פלט; מקומי → abort callback מחזיר true; temp files → למחוק partial בביטול יזום (לשמור רק על crash); ייצוא → temp path + **atomic-rename** כדי שביטול לא ישאיר `.he.txt`/`.docx` חתוך ליד המקור.

**(F) סימבולים ל-reuse הם private — משימת `pub(crate)` מפורשת.**
`samples_to_wav` (:7), `transcribe_groq_inner` (:155), `transcribe_deepgram_inner` (:203), `classify_status` (:125), `classify_request_error` (:115) — כולם private. ל-§8: לחשוף `pub(crate)` (או submodule משותף). הערה: `transcribe_groq_inner` **כבר** מקבל `&[f32]` — ה-refactor האמיתי = הזרקת `Client`+model+timeout, לא חתימת ה-samples.

### 14.2 חשוב (לטפל במהלך התכנון)

**(G) תקרת 10 הדקות של Deepgram = wall-clock עיבוד בצד שרת, לא duration headroom.** יש כשלי 503/504 מתועדים על קבצים גדולים תחת עומס (workaround רשמי = callback URL, נדחה בצדק). המסלול הסינכרוני-יחיד **כשיר אבל שביר**; retry עיוור מעלה מחדש את כל ה-~173MB. על 503/504 חוזרים → **fallback ל-client-side chunking** (reuse של `chunk.rs` של Groq), לא רק retry. לשקול Opus דחוס (Phase 4) מוקדם יותר למקרה 90-הדקות.

**(H) שגיאות batch לא מתאימות ל-`ApiError`.** `ApiError` transport-centric. להגדיר `BatchError` נפרד (batch.rs/decode.rs) עם Hebrew Display: `DecodeFailed`, `UnsupportedCodec(HE-AAC)`, `DiskFull`, `Cancelled`, + `FromApi(ApiError)`. "Cancelled" יוצג כ-"בוטל" רגוע, לא כ-toast שגיאה. מחרוזת עברית לכל וריאנט.

**(I) `save_transcript_next_to` נושא chrome של history-export + overwrite לא מוגדר.** `write_txt`/`write_docx` מקודדים כותרת "היסטוריית תמלול" + תוויות "פריט N" (export.rs:31-35,93-97). להוסיף **מצב כתיבה ללא-כותרת** ל-export.rs לפלט one-shot. overwrite: suffix `.he.2.txt` או confirm. `save_transcript_next_to` = כתיבת fs **סינכרונית ישירה, בלי dialog**; רק כפתור "ייצא" משתמש ב-dialog של export_history.

**(J) §5: claim של "תיקון lock-contention" שייך ל-Phase 4 בלבד.** ה-MVP (§5 "פישוט", §10 Phase 3) משתמש מחדש ב-RAM buffer הקיים שבו ה-CPAL callback (audio.rs:239) וה-VAD thread נועלים `samples`; ב-60 דק' זה Vec של ~115-230MB, ו-`extend_from_slice` יכול realloc+memcpy של כל ה-buffer תחת נעילת האודיו → **מחמיר** את ה-contention. לתחום את ה-claim ל-Phase 4; guardrail ל-MVP: `Vec::with_capacity` בגודל התקרה (או `Vec<Vec<f32>>` מחולק); VAD צריך רק את ה-tail הקטן, לא את כל ה-buffer.

**(K) §4: פלט symphonia תלוי-codec — המרה דרך `SampleBuffer<f32>`.** (מומש ב-§4 inline.) הערה: **symphonia 0.6 קיים ותקין** (יצא 15.5.2026) — ה-pin תקין, חשש אחד הסוקרים היה שגוי.

**(L) §4: rubato — flush/latency + pin.** block resamplers (FftFixedIn/SincFixedIn) נושאים latency פנימי + צריכים frames בגודל קבוע + flush בסוף-stream, אחרת ה-tail (~מאות ms = משפט אחרון ב-90 דק') נופל. לבחור API מפורש, לציין "frames בגודל קבוע → flush/zero-pad ל-frame האחרון". טסט decode.rs: אורך פלט ≈ `input_secs*16000` בטולרנס latency. אופציונלי: pin ל-0.16.x.

**(M) `inject_text` = char-by-char WM_CHAR — לא פרקטי ל-90 דק'.** (injector.rs:8-14 דרך enigo.text(), לא clipboard; ה-`InjectionMethod` מתעלם). למסלול inject של batch: paste אמיתי דרך clipboard (`arboard` כבר dependency, Cargo.toml:30) לטקסטים גדולים, או cap/warn. לציין ב-§8.

**(N) ולידציית קלט ל-batch — והגארד mic-silence לא ידלוף להעלאות.** מסלול `transcribe_file` עוקף את `transcribe`, לא יורש `is_effectively_silent`/`MIN_TRANSCRIBE_SAMPLES`. ולידציה ייעודית: לדחות buffer ריק/קרוב-לאפס עם "הקובץ ריק או פגום"; **לא** להשתמש בהודעת "בדוק הרשאת מיקרופון" במסלול ההעלאה.

**(O) §6: כשל chunk + תפר de-dupe יכולים להשמיט דקות בשקט.** אם chunk אמצעי ממצה backoff (429/timeout), `stitch_dedupe` יתפור ניצולים עם חור שקט. chunk שנכשל אחרי backoff → **לבטל את כל ה-batch עם שגיאה עברית** (או marker גלוי "[קטע X חסר]"), לא לתפור סביב החור. fallback ל-stitch כשאין overlap תואם (חיתוך offset קבוע באמצע ה-overlap). טסט לתפר ללא-התאמה, לא רק כפילות נקייה.

**(P) §11 בדיקות — 2 מצבי-כשל בסיכון גבוה:** (1) ביטול תוך כדי בקשת **ענן** — abort תוך ~1ש' + "בוטל"; (2) ריצה מקומית end-to-end 60 דק' + מדידת peak RSS + progress events זורמים (בלי freeze) — דרישת המפתח של הנרי; (3) `.m4a` אמיתי מ-iPhone end-to-end (decode→מקומי **וגם** ענן→textarea), לא רק assertion של decode.

**(Q) שתי נקודות-חנק לתקרת ההקלטה + דליפת state.** `MAX_RECORDING_CEILING_SECS=3600` נאכף ב-clamp (audio.rs:109), ו-lib.rs:1116-1121 מקודד 3600.0 שנית. מסלול `start_long_recording` ייעודי עם clamp גבוה משלו, שמשחזר את ה-max של ההכתבה ב-stop — כך ששני המסלולים לא ידליפו state; תקרת 3600 נשמרת להכתבה רגילה.

### 14.3 רשות (שיפורים)
- `BatchOpts.mode` **ברירת מחדל מ-`settings.TranscriptionMode`** (Local→local; Api/AutoFallback→cloud) + provider מ-`api_provider`; ה-toggle = override לכל batch.
- Groq: הקוקבוק הרשמי ממליץ chunks של 10 דק' (PCM16 ≈ 19.2MB, מתחת ל-25MB) — מחצית הבקשות/התפרים מול 5 דק'. 5 דק' = שמרנות מודעת.
- עלות Deepgram: PAYG עכשווי ~$0.46/שעה (לא $0.26). השפעה נמוכה (BYO-key), אבל הערכה כנה.
- §5 peak RAM: `samples_to_wav` בונה את כל ה-WAV ב-RAM → למקרה ענן 90-דק' peak = f32 (~115MB) + WAV bytes (~173MB) = ~290MB transient. לשחרר את ה-f32 לפני בניית ה-WAV.
