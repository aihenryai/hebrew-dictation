# Spec — מצב הקלטה רביעי + ארגון מחדש של בורר המקורות (Recording Modes UX)

- **תאריך:** 2026-07-12
- **סטטוס:** design מאושר (brainstorming, כל 4 השאלות סגורות מול הנרי) → spec review → תוכנית מימוש
- **קשר:** ממשיך ישירות את `2026-07-09-system-audio-capture-design.md` (מצבי Mic/System/Call). מוסיף מצב פגישה מקומי ומְאַרגן את ה-UI לבהירות מרבית — עדיפות #1 מפורשת של הנרי: "מאוד מאוד חשוב שחוויית המשתמש תהיה מאוד מאוד ברורה".

## 1. מטרה
שתי מטרות משולבות:
1. **מצב הקלטה רביעי — "פגישה פרטית במכשיר":** מיק + אודיו-מערכת מעורבבים ל**תמלול מונו אחד, מקומי (whisper)**, בלי הפרדת דוברים. סוגר פער פרטיות: מצב השיחה הקיים הוא ענן-בלבד (multichannel = Deepgram-only), אז משתמש פרטיות-מודע לא יכול לתמלל פגישה בלי להעלות אודיו. המחיר: אובדן "מי אמר מה".
2. **ארגון מחדש של בורר המקורות** לשתי קבוצות ברורות + הפיכת בורר ענן/מקומי למותנה-הקשר, כדי לחסל את הסתירה שבה שני בוררי-הצירים (מקור × מנוע) מתנגשים במצבי הפגישה.

## 2. מטרות / לא-מטרות
**מטרות (v1):**
- מצב הקלטה רביעי: `CallLocal` — מיק+מערכת → `mix_to_mono` → WAV מונו → whisper מקומי כפוי.
- בורר מקורות בשתי קבוצות עם כותרות: "הקלטה רגילה" (מיקרופון · אודיו מערכת) ו-"פגישות" (עם-זיהוי-דוברים · פרטית-במכשיר).
- בורר ענן/מקומי (`batchMode`) מוצג **רק** לקבוצת "הקלטה רגילה"; מוסתר בשני מצבי הפגישה.
- מיסוד כרטיסי הפגישה לפי תועלת ("עם זיהוי דוברים" / "פרטית במכשיר"), המנגנון בשורת-התיאור.
- שינוי-שם ה-enum: `Call` → `CallCloud`, הוספת `CallLocal` (סימטרי, מתעד את עצמו).
- Guard לפני הקלטה למצב `CallLocal`: מודל whisper מקומי חייב להיות מורד, אחרת שגיאה מנחה **לפני** ההקלטה (סימטרי ל-guard מפתח-Deepgram של `CallCloud`).

**לא-מטרות (במפורש בחוץ מ-v1):**
- הפרדת דוברים / "אני / הצד השני" במצב המקומי (אין multichannel מקומי — זו כל הנקודה של המצב).
- בחירת ענן ל-`CallLocal` (המצב מקומי-כפוי; מי שרוצה ענן בוחר "עם זיהוי דוברים").
- בורר שפה נפרד לפגישה מקומית (משתמש ב-`"he"` הקיים כמו שאר מסלול-הקובץ).
- macOS/Linux לשני מצבי הפגישה (WASAPI = Windows בלבד; ה-UI מסתיר).
- חשיפת דוברים בתצוגת-התמלול ב-UI (כמו שהוגדר ב-spec הקודם).

## 3. מבנה ה-UI (בורר המקורות)

הבחירה הראשית הופכת ל**מקור**; בורר ענן/מקומי יורד לתת-בחירה מותנית.

### 3.1 בוררי מקור — שתי קבוצות עם כותרות
```
── הקלטה רגילה ──────────────────
  🎙 מיקרופון          הקול שלכם בלבד
  🔊 אודיו מערכת       מה שמתנגן במחשב            (Windows בלבד)

── פגישות ───────────────────────  (Windows בלבד — כל הקבוצה)
  📞 פגישה — עם זיהוי דוברים   אתם + הצד השני, כל אחד בנפרד · מתומלל בענן (Deepgram)
  🔒 פגישה — פרטית במכשיר      אתם + הצד השני יחד · נשאר במחשב, בלי הפרדת דוברים
```
- הכותרות (`הקלטה רגילה` / `פגישות`) הן מפרידים ויזואליים; **ללא** קידומת "קבוצה א׳/ב׳" (רעש מיותר). מעוצבות ב-**inline styles** (עקבי עם דפוס ה-note הקיים ב-`App.tsx:2477-2482`) — **בלי קובץ CSS חדש**, כדי לשמר את "no new CSS" של בורר-המקורות הקיים.
- כרטיסי `אודיו מערכת`, `פגישה — עם זיהוי דוברים`, `פגישה — פרטית במכשיר` — Windows בלבד (`IS_WINDOWS`), כמו היום.
- מיפוי לערכי `RecordingSource` שנשלחים ל-backend: `mic` / `system` / `callcloud` / `calllocal`.

### 3.2 בורר ענן/מקומי — טוגל קליל למעלה (עודכן 2026-07-13 אחרי פידבק UX חי)
> **הגרסה הראשונה** עשתה את בורר ענן/מקומי "מותנה-הקשר" — כרטיסים גדולים שמוצגים רק ל-mic/system ונעלמים לפגישות. הנרי בדק את זה חי ומצא שזה מבלבל: הכרטיסים הגדולים ריחפו ליד קבוצת "פגישות" ונראו כמו ציר שלישי, ושני כרטיסי 🔒 ("פרטי—מכשיר" ו"פגישה — פרטית במכשיר") נראו כפילות. **התיקון (נבחר ע"י הנרי):** להפוך את ענן/מקומי ל**טוגל segmented קטן וקליל בראש המסך**, בעל שפה ויזואלית שונה מכרטיסי-המקור הגדולים, כך ש"מה מקליטים" (כרטיסים) ו"איך מתמללים" (טוגל) נקראים כשני צירים נפרדים.
- **טוגל `engine-toggle`** (CSS חדש ב-`App.css`, לא inline — מדובר ברכיב אמיתי): `מנוע תמלול: [☁ ענן] [💾 מקומי]`. אייקון `💾` למקומי (לא 🔒) כדי לא להתנגש עם כרטיס "פגישה — פרטית במכשיר". שולט ב-`batchMode` (cloud/local) עבור mic/system בלבד.
- **דימום לפגישות:** כשנבחר מקור פגישה (`callcloud`/`calllocal`) הטוגל מקבל class `is-disabled` (opacity 0.4 + `pointer-events:none`) והכפתורים `disabled` — כי הפגישה קובעת את המנוע בעצמה. כותרת "פגישות" נושאת את ההערה `· קובעות מנוע בעצמן`. זה מסלק את הסתירה **בלי** להסתיר את הבורר (הנרי העדיף טוגל-מעומעם על היעלמות/הופעה).
- הערת השקיפות הישנה ("שיחה מתומללת תמיד בענן…") נמחקה כבר; המידע חי בשורת-התיאור של כרטיס "עם זיהוי דוברים".

### 3.3 סדר הרינדור במסך
1. **טוגל מנוע-תמלול** (`engine-toggle`) — למעלה, מיד אחרי הכותרת; מתעמעם לפגישות (§3.2).
2. בוררי המקור (שתי הקבוצות) — הבחירה הראשית, כרטיסים גדולים.
3. כפתורי הפעולה (בחר קבצים / הקלט ותמלל) — ללא שינוי.

## 4. ארכיטקטורה ורכיבים (Backend)

מצב `CallLocal` הוא הקוד היחיד באמת-חדש. הוא **לא** עובר במסלול-השיחה האינליין (`stop_call_recording`); הוא חוזר ל**מסלול-הקובץ המונו הקיים** (`stop_batch_recording_to_file` → frontend `transcribe_file`).

### 4.1 `RecordingSource` — שינוי-שם + ערך חדש (`batch.rs`)
```rust
#[serde(rename_all = "lowercase")]
pub enum RecordingSource {
    #[default] Mic,     // "mic"        — cpal, מונו (קיים)
    System,             // "system"     — WASAPI loopback, מונו (קיים)
    CallCloud,          // "callcloud"  — מיק+מערכת → סטריאו → Deepgram multichannel (= Call הקיים, שינוי-שם)
    CallLocal,          // "calllocal"  — מיק+מערכת → mix_to_mono → whisper מקומי (חדש)
}
```
> **אין מיגרציה:** `recordSource` הוא state של קומפוננטה בלבד (`App.tsx:435`, `useState("mic")`), **לא** נשמר ל-settings.json. לכן שינוי-השם והמחרוזות בטוח — הפרונט והבקאנד משתחררים יחד. הטסטים תופסים כל mis-route.

### 4.2 `recorders_for_source` (`batch.rs`) — טבלה מעודכנת
```
Mic       → (true,  false)
System    → (false, true)
CallCloud → (true,  true)    // = Call הקודם
CallLocal → (true,  true)    // חדש — גם שני המקליטים
```
> מסלול ה-**start** (`start_recorders_for_source`, `lib.rs:627`) כבר טבלאי לחלוטין דרך `recorders_for_source` → **אפס שינוי שם**. הפעלת שני המקליטים ל-`CallLocal` קורית אוטומטית, כולל rollback-על-כשל-loopback והגייט Windows-only הקיימים.

### 4.3 `mix_to_mono(mic: &[f32], system: &[f32]) -> Vec<f32>` (טהור, חדש, `audio.rs`)
ליד `interleave_stereo` (`audio.rs:641`). ממצע דגימה-דגימה את שני באפרי ה-16k-mono, מרפד את הקצר יותר בשקט (`0.0`):
- `out.len() == max(mic.len(), system.len())`.
- `out[i] = (mic.get(i).unwrap_or(0.0) + system.get(i).unwrap_or(0.0)) * 0.5`.
> **מיצוע (×0.5) ולא סכימה** — מונע clipping כששני הצדדים מדברים יחד. המחיר: כשצד אחד שותק, הצד המדבר יורד ל-חצי-עוצמה; whisper עמיד לרמות. תואם את דפוס הריפוד-בשקט של `interleave_stereo`.

### 4.4 מסלול ה-stop — ענף מיזוג ב-`stop_batch_recording_to_file` (`lib.rs`)
היום הפונקציה **דוחה** את `Call` ("מצב שיחה נעצר דרך stop_call_recording…", `lib.rs:676-678`). השינוי:
- לדחות **רק** `CallCloud` (עדיין אינליין, ללא שינוי לוגי).
- **לקבל** `CallLocal` דרך ענף מיזוג. שאר הפונקציה **נשארת כמו שהיא**: silence-guard על הבאפר הממוזג (`is_effectively_silent(&samples, 0.005)`, `lib.rs:688`) → `write_wav_16k_mono` → מחזיר path.

`stop_recorder_for_source` (`lib.rs:710`) מקבל ענף חדש `#[cfg(windows)] CallLocal`:
```
CallLocal → drain mic (state.recorder) + drain system (state.system_recorder),
            שניהם מחושבים לפני propagation-של-error (כמו run_stop_call_recording),
            → audio::mix_to_mono(&mic, &system) → Vec<f32> מונו
```
> חישוב **שני** התוצאות לפני `?` — כדי שכשל stop של מיק לא ידלג על ניקוז ה-system recorder (ה-thread של WASAPI חייב תמיד להיסגר, אחרת ידלוף לסשן הבא). מדפוס `run_stop_call_recording` (`lib.rs:772-783`).
> הודעת ה-silence הקיימת מקבלת מקרה ל-`CallLocal` (מיק+מערכת) — לא לשלוח משתמש-פגישה לבדוק רק את המיקרופון.

### 4.5 כפיית מנוע מקומי (`CallLocal` = local כפוי)
- **Backend:** אין נתיב multichannel; `CallLocal` פשוט מחזיר path, והפרונט קורא `transcribe_file` עם `opts.mode="local"`. `pick_batch_route("local") → Local` (whisper). אין צורך לגעת ב-`pick_batch_route`.
- **Frontend:** ל-`calllocal`, ה-`opts.mode` שנשלח ל-`transcribe_file` הוא `"local"` **כפוי** — לא `batchMode` (שממילא מוסתר למצבי פגישה). ל-`mic`/`system` — `batchMode` כרגיל.

### 4.6 Guard מודל-מקומי לפני הקלטה (`batch.rs` + `lib.rs`)
סימטרי ל-`ensure_call_deepgram_available` (`batch.rs:63`). helper טהור חדש:
```rust
pub fn ensure_local_meeting_model_available(has_local_model: bool) -> Result<(), String>
// has_local_model=false → Err("פגישה מקומית דורשת מודל מקומי מורד. הורד אותו בהגדרות.")
```
ב-`start_batch_recording` (`lib.rs:594`), לצד בדיקת ה-Deepgram של `CallCloud`, ענף ל-`CallLocal`:
```
has_local_model = model::is_model_downloaded(&settings.preferred_model)   // model.rs:80, settings.rs:73
ensure_local_meeting_model_available(has_local_model)?                    // לפני swap של guard ההקלטה
```
> מסלול ה-transcribe המקומי כבר בודק קיום-מודל ומחזיר שגיאה (`lib.rs:1096-1102`) — אבל **אחרי** ההקלטה. ה-guard הזה מקדים את הכשל ל**לפני** ההקלטה (סרגל-הבהירות של הנרי: לא לבזבז הקלטה על מצב שיכשל בוודאות).
> ⚠️ **הגבלת ה-guard — לא סימטרי-מלא ל-Deepgram-key.** הבדיקה היא על **קובץ** המודל בדיסק (`is_model_downloaded`), בעוד שהתמלול המקומי דורש שה**מנוע** של whisper יהיה טעון בזיכרון (`lib.rs:468-473`, נטען ברקע בהפעלה רק אם קיים מודל כלשהו, `App.tsx:865`). נשאר חלון-שארית צר: מודל מורד (guard עובר) אבל המנוע לא נטען, או שנטען מודל **אחר** מ-`preferred_model` → `CallLocal` עדיין ייכשל *אחרי* ההקלטה. זהו **בדיוק** הפער הקיים כבר במסלול Mic + "פרטי — מכשיר" (לא רגרסיה). **החלטת v1:** מקבלים את החלון הזה כמצב-קצה מתועד; ה-guard תופס את המקרה השכיח (אין מודל כלל). שיפור אופציונלי (לא חוסם): לבדוק גם `whisperLoaded`/`canRecord` שהפרונט כבר עוקב אחריהם ולהשבית את כרטיס "פרטית במכשיר" עד שהמנוע טעון.

## 5. זרימת נתונים (מצב CallLocal)
```
[mic:    cpal AudioRecorder]  ─┐
                               ├─► mix_to_mono (ממוצע ×0.5, ריפוד-שקט) → Vec<f32> מונו 16kHz
[system: wasapi loopback]     ─┘                │
                                                ▼
                              write_wav_16k_mono → tmp path  (מסלול הקובץ הקיים)
                                                │
             frontend: transcribe_file(path, mode="local" כפוי)  → whisper מקומי
                                                │
                    TranscribeFileResult { text, segments }  → הזרקה/העתקה/TXT/DOCX/SRT
                                                │
                       SRT: style=Diarization (מונו יחיד → בלי תוויות דוברים)
```

## 6. טיפול בשגיאות ומגבלות
- **CallLocal בלי מודל מקומי:** guard לפני הקלטה (§4.6) — שגיאה מנחה, לא קורס, לא מקליט.
- **CallLocal + מערכת שקטה:** silence-guard הקיים רץ על הבאפר ה**ממוזג** (`lib.rs:688`) — פגישה שבה רק צד אחד דיבר **לא** נחסמת (לבאפר יש תוכן). הודעת-silence מודעת-מקור.
- **כשל loopback ל-CallLocal:** ה-rollback הקיים ב-`start_recorders_for_source` עוצר את המיק ומחזיר שגיאה; המיק ממשיך לעבוד רגיל אחר כך.
- **Cancel ל-CallLocal:** עובר דרך `cancel_batch_recording` (`lib.rs:835`) בלבד. ✅ **אין קוד חדש נדרש:** הפונקציה כבר **source-agnostic** — היא מנקזת את ה-system recorder ללא-תנאי (`lib.rs:845-848`, זהו תיקון `af30355`). `CallLocal` = `(true,true)` כמו `CallCloud`, אז אותו נתיב-ביטול בדיוק מכסה אותו. **הסיכון הוא רגרסיה, לא קוד חסר** → הבדיקה הידנית #3 היא הגייט (לוודא ש-start-פגישה-מחדש עובד אחרי Cancel). לא להוסיף cancel מודע-מקור — מיותר.
- **לא-Windows:** שני מצבי הפגישה מוסתרים ב-UI; ה-backend דוחה `uses_system` off-Windows כמו היום.

## 7. אסטרטגיית בדיקות
- **Unit (רץ ללא אודיו):**
  - `mix_to_mono` — אורכים שווים (ממוצע נכון), אורך לא-שווה (ריפוד-שקט לקצר, אורך=max), שני קלטים ריקים (→ ריק), צד אחד ריק (→ חצי-עוצמה של הצד המלא).
  - `recorders_for_source` — הטבלה המעודכנת, כולל `CallCloud`=(t,t) ו-`CallLocal`=(t,t).
  - `RecordingSource` deserialize — `"callcloud"`/`"calllocal"` → הווריאנטים, default עדיין `Mic`.
  - `ensure_local_meeting_model_available` — true→Ok, false→Err מכיל "מודל מקומי".
  - `stop_batch_recording_to_file` דוחה `CallCloud` אבל **לא** `CallLocal` (אם ניתן לבדוק את הענף בבידוד; אחרת ידני).
- **ידני (Windows, אודיו אמיתי — הנרי):**
  1. "פגישה — פרטית במכשיר" עם מודל מורד: לדבר בזמן שמתנגן אודיו-מערכת, לעצור → תמלול מונו מקומי אחד (בלי "אני/הצד השני"), נשאר במכשיר.
  2. בלי מודל מורד → שגיאת ה-guard לפני ההקלטה.
  3. Cancel באמצע "פרטית במכשיר" → ואז start מחדש של פגישה עובד (מוודא ניקוז ה-system recorder).
  4. רגרסיה: Mic/System/"עם זיהוי דוברים" עדיין עובדים; בורר ענן/מקומי מופיע רק ל-Mic/System.

## 8. נגיעות בקוד קיים (blast radius)
- **`audio.rs`** — `mix_to_mono` (הוספה טהורה). `interleave_stereo` ללא שינוי.
- **`batch.rs`** — שינוי-שם `Call`→`CallCloud` + הוספת `CallLocal` ב-`RecordingSource`; `recorders_for_source` (2 ענפים); `ensure_local_meeting_model_available` (חדש); עדכון טסטים קיימים ל-`CallCloud` + טסטים חדשים.
- **`lib.rs`** — ענף `CallLocal` ב-`start_batch_recording` (guard מודל) + ב-`stop_batch_recording_to_file`/`stop_recorder_for_source` (ניקוז-כפול + `mix_to_mono`); שינוי-שם `Call`→`CallCloud` בכל ההתאמות (`matches!`, `unreachable!` וכו'). ⚠️ הזרוע הלא-Windows של `stop_recorder_for_source` (`lib.rs:726-729`, כיום `System | Call => Err`) חייבת להיות `System | CallCloud | CallLocal` — בדיקת-מיצוי של Rust כופה זאת (אפס-סיכון, אך מצוין כאן כדי שלא יופתעו). `cancel_batch_recording` — **ללא שינוי** (כבר source-agnostic, §6). `stop_call_recording` ללא שינוי לוגי מעבר לשם.
- **`model.rs` / `settings.rs`** — **קריאה בלבד** (`is_model_downloaded`, `preferred_model`); ללא שינוי.
- **Frontend `App.tsx`** — טיפוס `RecordingSource` (`mic|system|callcloud|calllocal`); בורר המקורות (2 קבוצות + כותרות עם inline-styles, בלי CSS חדש + 2 כרטיסי פגישה); רינדור מותנה של `batch-mode-cards`; מחיקת פסקת ה-note הנפרדת; `handleStopBatchRecord`: `isCall`→`isCallCloud` (רק `callcloud` במסלול האינליין), `CallLocal` במסלול-הקובץ עם `mode="local"` כפוי; `BatchResult.isCall` נשאר `true` רק ל-`callcloud` (מזין סגנון SRT — `CallLocal` = `Diarization`/מונו).
- **בלי `Cargo.toml`** — אין תלות חדשה (מחזור `wasapi` + whisper הקיימים).

## 9. סיכונים
- **דליפת system recorder ב-Cancel** (§6) — הסיכון הכי חד; הבדיקה הידנית #3 היא הגייט.
- **איכות תמלול מונו-ממוזג** — מיצוע שני צדדים למונו יכול לפגוע ב-whisper כשדוברים חופפים; מקובל ל-v1 (הפרטיות היא הערך), מתועד. אם בעייתי — שיפור עתידי (AGC/normalize לפני מיזוג).
- **בהירות ה-UI** — כל מטרת הסשן. הבדיקה הידנית #4 (הופעה מותנית של הבורר) + סקירת הנרי הן הגייט.
