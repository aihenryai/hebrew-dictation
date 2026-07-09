# Spec — לכידת אודיו-מערכת (System Audio Capture)

- **תאריך:** 2026-07-09
- **סטטוס:** design מאושר (brainstorming) → spec review (סבב 2) → תוכנית מימוש
- **קשר:** משלים את diarization לחזון "תמלול פגישות"; ממשיך את דפוס ה-batch של SRT export.

## 1. מטרה
לתמלל אודיו שאינו מהמיקרופון — "הצד השני" של שיחת Zoom/Meet/Teams, או תוכן שמתנגן במחשב — דרך **WASAPI loopback** ב-Windows. במצב שיחה כל צד מתויג בנפרד ("אני" מול "הצד השני") בלי ניחוש.

## 2. מטרות / לא-מטרות
**מטרות (v1):**
- שלושה מצבי הקלטה: `Mic` (קיים), `System` (loopback בלבד), `Call` (מיק + מערכת).
- מצב Call מפיק תמלול מופרד: ערוץ 0 = "אני", ערוץ 1 = "הצד השני".
- **batch בלבד**: הקלטה → עצירה → תמלול.

**לא-מטרות (במפורש בחוץ מ-v1):**
- תמלול בזמן-אמת / כתוביות חיות לשיחה.
- בחירת מכשיר פלט (רק מכשיר ה-render הדיפולטי).
- whisper מקומי למצב Call (multichannel = Deepgram בלבד).
- loopback ב-macOS/Linux (Windows בלבד).

## 3. מצבי הקלטה
פרמטר חדש **`source: RecordingSource { Mic, System, Call }`** נשלח לפקודת ההקלטה. ברירת מחדל `Mic` (התנהגות קיימת, אפס רגרסיה).
> ⚠️ שם השדה הוא `source` **ולא** `mode` — כדי לא להתנגש עם `BatchOpts.mode` הקיים (`"cloud"`/`"local"`, `batch.rs:10`) שבוחר את מנוע-התמלול. `BatchOpts.mode` ממשיך לבחור cloud/local עבור `Mic`/`System`.

- **Mic** — `AudioRecorder` הקיים (cpal), מונו. ללא שינוי. עובר במסלול הקיים (`stop_batch_recording_to_file` → `transcribe_file`).
- **System** — `SystemAudioRecorder` חדש (wasapi loopback), מונו → WAV מונו → מסלול `transcribe_file` הקיים (diarization אופציונלי אם בתוכן כמה דוברים).
- **Call** — שני המקליטים בו-זמנית → `interleave_stereo` → מסלול-בייטים סטריאו ייעודי (§4.3) → Deepgram `multichannel=true`. **Call כופה cloud/Deepgram** גם אם `BatchOpts.mode="local"` (ראה §6), כי multichannel הוא Deepgram-only.

## 4. ארכיטקטורה ורכיבים

### 4.1 `SystemAudioRecorder` (חדש, `system_audio.rs`, `#[cfg(target_os = "windows")]`)
מעל ה-crate `wasapi`. לוכד את מכשיר ה-render הדיפולטי דרך loopback → resample מהקצב המקורי (בד"כ 48kHz/44.1kHz) ל-**16kHz מונו** → `Vec<f32>`. חושף `start()` / `stop() -> Vec<f32>` בצורה מקבילה ל-`AudioRecorder`.
> **Resample:** יש בקודבייס שתי טכניקות — אינטרפולציה לינארית ידנית (`audio.rs:611` `resample`, זה מה שהמיק משתמש) ו-`rubato` (`decode.rs`, לפענוח קבצים). לבחור אחת; **לא** "כמו המיק דרך rubato" (המיק אינו rubato).
> **מיקום:** שדה חדש נפרד ב-`AppState` (למשל `system_recorder`), עצמאי מ-`state.recorder` (המיק). ה-re-entrancy guard "הקלטה כבר פעילה" (`audio.rs:135`) הוא per-recorder.

### 4.2 `interleave_stereo(mic: &[f32], system: &[f32]) -> Vec<f32>` (טהור, `audio.rs`)
מיישר לאורך המקסימלי (padding בשקט לקצר), משזר `L=mic`/`R=system` → מערך משולב (אורך `= 2 * max_len`).

### 4.3 מסלול-בייטים סטריאו — הליבה (עוקף את מסלול המונו!)
> 🔴 **קריטי:** Call **אסור** שיעבור דרך `transcribe_file` → `decode_file_to_16k_mono` (`decode.rs:100-108`) — הוא ממזג **כל** קובץ למונו והורס את הפרדת-הערוצים לפני Deepgram. גם `samples_to_wav` הקיים (`api_transcribe.rs:7`) מקודד `num_channels = 1` קשיח, אז הוא לא מתאים לגוף-הבקשה.
מסלול ייעודי (backend-only, בלי לחצות samples ב-IPC — שני המקליטים ב-backend):
- **`samples_to_wav_stereo(interleaved: &[f32], 16000) -> Vec<u8>`** — פונקציה **נפרדת** (לא הכללה של `samples_to_wav` עם `channels` — כדי לא לגעת בכל קוראי-המונו הקיימים: groq/deepgram single+batch + טסט decode; פחות blast radius). בונה גוף WAV דו-ערוצי בזיכרון.
- **`transcribe_deepgram_multichannel(client, stereo_wav_bytes, key, lang) -> Result<(String, Vec<crate::srt::TimedSegment>), ApiError>`** — מחזיר `(text מתויג, segments ממוזגים)`, במקביל בדיוק ל-`transcribe_deepgram_batch`. **URL:** אותו בסיס כמו `transcribe_deepgram_batch` (`api_transcribe.rs:263-266` — `model=nova-3&language={lang}&smart_format=true&punctuate=true`) + `&multichannel=true`, ו**בלי** `diarize` ובלי `paragraphs` (ה-`text` נבנה מהסגמנטים, §4.4, ולא מה-transcript השטוח).

### 4.4 פרסור multichannel + הטבעת ערוץ
לכל ערוץ בנפרד: `parse_deepgram_words(&body["results"]["channels"][i]["alternatives"][0])` (ממחזר את הפרסר הקיים, `api_transcribe.rs:316-334`).
> ⚠️ מכיוון ש-`diarize` כבוי, הפרסר מחזיר `speaker: None` לכל המילים. לכן **מטביעים מפורשות** את אינדקס-הערוץ מיד אחרי הפרסור: `map` על ה-`Vec<TimedWord>` שמחזיר, קובע `speaker = Some(i)`, ואז `chunk_words_to_cues`. (הטבעה על `TimedWord` לפני ה-chunk.)
- **מיזוג כרונולוגי:** שני מערכי ה-`TimedSegment` ממוזגים לפי `start_ms` (שניהם על שעון קובץ-הסטריאו האחד — אז המיזוג מדויק בתוך הקובץ).
- **בניית ה-`text`** (הפלט להזרקה/העתקה/TXT/DOCX): מהסגמנטים הממוזגים, כל שורה עם תווית-הצד — `"אני: <טקסט>"` / `"הצד השני: <טקסט>"` — בסדר כרונולוגי. זהו ה-`text` שנכנס ל-`TranscribeFileResult` (§4.6). ⚠️ **לא** להשתמש ב-`transcript`/`paragraphs` השטוח של Deepgram — ב-multichannel הוא per-channel, וברירת-המחדל (ערוץ 0 בלבד) תאבד בשקט את "הצד השני".

### 4.5 תיוג ב-render — interface מפורש
`render_srt` היום מקודד "דובר N:" ומתייג **רק** כש-`distinct_speakers >= 2` (`srt.rs:108-122`). שינוי החתימה:
```
render_srt(files: &[Vec<TimedSegment>], style: SpeakerLabelStyle) -> String
enum SpeakerLabelStyle { Diarization, Call }
```
- `Diarization` — התנהגות קיימת בדיוק (מתייג רק אם ≥2 דוברים, `"דובר {n+1}:"`). כל הקוראים הקיימים מעבירים `Diarization` → פלט ללא שינוי.
- `Call` — **תמיד מתייג** (עוקף את גייט ה-≥2, כי בשיחה שבה צד אחד שתק יש דובר יחיד אבל עדיין צריך "אני/הצד השני"). `speaker 0 → "אני:"`, `1 → "הצד השני:"`.

### 4.6 Orchestration (ב-`lib.rs`)
פקודת ההקלטה מקבלת `source`.
- **`System`** — מנתב ל-`system_recorder` (לא ל-`state.recorder` שהוא המיק) → WAV מונו → מסלול `transcribe_file` הקיים.
- **`Call`** — מפעיל את שני המקליטים; ב-stop עוצר את שניהם, קורא `interleave_stereo`, מריץ את גארד ה-silence הקיים (`is_effectively_silent`) על ה-buffer **המשולב** *כאן ב-orchestration* — כי Call **לא** עובר דרך `stop_batch_recording_to_file`/`run_transcribe_file` שבהם הגארדים הקיימים יושבים (`lib.rs:595, 423`) — בונה גוף סטריאו (§4.3), קורא `transcribe_deepgram_multichannel`, ו**עוטף את `(text, segments)` ל-`TranscribeFileResult`** (`lib.rs:354-362`) כדי שכל צרכני-הפלט הקיימים (הזרקה/העתקה/TXT/DOCX/SRT) יעבדו ללא שינוי.
- **Frontend:** בורר `source` לפני הקלטה (dropdown/toggle).

## 5. זרימת נתונים (מצב Call)
```
[mic:    cpal AudioRecorder]  ─┐
                               ├─► interleave_stereo → samples_to_wav_stereo (in-memory, 2ch/16kHz)
[system: wasapi loopback]     ─┘                │
                                                ▼   (עוקף את transcribe_file / decode_file_to_16k_mono
                                                     שממזגים למונו)
              transcribe_deepgram_multichannel  (POST body = stereo WAV, &multichannel=true, ללא diarize)
                                                │
              parse channels[0] → stamp speaker=Some(0)  ("me")
              parse channels[1] → stamp speaker=Some(1)  ("them")
                                                ▼
        merge segments by start_ms → render_srt(.., Call) → "אני:" / "הצד השני:"
```

## 6. טיפול בשגיאות ומגבלות
- **Call + מנוע-תמלול:** `Call` תמיד משתמש ב-Deepgram (multichannel). אם `BatchOpts.mode="local"` אבל **קיים** מפתח Deepgram → Call כופה cloud להקלטה הזו (שקוף). אם **אין** מפתח Deepgram כלל → guard שמחזיר שגיאה מנחה **לפני** ההקלטה: "מצב שיחה דורש מפתח Deepgram". לא קורס.
- **כשל loopback** (wasapi bind/capture): המצב לא זמין + הודעה; המיק ממשיך לעבוד רגיל.
- **מערכת שקטה** (רק צד אחד דיבר): גארד ה-silence הקיים `is_effectively_silent` רץ על ה-buffer ה**משולב** (לא per-channel) — אז שיחה שבה רק צד אחד דיבר **לא** נחסמת (ל-buffer המשולב יש תוכן). אין בדיקת silence per-channel.
- **לא-Windows**: `System`/`Call` לא זמינים (cfg + ה-UI מסתיר).
- **מגבלת יישור (v1)**: היישור מניח שהמיק והמערכת התחילו יחד ורצים באותו קצב. כל צד מתומלל נכון (Deepgram נותן timestamps על שעון קובץ-הסטריאו האחד → המיזוג הכרונולוגי מדויק בתוך הקובץ); רק סנכרון-הלכידה עצמו עלול לסבול drift קל בשיחות ארוכות מאוד. שיפור (יישור לפי גלאי-התחלה) — עתידי.

## 7. אסטרטגיית בדיקות
- **Unit (רץ על Windows, ללא אודיו):**
  - `interleave_stereo` — אורכים שווים, אורך לא-שווה (padding נכון), קלט ריק.
  - **פרסור+הטבעה multichannel** — fixture JSON עם `channels[0]/[1]` נושאי `words` **ללא** שדה `speaker` (diarize כבוי) → מאמת שהווריאנט מטביע `speaker: Some(0)`/`Some(1)` נכון (זהו הצעד המפורש מ-§4.4, לא ה-None שהפרסר מחזיר).
  - **render של Call** — סגמנטים משני ערוצים, כולל מקרה שבו לערוץ אחד אין סגמנטים (דובר יחיד) → מאמת שתמיד מתייג "אני:"/"הצד השני:" (עוקף את גייט ה-≥2).
  - **בניית `text` ל-Call** — סגמנטים ממוזגים משני הצדדים → `text` עם שתי התוויות בסדר כרונולוגי (מוודא ש"הצד השני" לא נאבד).
  - **guard ה-Call/cloud** — Call + אין מפתח → שגיאה; Call + יש מפתח (גם אם mode=local) → עובר דרך cloud.
- **ידני (Windows, אודיו אמיתי):** לכידת loopback בפועל — להשמיע וידאו/שיחה ולוודא שהערוץ נלכד ומתומלל.

## 8. סיכונים
- **אמינות `wasapi` loopback** — המניע לבחירה בו על פני cpal-loopback (הפכפך). לאמת בהרצה ידנית מוקדמת על Windows.
- **drift שעוני-לכידה** בשיחות ארוכות (ראה §6) — מקובל ל-v1, מתועד.
- **קצב לכידה מקורי משתנה** בין מכשירי פלט (48k/44.1k) → resample ל-16k מטפל.

## 9. נגיעות בקוד קיים (blast radius)
- **`system_audio.rs`** — חדש, Windows-only (`SystemAudioRecorder`).
- **`audio.rs`** — `interleave_stereo` (הוספה טהורה); אולי מיחזור helper ה-resample. המיק ללא שינוי.
- **`api_transcribe.rs`** — `samples_to_wav_stereo` (פונקציה נפרדת, לא נוגעים בקוראי-המונו) + `transcribe_deepgram_multichannel` (מחזיר `(text, segments)`) + פרסור/הטבעה per-channel (ממחזר `parse_deepgram_words`) + בניית ה-`text` המתויג.
- **`decode.rs`** — **לא** משתנה, אבל Call **עוקף** אותו במפורש (מתועד כדי שהמתכנן לא ינתב את Call דרך `transcribe_file`).
- **`srt.rs`** — `render_srt` מקבל `SpeakerLabelStyle`; Call = תמיד מתייג.
- **`lib.rs`** — פקודת הקלטה מודעת-`source` + orchestration של שני המקליטים + guard ה-Call/cloud; שדה `system_recorder` ב-`AppState`.
- **`settings.rs` / Frontend** — בורר `source`.
- **`Cargo.toml`** — תלות `wasapi` (Windows-only target).
