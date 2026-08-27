# Benchmark — Hebrew Dictation Backend Gate

> **Phase 0** של תכנית v2.0 Freemium. Gate שמחליט איזה backend לתמלול ענן נבחר ל-Cloudflare Worker.

## מה זה עושה
משווה עד 5 מנועי תמלול לעברית על אותן דגימות אודיו:

| Backend | תפקיד | עלות משוערת |
|---|---|---|
| **Groq whisper-large-v3-turbo** | מועמד עיקרי — זול פי ~6 מ-Deepgram | ~$0.04/שעת אודיו |
| **Deepgram Nova-3 (batch)** | baseline (v1.0) | ~$0.26/שעת אודיו |
| **Deepgram Nova-3 (streaming)** | המסלול שהאפליקציה משתמשת בו בפועל כברירת מחדל — פחות הקשר ימני מ-batch, מדד שונה בפועל | כמו batch |
| **Deepgram Nova-3 + keyterm** | עם `--keyterms`, לבדיקת Keyterm Prompting לפני החלטה אם לממש | כמו batch |
| **Local faster-whisper** | אופציונלי; ל-`--local-model ivrit-ai/whisper-large-v3-turbo-ct2` נתונים ייעודיים לעברית | $0 (CPU) |

המדד: **WER** (Word Error Rate). סף החלטה:

- WER **< 15%** → ✅ ממשיכים עם Groq
- WER **15-25%** → בודקים `whisper-large-v3` הרגיל (לא turbo)
- WER **> 25%** → fallback ל-Deepgram, עדכון תמחור

> ⚠️ Streaming ו-keyterm נוספו 2026-08-27 עבור עבודת איכות התמלול
> (`HANDOFF-TRANSCRIPTION-QUALITY.md`) — לא חלק מהחלטת ה-Phase-0 המקורית. לפני בחירת מונחי
> keyterm, לקרוא `docs/research/2026-08-27-vocabulary-mechanisms.md`.

---

## התקנה

```bash
cd benchmark
python -m venv venv
venv\Scripts\activate    # Windows
pip install -r requirements.txt
```

אופציונלי — תמלול לוקאלי (יוריד ~1.6GB מודל בהרצה ראשונה):
```bash
pip install faster-whisper
```
לעברית עדיף `--local-model ivrit-ai/whisper-large-v3-turbo-ct2` (בניית CTranslate2, לא ה-ggml
שהאפליקציה עצמה משתמשת בו) על פני ברירת המחדל הכללית `large-v3-turbo`.

אופציונלי — Deepgram streaming (המסלול שהאפליקציה משתמשת בו בפועל כברירת מחדל):
```bash
pip install websocket-client
```

## מפתחות

```bash
copy .env.example .env
# ערוך .env והכנס:
#   GROQ_API_KEY=gsk_...
#   DEEPGRAM_API_KEY=...
```

## הכנת דגימות

תחת `samples/`, צור תיקייה לכל sample:

```
samples/
  sample_01_quiet/
    audio.wav          # 15-30 שניות, עברית, mono 16kHz מועדף
    reference.txt      # תמלול ייחוס ידני (UTF-8)
  sample_02_tech_terms/
    audio.wav
    reference.txt
```

⚠️ ל-backend ה-streaming: **mono 16kHz 16-bit PCM חובה, לא רק מועדף** — פורמט אחר נכשל עם הודעה
שכוללת פקודת `ffmpeg` להמרה. זה בדיוק הפורמט שהאפליקציה עצמה כותבת עם `debug_save_audio` (ראה
`HANDOFF-TRANSCRIPTION-QUALITY.md`), כך שדגימה אמיתית מהאפליקציה נכנסת ישירות בלי המרה.

**המלצה: 8-10 דגימות מגוונות:**
1. משפט רגיל, קצב נורמלי
2. משפטים עם מונחים טכניים באנגלית בתוך עברית (API, ChatGPT, email)
3. רעש רקע קל (רחוב/קפה)
4. קצב מהיר
5. משפט ארוך עם פסיקים וסוגריים
6. ציטוט/מספרים
7. מבטא / אינטונציה שונה
8. הקלטה ישירות מהמיקרופון של האפליקציה (real-world)

## הרצה

```bash
python run_benchmark.py
```

דילוגים אופציונליים:
```bash
python run_benchmark.py --skip-local                 # ללא faster-whisper
python run_benchmark.py --skip-deepgram               # רק Groq vs Local
python run_benchmark.py --skip-groq                    # רק Deepgram vs Local
python run_benchmark.py --skip-deepgram-streaming      # רק batch, בלי streaming
python run_benchmark.py --keyterms "בינטק,הכתבה בעברית" # מוסיף Deepgram + keyterm
```

## פלט

- **`results.md`** — טבלת סיכום + פרט לכל sample (reference vs hypothesis)
- **stdout** — decision gate אוטומטי בסוף ההרצה

## אחרי ההרצה

1. קרא את `results.md`
2. בדוק את ה-decision gate בסוף הפלט
3. דווח בשיחה — התכנית תתעדכן בהתאם (Phase 1 יתחיל עם ה-backend שנבחר)
