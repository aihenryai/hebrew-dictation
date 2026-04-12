# Hebrew Dictation API — Cloudflare Worker

> Phase 1 MVP של תכנית v2.0 Freemium. Worker שמפרוקסי בקשות תמלול מ-Tauri client אל Groq/Deepgram עם quota tracking לפי device hash.

## ארכיטקטורה

```
[Tauri App] ──POST /transcribe──▶ [Worker] ──▶ [Groq / Deepgram]
   X-Device-Hash                     │
   X-Audio-Duration-Seconds          ├─▶ KV: DEVICES (per-device monthly counter)
   raw audio bytes                   └─▶ Auto monthly reset
```

**Backend switch:** `wrangler.toml` → `[vars] BACKEND = "groq" | "deepgram"`. אחרי benchmark Phase 0 — להעביר אם Groq נכשל.

## דרישות מקדימות

- Cloudflare account (חינם — אין צורך בכרטיס אשראי לפיתוח)
- Node.js 20+
- Wrangler CLI (יותקן עם `npm install`)

## התקנה

```bash
cd cloudflare-worker
npm install
```

## הגדרת KV namespaces (חד פעמי)

```bash
npx wrangler kv namespace create DEVICES
npx wrangler kv namespace create EMAILS
npx wrangler kv namespace create LICENSES
```

כל פקודה מחזירה `id = "..."`. העתק ל-`wrangler.toml` במקום ה-`PLACEHOLDER_*` המתאים.

## הגדרת secrets

```bash
npx wrangler secret put GROQ_API_KEY
# הדבק את gsk_...

npx wrangler secret put DEEPGRAM_API_KEY
# הדבק את ה-Deepgram key
```

## פיתוח מקומי

```bash
npm run dev
```

זה מריץ Worker מקומית על `http://127.0.0.1:8787`. KV namespaces משתמשים ב-emulator מקומי.

## דפלוי

```bash
npm run deploy
```

ידחוף ל-`hebrew-dictation-api.workers.dev` (או הדומיין שתגדיר).

## Smoke test

```bash
# Health check
curl https://hebrew-dictation-api.workers.dev/health

# Quota (יוצר device record בפעם הראשונה)
curl https://hebrew-dictation-api.workers.dev/quota \
  -H "X-Device-Hash: test-device-12345678901234567890"

# Transcribe (דורש קובץ אודיו)
curl -X POST https://hebrew-dictation-api.workers.dev/transcribe \
  -H "X-Device-Hash: test-device-12345678901234567890" \
  -H "X-Audio-Duration-Seconds: 5.2" \
  -H "Content-Type: audio/wav" \
  --data-binary @sample.wav
```

## Endpoints

| Method | Path | תפקיד |
|---|---|---|
| `POST` | `/transcribe` | מקבל אודיו, מחזיר טקסט + quota |
| `GET` | `/quota` | מצב quota נוכחי לdevice |
| `GET` | `/health` | smoke test (`200 ok`) |

ב-Phase 3 מתווספים: `POST /claim-email`, `POST /verify-email`.
ב-Phase 4 מתווספים: `POST /webhooks/lemonsqueezy`, `POST /license/validate`.

## עלויות

- **Workers free tier:** 100K requests/day. ב-30 דק׳/חודש למשתמש חינמי, זה ~3,300 משתמשים פעילים יומית — המון.
- **KV free tier:** 100K reads/day, 1K writes/day. בכל transcription יש 1 read + 1 write על DEVICES → תומך ב-~1K transcriptions/day.
- אם נחרוג: Workers Paid plan = $5/חודש, ללא הגבלות מעשיות.
