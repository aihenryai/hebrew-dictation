# Benchmark — Hebrew Dictation Backend Gate

> **Phase 0** של תכנית v2.0 Freemium. Gate שמחליט איזה backend לתמלול ענן נבחר ל-Cloudflare Worker.

## מה זה עושה
משווה עד 3 מנועי תמלול לעברית על אותן דגימות אודיו:

| Backend | תפקיד | עלות משוערת |
|---|---|---|
| **Groq whisper-large-v3-turbo** | מועמד עיקרי — זול פי ~6 מ-Deepgram | ~$0.04/שעת אודיו |
| **Deepgram Nova-3** | baseline (v1.0) | ~$0.26/שעת אודיו |
| **Local faster-whisper large-v3-turbo** | אופציונלי, reference לעקביות עם מודל L-2 | $0 (CPU) |

המדד: **WER** (Word Error Rate). סף החלטה:

- WER **< 15%** → ✅ ממשיכים עם Groq
- WER **15-25%** → בודקים `whisper-large-v3` הרגיל (לא turbo)
- WER **> 25%** → fallback ל-Deepgram, עדכון תמחור

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
python run_benchmark.py --skip-local       # ללא faster-whisper
python run_benchmark.py --skip-deepgram    # רק Groq vs Local
python run_benchmark.py --skip-groq        # רק Deepgram vs Local
```

## פלט

- **`results.md`** — טבלת סיכום + פרט לכל sample (reference vs hypothesis)
- **stdout** — decision gate אוטומטי בסוף ההרצה

## אחרי ההרצה

1. קרא את `results.md`
2. בדוק את ה-decision gate בסוף הפלט
3. דווח בשיחה — התכנית תתעדכן בהתאם (Phase 1 יתחיל עם ה-backend שנבחר)
