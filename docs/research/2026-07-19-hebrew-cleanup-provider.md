# Hebrew Transcript Cleanup — On-Device vs Cloud Provider

- **Date:** 2026-07-19
- **Status:** Research complete. No code written. Informs a product/roadmap decision, not yet approved.
- **Trigger:** Henry flagged the open-source macOS app [ghost-pepper](https://github.com/matthartman/ghost-pepper) (100% on-device dictation + on-device LLM cleanup) and asked whether an **on-device** cleanup path is viable for hebrew-dictation's Hebrew "Smart Cleanup" (רישוף חכם).
- **Method:** three parallel research agents (Hebrew model landscape · Windows/Rust runtime + speed · cloud-provider alternative), each required to cite sources and flag where no evidence exists. Synthesis below.

---

## The question

"Smart Cleanup" today sends the raw Hebrew transcript to **cloud Groq `llama-3.3-70b-versatile`** to strip fillers (אהה/אמ/יעני/כאילו), repetitions and false-starts, fix punctuation, and lightly tidy — preserving meaning, tone and the speaker's words. We want to know if it can move **on-device** (a local LLM on the user's Windows machine) to (a) escape Groq's rate limits — the 100k-tokens/day cap just killed the translation feature — (b) drop the API-key + internet requirement, and (c) let privacy-conscious local-only users (the CallLocal crowd) get cleanup without their text leaving the machine.

---

## TL;DR — recommendation

**On-device Hebrew cleanup is technically viable but lands in the worst quadrant**: the only trustworthy Hebrew model at ≤8B is 7B, and 7B on a CPU-only mid-range laptop is *painfully slow* (tens of seconds per cleanup); the fast tier (1–3B) has weak Hebrew. Plus an engineering landmine (whisper.cpp + llama.cpp ggml symbol clash) and a ~5GB RAM cost.

**The pain we actually started from — rate limits — is a Groq-specific artifact, and the low-effort fix is to switch the cloud provider to Gemini.** It removes the cap (likely for free), *measurably improves* Hebrew quality, is a ~3-line change, and **un-shelves the translation feature** (whose only blocker was Groq's daily-token cap).

- **Do now:** move Smart Cleanup's provider Groq → **Gemini 2.5 Flash-Lite** (or Flash). Config change, quality upgrade, revives translation.
- **Later, opt-in only:** if there's real demand for *offline/private* cleanup (text never leaves the machine — the one thing no cloud switch solves), build an on-device tier with **DictaLM-2.0-Instruct (7B)** via a bundled llama.cpp **sidecar**, framed honestly as "private but slower." Not the default.

---

## Part 1 — On-device model landscape (which local LLM cleans Hebrew?)

**Bottom line:** the smallest model to actually trust for unattended Hebrew cleanup is **~7B**, specifically **DictaLM-2.0-Instruct** (Dicta, Apache 2.0). It is the only ≤8B model with a *published, third-party-judged* Hebrew result showing it beats a same-size generalist.

### The evidence problem (read first)
- The public **Hebrew LLM Chat Leaderboard v2** contains only ~32 frontier/large models — **no small ≤8B open generalist** (Gemma-2/3-small, Qwen2.5, Llama-3.1-8B, Aya-23) is benchmarked there.
- The only place small/mid models are measured head-to-head on Hebrew *generation* is **Dicta's own technical reports** (vendor-authored, but using an external GPT-4o/Gemini judge — more credible than self-report).
- **No independent (non-Dicta) benchmark comparing Gemma/Qwen/Llama/Aya on Hebrew generation quality exists.** State this plainly to anyone using this doc.

### DictaLM 2.0 paper — Hebrew summarization, GPT-4-judged (1–10), the most ≤8B-relevant evidence
| Model | Relevance | Coherence | Consistency | Fluency |
|---|---|---|---|---|
| Gemma-7B-it | 6.29 | 5.37 | 7.25 | 7.19 |
| Llama-3-8B-Instruct | 7.7 | 6.96 | **8.53** | 8.14 |
| **DictaLM2.0-Instruct (7B)** | **8.1** | **7.45** | 8.34 | **8.54** |
| GPT-4 / Claude 3 Haiku (ceiling) | 8.6–8.7 | 7.95–7.96 | 9.48–9.67 | 8.69 |

DictaLM2.0-Instruct (7B) **beats Gemma-7B-it across the board and beats Llama-3-8B on 3 of 4** on a Hebrew generation task — a reasonable proxy for "rewrite Hebrew, preserve meaning."

### DictaLM 3.0 report — smaller tiers fail Hebrew generation (0-shot win-rate, higher=better)
| Model | Summ | Trans | Nikud (orthography) |
|---|---|---|---|
| DictaLM-3.0-1.7B-Instruct | 9.72 | 2.16 | 52.76 |
| gemma-3-1b-it | 0.35 | 0.15 | 3.44 |
| Qwen3-1.7B (think) | 0.4 | 0 | 2.93 |
| Llama-3.3-70B-Instruct | 37.83 | 19.31 | **4.05** (catastrophic even at 70B) |

At **≤1.7B every model is weak** on open Hebrew generation; generalist tiny models are ~0. Specialization (DictaLM) buys real orthography (Nikud) competence even small, but not generation quality. **Llama at any size is a Hebrew blind spot** (Nikud 4.05 at 70B; Hebrew not officially supported).

### Per-model verdict
| Model | Size | Hebrew evidence | GGUF | License |
|---|---|---|---|---|
| **DictaLM-2.0-Instruct** | 7B | **Cited**: beats Gemma-7B, ~Llama-3-8B (GPT-4-judged) | ✅ official (Q4_K_M) | **Apache 2.0** ✅ |
| DictaLM-3.0-1.7B-Instruct | 1.7B | Cited: weak generation (~10/100), decent Nikud | ✅ official | Apache 2.0 |
| DictaLM-3.0-Nemotron-12B | 12B | Cited: best Nikud in tier, but Gemma-3-12b edges it on Summ/Trans | ✅ official | Apache 2.0 |
| Gemma 3 (1B/12B/27B) | — | Cited (Dicta's table): 1b near-zero; 12b beats Dicta's 12b on Summ/Trans | ✅ | Gemma custom terms |
| Qwen2.5 / Qwen3 | — | Qwen2.5: **none found**. Qwen3 measured & **not good** for Hebrew | ✅ | Apache 2.0 (most) |
| Aya-23 / Aya Expanse | 8B/32B | Expanse-32B mid-pack; 8B untested | ✅ | ❌ **CC-BY-NC (non-commercial)** — blocks a shipped product |
| Llama 3.x | — | No official Hebrew; Nikud blind spot | ✅ | Llama community license |

**Smallest trustworthy = DictaLM-2.0-Instruct 7B.** A 1.7–3B tier would need a diff-based guard (reject outputs that diverge too far from the input) because small models can silently alter meaning in Hebrew. *Inference flag:* the cleanup task is narrower than summarization (constrained rewriting), so a small model *might* do better on it specifically — but nothing benchmarks that, and the downside (silent meaning drift) worsens as models shrink.

---

## Part 2 — Runtime & speed (Rust/Tauri, Windows, whisper.cpp already linked)

**Bottom line:** llama.cpp is the right engine, but **not as a second FFI crate** — run it as a **sidecar process**. The 1–3B tier is snappy on CPU; the 7–8B tier (which is the quality floor from Part 1) is **painful on CPU-only laptops**.

### ⚠️ The integration landmine (load-bearing)
whisper.cpp and llama.cpp **both statically vendor `ggml`**. Linking both into one binary produces confirmed Windows MSVC `LNK2005` symbol conflicts (`ggml_backend_buft_name already defined`, etc.) — [llama.cpp issue #9267](https://github.com/ggml-org/llama.cpp/issues/9267), still unfixed upstream. Since whisper.cpp is *already* FFI-linked into our process, adding `llama-cpp-2` the naive way hits this wall.

**Workaround:** bundle the official prebuilt `llama-server.exe`/`llama-cli.exe` as a **Tauri sidecar** (separate process, separate address space), talk to it over `127.0.0.1`, and download it + one GGUF on demand — exactly mirroring the existing whisper-model download pattern. Same GGUF format, same MIT license family. *Fallback if the sidecar is too much friction:* **candle** (pure Rust, no ggml clash, no clang/bindgen) — at the cost of slower CPU decode and narrower model support.

### CPU inference speed (measured points; laptop figures are extrapolated)
| Tier | Reported tok/s | Hardware |
|---|---|---|
| 1–3B Q4_K_M | 10–15 | "modern CPU" (desktop-ish) |
| 3.8B (Phi-4 mini) | 12 | i7-12700 desktop |
| 7–8B Q4_K_M | 4–12 | Ryzen 9950X / i9-14900K **desktop, AVX-512** |
| 7–8B Q4_K_M | ~4–5 | "modern CPU" |

No benchmark pinned to a thin *mobile* chip was found — that's the data gap. Extrapolated latency for **one cleanup prompt (50–300 tokens out)** on a mid-range laptop:
- **1–3B:** ~8–20 tok/s → **~3–15s** → acceptable.
- **7–8B:** ~3–8 tok/s → **~13–100s** → mostly "tens of seconds, painful."

### Windows gotchas
- The ggml symbol clash (above) — design around it from day one.
- **AVX2 as the baseline** binary; only opt into AVX-512 after a CPUID check, or older CPUs crash with illegal-instruction. (Upstream ships separate per-ISA binaries.)
- A 7–8B Q4 GGUF is ~4.5–5GB, on top of whisper models (1.5–6GB) → set a 16GB RAM floor if both resident, or unload whisper first (cleanup is sequential after transcription anyway).
- Build machine needs clang/libclang + C++17 (dev-only; end users get a compiled binary).

---

## Part 3 — Cloud alternative (does switching provider fix the pain?)

**Bottom line: yes — cheaply, and it's a quality upgrade.** The daily-cap pain is a **Groq-specific artifact** (100k TPD is unusually low), not a universal cloud limit.

| Provider + model | Free-tier limits (2026) | ~Paid $/mo (50–200 calls/day) | Hebrew: Summ / Nikud |
|---|---|---|---|
| **Groq llama-3.3-70b** (current) | 30 RPM / 12k TPM / **100k TPD** ← the pain | $0.41–1.66 | 37.8 / **4.05** |
| **Gemini 2.5 Flash-Lite** | ~30 RPM / ~1M TPM / ~1,500 RPD, **no daily token cap** | $0.15–0.60 | **43.0** / **70.7** |
| **Gemini 2.5 Flash** | ~15 RPM / ~1M TPM / ~1,500 RPD, no token cap | $0.84–3.36 | **46.9** / **79.5** |
| OpenAI gpt-4o-mini | no ongoing free daily quota | $0.23–0.90 | 23.9 (weakest) / 50.8 |

- **Gemini's free tier has no daily *token* cap** — only ~1,500 requests/day, i.e. 7–30× headroom for this workload. It likely solves the pain **for free**.
- Gemini Flash-Lite/Flash **beat the current Groq model on the Hebrew rewriting proxy AND on Nikud** (Groq's 4.05 is worst-in-leaderboard). Switching is a quality upgrade, not just a limit fix.
- **Migration ≈ config change:** Gemini exposes an OpenAI-wire-compatible endpoint (`.../v1beta/openai/`), and our `enhance_inner` already speaks OpenAI-wire to Groq → swap `base_url` + key + model name. The `finish_reason` truncation fix (2026-07-19) carries over unchanged.
- **Even simpler if diversification isn't a goal:** Groq's own paid Developer tier (10× limits, same code, add a card) removes the cap with a zero-code change. Decide whether moving *off* Groq is itself desired.

### What cloud-switching does NOT solve
Privacy (text still leaves the machine — Gemini/OpenAI/Groq are all "cloud"), offline use, and the API-key requirement. **Only on-device inference addresses those.** If the goal was ever "dictated text never leaves the device," no provider switch touches it.

---

## Synthesis & decision

Map the choice to the *actual goal*:

1. **Goal = "stop hitting the rate limit" (what killed translation, what pains cleanup).**
   → **Switch Smart Cleanup provider Groq → Gemini 2.5 Flash-Lite.** Low effort, free/pennies, quality upgrade, and it **revives the shelved translation feature** — its only blocker was Groq's 100k-TPD cap, and Gemini has none. The provider question left open in the translation spec now has an answer: **Gemini.**

2. **Goal = "dictated text never leaves the device" (offline / privacy — the CallLocal ethos).**
   → On-device is the *only* path, and it means **DictaLM-2.0-Instruct 7B via a llama.cpp sidecar**, downloaded on demand, offered as an **opt-in "private cleanup" tier that is explicitly slower** — the exact analogue of how CallLocal is already an opt-in, slower, on-device privacy mode. Not the default, and only worth building if there's real demand.

The on-device Hebrew-quality/CPU-speed tension means #2 is a real project with a real UX cost, not a drop-in. #1 is a config change that also fixes translation. Recommend #1 now; hold #2 for demand.

---

## Sources

**Hebrew models:** [DictaLM 2.0 paper](https://arxiv.org/abs/2407.07080) · [DictaLM 3.0 report](https://arxiv.org/abs/2602.02104) · [dictalm2.0-instruct-GGUF](https://huggingface.co/dicta-il/dictalm2.0-instruct-GGUF) · [Hebrew LLM Leaderboard v2](https://huggingface.co/spaces/hebrew-llm-leaderboard/chat-leaderboard) · [leaderboard blog](https://huggingface.co/blog/leaderboard-hebrew) · [aya-expanse-8b (license)](https://huggingface.co/CohereLabs/aya-expanse-8b) · [Llama 3.1 model card (langs)](https://github.com/meta-llama/llama-models/blob/main/models/llama3_1/MODEL_CARD.md)

**Runtime/speed:** [llama-cpp-2](https://github.com/utilityai/llama-cpp-rs) · [ggml/whisper symbol conflict #9267](https://github.com/ggml-org/llama.cpp/issues/9267) · [whisper+llama discussion #3544](https://github.com/ggml-org/whisper.cpp/discussions/3544) · [llama.cpp releases (per-ISA)](https://github.com/ggml-org/llama.cpp/releases) · [candle](https://github.com/huggingface/candle) · [onnxruntime-genai](https://github.com/microsoft/onnxruntime-genai) · [CPU LLM bench](https://www.promptquorum.com/local-llms/best-cpu-only-llm) · [llama.cpp benchmarks](https://www.myaihardware.com/llama-cpp-benchmarks)

**Cloud:** [Groq rate limits](https://console.groq.com/docs/rate-limits) · [Gemini rate limits](https://ai.google.dev/gemini-api/docs/rate-limits) · [Gemini pricing](https://ai.google.dev/gemini-api/docs/pricing) · [Gemini via OpenAI library](https://developers.googleblog.com/en/gemini-is-now-accessible-from-the-openai-library/) · [OpenAI rate limits](https://developers.openai.com/api/docs/guides/rate-limits) · [hebrew-llm chat-results dataset](https://huggingface.co/datasets/hebrew-llm-leaderboard/chat-results)
