# Spec — "רישוף חכם" (Smart Cleanup) ל-Hebrew Dictation

- **תאריך:** 2026-06-15
- **גרסת יעד:** v2.9.0
- **סטטוס:** Design approved (ממתין ל-spec review + תוכנית מימוש)
- **פרויקט:** `AI-Tools/MCP-Dev/hebrew-dictation`

---

## 1. הבעיה והמטרה

כיום התוסף מחזיר **תמלול גולמי** — בדיוק מה שנאמר, כולל מילות מילוי ("אהה", "אֶמ", "יעני", "כאילו"), גמגומים, חזרות ו-false-starts. כל המתחרים המובילים (Wispr Flow, superwhisper, Aqua, Willow, VoiceInk) כבר עברו משכבת תמלול לשכבת **post-processing מבוסס-LLM** שמנקה את הטקסט. זה הקו שהתוסף נמצא מתחתיו.

**המטרה:** להוסיף שכבת **רישוף** opt-in שהופכת "תמלול" ל"הכתבה" — טקסט עברי נקי, מפוסק ומוכן-לשליחה — תוך שמירה על שני היתרונות הייחודיים של הכלי: **חינמי** (אפס כרטיס אשראי) ו**פרטיות** (offline נשאר אופציה).

**הבידול:** "הכלי החינמי היחיד עם רישוף עברי איכותי ופרטיות מלאה". אף מתחרה גלובלי לא מתחייב לעברית, וכולם בתשלום $8–15/חודש.

---

## 2. Scope — מה נכלל ב-v1

מצב **opt-in** (כבוי כברירת-מחדל) שלוקח את הטקסט הגולמי מ-`transcribe`, מעביר אותו ב-LLM שמנקה מילות מילוי / חזרות / false-starts ומסדר פיסוק וניסוח בעברית, ואז מזריק את התוצאה הנקייה לשדה הפעיל.

### גבולות (YAGNI — מה *לא* ב-v1)
- ❌ **Streaming** — שם הטקסט זורם תוך כדי דיבור; אין נקודת "סוף" לרישוף. רישוף שייך ל-batch mode בלבד. במצב streaming המתג לא רלוונטי.
- ❌ **Snippets / Smart Replace** — ניצחון מהיר נפרד, גרסה הבאה.
- ❌ **Modes מרובים אוטומטיים** — תשתית כן (סעיף 4.4), מימוש לא. v1 = מצב יחיד.
- ❌ **OpenAI כספק רישוף** — סותר את פילוסופיית אפס-כרטיס-אשראי (ר' החלטה D2).
- ❌ **רישוף מקומי (SLM offline)** — דורש הורדת מודל נוסף + research. vNext.

---

## 3. החלטות עיצוב מרכזיות

### D1 — מנוע הרישוף: Groq (Llama 3.3 70B)
- חינמי, מהיר (~שנייה לפסקה ב-Groq), כבר נתמך כספק ב-`api_transcribe.rs`.
- מי שמתמלל ב-Groq → רישוף מיידי באותו מפתח, אפס onboarding נוסף.
- מי שב-Deepgram/מקומי → מוסיף מפתח Groq חינמי (בלי כרטיס אשראי).
- מודל: `llama-3.3-70b-versatile` (או דגם Llama עדכני זמין ב-Groq; נקבע סופית בזמן מימוש מול ה-API).

### D2 — לא OpenAI
OpenAI הוסר במכוון ב-v2.4.0 כי דרש כרטיס אשראי, בניגוד ל"אפס סיכון כספי". להחזירו לרישוף יסתור את עמוד התווך של המוצר. נדחה ל-vNext גמיש בלבד אם תהיה דרישה.

### D3 — fail-safe מוחלט (קריטי)
רישוף הוא **שיפור, לא נקודת כשל**. כל כשל (אין מפתח / רשת / timeout / תשובה ריקה / תשובה חשודה) → מזריקים את **הטקסט הגולמי** המקורי. המשתמש לעולם לא מאבד את מה שאמר. ה-fail-safe ממומש ב-frontend: ה-frontend מחזיק את `raw_text`, קורא ל-`enhance_text`, ובכשל מזריק את `raw_text`.

### D4 — תשתית Modes מההתחלה, מצב יחיד ב-v1
שדה `enhance_mode` נשמר ב-settings; v1 שולח תמיד `"he_general"`. ה-prompts נשמרים כטבלת פרופילים ב-`enhance.rs` שקל להרחיב (מייל / וואטסאפ / מסמך) — מונע refactor כשנוסיף Modes בעתיד. זה ה-meta-feature של superwhisper/VoiceInk.

### D5 — opt-in, default OFF + שקיפות פרטיות
- מתג "✨ רישוף חכם" בהגדרות, כבוי כברירת-מחדל → שומר התנהגות + פרטיות קיימות.
- הדלקה ללא מפתח Groq → הנחיה להוסיף מפתח.
- **אזהרת פרטיות במצב מקומי:** אם המשתמש ב-`Local` ומדליק רישוף — הבהרה חד-פעמית שהטקסט (ולא האודיו) יישלח ל-Groq. מודל VoiceInk: *הקול נשאר מקומי, רק הטקסט עף לענן, opt-in*.

---

## 4. ארכיטקטורה

### 4.1 Data flow
```
stop_recording → Vec<f32>
   → transcribe(samples, lang)            // קיים, lib.rs:236 — מחזיר raw_text
   → [חדש] אם enhance_enabled && !streaming && has_groq_key:
         enhance_text(raw_text, mode)      // מחזיר Ok(enhanced) או Err
            ├─ Ok(enhanced) → inject(enhanced)
            └─ Err(_)       → inject(raw_text)   // fail-safe (D3)
      אחרת:
         inject(raw_text)                  // התנהגות קיימת, ללא שינוי
```
ה-orchestration ב-frontend (`App.tsx`, ב-`stopAndTranscribe`) — בדיוק כפי שהוא כבר עושה `transcribe` ואז `inject_text` היום. מוסיפים שלב אחד באמצע.

### 4.2 מודול חדש: `src-tauri/src/enhance.rs`
- `enum EnhanceMode { HeGeneral }` — עם `from_str` / default, מוכן להרחבה.
- `fn build_messages(mode, text) -> Vec<ChatMessage>` — בונה system+user prompt. **טהור וניתן ל-unit-test בלי רשת.**
- `async fn enhance_inner(text, mode, api_key) -> Result<String, EnhanceError>` — קריאת Groq chat completions (אותו pattern של `reqwest` + `classify_status` כמו `api_transcribe.rs`), `temperature: 0.2`, timeout 10s.
- `enum EnhanceError` — `Unauthorized | RateLimited | Network | Timeout | Empty | Suspicious | Other` עם הודעות עברית (תואם ל-`ApiError` הקיים).
- **הגנת הוזיה:** אם הפלט ריק, או ארוך מפי-2 מהקלט (`output.chars().count() > raw.chars().count() * 2`) → `Suspicious` → fallback ל-raw. הסף קבוע (לא "נקבע במימוש") כדי שה-unit test יהיה דטרמיניסטי.

### 4.3 ה-prompt (פרופיל `he_general`)
**System:**
> אתה עורך לשוני לעברית. קלט: תמלול דיבור גולמי. פלט: אותו טקסט כטקסט כתוב נקי. הסר מילות מילוי (אהה, אמ, יעני, כאילו), חזרות וגמגומים. תקן פיסוק ורווחים. שמור בדיוק על המשמעות, הטון והשפה של הדובר. אל תוסיף מידע, אל תקצר משמעותית, אל תתרגם, אל תענה לתוכן — ערוך בלבד. החזר אך ורק את הטקסט הערוך, בלי הקדמות, הסברים או מירכאות.

**User:** הטקסט הגולמי.

> **שפה (v1 ממוקד עברית):** ל-`enhance_text` אין פרמטר שפה — הוא מקבל טקסט בלבד. ה-prompt מורה "שמור על שפת הדובר", כך שקלט באנגלית / `multi` (code-switching) נשמר בשפתו (best-effort). רישוף איכותי ללא-עברית הוא מחוץ ל-scope v1.

### 4.4 שינויי `settings.rs`
שני שדות חדשים, שניהם `#[serde(default)]` ל-back-compat (אותו pattern של כל השדות הקיימים):
- `enhance_enabled: bool` (default `false`).
- `enhance_mode: String` (default `"he_general"`). **ערך לא-מוכר שנטען מ-JSON ישן → נופל ל-default** דרך `EnhanceMode::from_str` (מראה את ה-pattern של ה-`ApiProvider` deserializer ב-`settings.rs:46`, ששומר back-compat).
מתווספים ל-`AppSettings`, ל-`RedactedSettings`, ל-`redacted()`, ול-`Default`.

### 4.5 שינויי `lib.rs`
- command חדש `enhance_text(state, text, mode: Option<String>) -> Result<String, String>` — קורא את **`s.groq_api_key` ישירות** ומפעיל `enhance::enhance_inner`. אם אין מפתח → `Err` (וה-frontend נופל ל-raw).
- ⚠️ **לא** להעתיק את ה-pattern של `transcribe` שמשתמש ב-`active_api_key()` (תלוי-ספק). הרישוף תמיד דרך Groq, ללא תלות בספק התמלול — משתמש ב-Deepgram/מקומי חייב להעביר את מפתח ה-Groq שלו ל-Groq, לא מפתח Deepgram.
- רישום ב-`invoke_handler` (ליד `transcribe` / `inject_text`).
- אין שינוי ל-flow הקיים — רק תוספת.

### 4.6 שינויי `App.tsx`
- `stopAndTranscribe`: אחרי `transcribe`, אם `enhance_enabled && !streaming && has_groq_key` → `try { enhanced = await enhanceText(raw, mode) } catch { enhanced = raw }` → `inject(enhanced)`.
- הגדרות: מתג "✨ רישוף חכם" (gated על `has_groq_key`, עם הנחיה אם חסר) + אזהרת פרטיות במצב Local.
- Toolbar: אינדיקטור "✨ משכתב…" בזמן הקריאה ל-enhance.

---

## 5. אסטרטגיית בדיקות
- **Unit (Rust):** `build_messages` מייצר את ה-system+user הנכון לכל mode.
- **Unit (Rust):** הגנת הוזיה — פלט ריק / ארוך-מדי → `Suspicious`.
- **Unit (Rust):** מיפוי status→`EnhanceError` (401→Unauthorized וכו').
- **ידני (smoke):** משפט עברי עם "אהה/כאילו" + חזרה → פלט נקי; ניתוק רשת באמצע → הטקסט הגולמי מוזרק (fail-safe).

---

## 6. סיכונים ידועים
- **Latency:** הרישוף מוסיף ~שנייה אחרי ה-stop. מקובל ל-batch; Groq מהיר. אם יחרוג — timeout 10s → fallback raw.
- **הוזיות LLM:** ממותן ע"י temperature נמוך, prompt מגביל, והגנת אורך-פלט.
- **בלבול פרטיות:** ממותן ע"י default OFF + אזהרה מפורשת במצב Local.

---

## 7. עבודה עתידית (vNext)
Snippets/Smart Replace · Modes אוטומטיים לפי אפליקציה · רישוף ב-streaming · רישוף מקומי (SLM) · בורר ספק LLM גמיש (OpenAI).
