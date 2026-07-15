# Dictation MCP — Design Spec

- **Date:** 2026-07-15
- **Status:** Draft (pending spec-review loop + user approval)
- **Author:** Claude (brainstormed with Henry)
- **Backlog origin:** HANDOFF "Voicebox comparison" item #2 — an MCP server around the dictation that builds on the shipped Local API (#1). Gives coding agents (Claude Code / Cursor) voice dictation *into the agent session*, and lets automation (video pipeline / cloud-agent) read the latest dictation programmatically.

---

## 1. Problem & goals

The Hebrew-dictation app already ships an **opt-in Local API** (`local_api.rs`): a `tiny_http` server on `127.0.0.1:5757`, `GET /transcript` → `{"text": ...}`, gated behind `local_api_enabled` (off by default). It exposes the last injected transcript but has **no programmatic MCP surface** and **no way to tell a fresh dictation from a stale one** — a consumer that reads `/transcript` may get a transcript from an hour ago and mistake it for what the user just said.

**Goal:** a thin MCP server that wraps the Local API and serves two consumers *equally*:

1. **Coding agents** (Claude Code / Cursor) — dictate a prompt by voice; the agent pulls the fresh transcript.
2. **Automation** (video pipeline / cloud-agent) — read the latest dictation as data.

**Success scenario:** Henry is in Claude Code, the agent calls `wait_for_dictation`, Henry presses Alt+D and speaks a sentence, and that exact sentence comes back to the agent as text — with confidence it is the new utterance, not an old one.

### Non-goals (out of scope for v1, YAGNI)

- Remote-triggered recording (an MCP tool that starts the mic) — deliberately excluded to add **zero new attack surface**. The app's mic is driven only by the user's hotkey/UI.
- Token / auth on the API — read-only endpoints only, which Henry already reasoned are low-risk (HANDOFF: "a read-only 'last transcript' endpoint is low-risk").
- WebSocket / server-push live streaming into a running turn (MCP has no server→model push mid-tool-call).
- History access, transcript editing, MCP resources/prompts.
- Publishing the MCP server as a GitHub repo (v1 is a standalone local folder).

---

## 2. Architecture

Three pieces, two locations:

```
┌─────────────────────────────┐        GET /transcript          ┌──────────────────────────┐
│ hebrew-dictation app (Rust) │  ◄──── 127.0.0.1:5757 ────────  │ hebrew-dictation-mcp     │
│  local_api.rs               │        {text, seq, at}          │ (Node/TS, stdio)         │
│  + monotonic `seq` counter  │                                 │  get_last_transcript     │
└─────────────────────────────┘                                 │  wait_for_dictation      │
                                                                 │  dictation_status        │
                                                                 └────────────┬─────────────┘
                                                                   stdio MCP  │
                                                              ┌───────────────┴───────────────┐
                                                              │ Claude Code / Cursor / scripts │
                                                              └────────────────────────────────┘
```

- **Location A** — the app repo (`AI-Tools/MCP-Dev/hebrew-dictation/`): the Rust `seq` change.
- **Location B** — a new sibling folder (`AI-Tools/MCP-Dev/hebrew-dictation-mcp/`): the Node/TS MCP server. Matches the existing MCP-Dev convention (`pro-image-mcp`, `video-animator-mcp`).

---

## 3. Part A — Rust change (freshness signal)

**Minimal, additive, backward-compatible.** The only new concept is a monotonic sequence number that increments once per successful transcript injection.

### 3.1 State

Today (`lib.rs:59`, `lib.rs:1912`):

```rust
last_transcript: Arc<Mutex<String>>,
```

Becomes a small struct:

```rust
#[derive(Clone, Default)]
pub struct LastTranscript {
    pub text: String,
    pub seq: u64,     // monotonic; increments once per successful injection
    pub at_ms: u64,   // unix epoch millis of the last update (0 if never)
}
// state field:
last_transcript: Arc<Mutex<LastTranscript>>,
```

### 3.2 Update site

Today (`lib.rs:1317`), inside `inject_text_defocused`, after a successful injection:

```rust
if let Ok(mut last) = state.last_transcript.lock() {
    *last = text.to_string();
}
```

Becomes a call to a **pure, unit-testable helper** (the TDD seam):

```rust
/// Record a freshly injected transcript: replace text, bump seq, stamp time.
pub fn bump_transcript(last: &mut LastTranscript, text: &str, now_ms: u64) {
    last.text = text.to_string();
    last.seq = last.seq.wrapping_add(1);
    last.at_ms = now_ms;
}
```

Call site passes `now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)`.

**Seq resets to 0 on app restart** (in-memory counter, intentionally not persisted). This is *safe by design*: `wait_for_dictation` always re-reads the current `seq` as its baseline at call time (§4.3), so a reset never makes a stale transcript look fresh. Do not "fix" this into a persisted counter.

### 3.3 API response

`local_api.rs` `GET /transcript` serializes from the struct via a pure helper:

```rust
pub fn transcript_json(last: &LastTranscript) -> String {
    serde_json::json!({ "text": last.text, "seq": last.seq, "at": last.at_ms }).to_string()
}
```

`local_api::start` takes `Arc<Mutex<LastTranscript>>` instead of `Arc<Mutex<String>>`. The request handler stays panic-free exactly like today: lock the mutex, and on a poisoned lock fall back to a default/empty transcript (mirrors the current `unwrap_or_default()`), then call `transcript_json`.

**Backward compatibility:** the JSON only *adds* `seq` and `at`; the existing `text` key is unchanged, so any current reader of `.text` keeps working. Disabled API = no listener, identical to today.

### 3.4 Rust tests (TDD, red→green)

- `bump_transcript` increments seq `0→1→2` across calls and replaces text / stamps time.
- `transcript_json` output contains `text`, `seq`, `at` with expected values.
- (Behavior when API disabled is unchanged — no new test needed; existing coverage stands.)

---

## 4. Part B — MCP server (`hebrew-dictation-mcp/`)

### 4.1 Stack & layout

- Node + **TypeScript ^5.5**, `@modelcontextprotocol/sdk ^1.x`, `zod ^4` — same dependency versions as `pro-image-mcp`, and the same `src/index.ts` + `test-client.mjs` file layout.
- Transport: **stdio** (the standard for Claude Code / Cursor local servers). NB: the MCP-Dev siblings use *different* transports — `pro-image-mcp` is a Cloudflare Worker (HTTP, `@cloudflare/vitest-pool-workers`) and `video-animator-mcp` is Node/Express — so **do not copy `pro-image-mcp`'s Workers vitest config**; use a plain Node `vitest` (or `node --test`) setup for `test/client.test.ts`.
- Layout:
  - `src/index.ts` — MCP server, registers the 3 tools.
  - `src/client.ts` — `DictationClient`: `fetchTranscript()` + `waitForNext()`. Isolated from the MCP wiring so it is unit-testable against a mock HTTP server.
  - `test/client.test.ts` — unit tests.
  - `test-client.mjs` — a stdio smoke client (like `pro-image-mcp`).
  - `README.md` — registration snippets for Claude Code + Cursor.
- Config via env: `DICTATION_API_PORT` (default `5757`); host fixed to `127.0.0.1`.

### 4.2 `DictationClient` (the logic core)

```
fetchTranscript(): Promise<{ text: string; seq: number; at: number }>
  → GET http://127.0.0.1:{port}/transcript, parse JSON.
  → connection refused / non-200 → throw a typed DictationUnavailableError.

waitForNext(sinceSeq: number, timeoutMs: number, pollMs = 500):
    Promise<{ status: "new"; text; seq; at } | { status: "timeout" }>
  → loop: fetchTranscript(); if seq > sinceSeq → return {status:"new", ...}.
  → else sleep(pollMs) and repeat until elapsed >= timeoutMs → return {status:"timeout"}.
```

Polling (not server long-poll) keeps the Rust side single-threaded and unchanged — the wait lives entirely in Node. This matches the HANDOFF's "pull-based" note.

**Mid-wait failure:** if the API becomes unreachable *during* a wait (e.g., the app restarts inside the timeout window), the next `fetchTranscript()` throws `DictationUnavailableError`, which propagates out of `waitForNext` and aborts the wait — no silent retry loop. The tool surfaces the same actionable "API not reachable" error (§4.4).

### 4.3 Tools

| Tool | Input (zod) | Behavior | Result |
|---|---|---|---|
| `get_last_transcript` | none | `fetchTranscript()` | `{ text, seq, at }` — the latest, whatever it is. Non-blocking. For the automation consumer. |
| `wait_for_dictation` | `{ timeout_seconds?: number = 120 }` (clamp 1..600) | read current seq via `fetchTranscript()`, then `waitForNext(seq, timeout)` | `status:"new"` → the fresh text; `status:"timeout"` → a plain "no new dictation within Ns" message (not an error). For the coding-agent consumer. |
| `dictation_status` | none | `fetchTranscript()` once, report reachability | reachable → "API up, last seq N"; unreachable → actionable guidance (enable `local_api_enabled`, launch the app). |

### 4.4 Error handling

- **App down / API disabled:** every tool catches `DictationUnavailableError` and returns a clear tool error: *"Hebrew-dictation Local API not reachable on 127.0.0.1:{port}. Enable `local_api_enabled` in the app settings and make sure the app is running."* Never an unhandled crash.
- **`wait_for_dictation` timeout:** returns a structured non-error result so the agent can decide (ask again / give up), rather than throwing.

### 4.5 Node tests (TDD)

- `waitForNext` returns `status:"new"` with the new text once the mock's seq increments.
- `waitForNext` returns `status:"timeout"` when seq never changes within the window.
- `fetchTranscript` throws `DictationUnavailableError` on connection refused.
- `get_last_transcript` passes the parsed payload straight through.

---

## 5. Data flow — the "dictate into the agent" path

1. Agent (Claude Code) calls `wait_for_dictation({ timeout_seconds: 120 })` and, in its own text, tells Henry "go ahead and dictate."
2. Tool reads current `seq` = N.
3. Henry presses **Alt+D** and speaks; the app transcribes and injects → `bump_transcript` sets `seq = N+1`, new text.
4. Tool's `waitForNext` sees `seq > N` on its next poll → returns the new text.
5. Agent receives the sentence as the user's input.

No remote trigger: the recording is always started by Henry's own hotkey/UI. The MCP only *observes* the freshness counter.

---

## 6. Acceptance criteria

- **Rust:** `GET /transcript` returns `{text, seq, at}`; `seq` increments exactly once per successful injection (dictation and streaming paths both go through `inject_text_defocused`); `cargo test` covers `bump_transcript` + `transcript_json`; `cargo build` clean; disabled-API behavior unchanged.
- **MCP:** the 3 tools work over stdio; `wait_for_dictation` resolves on a new dictation and times out gracefully; unreachable-API errors are actionable; `npm test` green; `test-client.mjs` smoke run lists the tools and calls `dictation_status`.
- **Docs:** `README.md` shows the exact registration snippet for Claude Code (`.mcp.json`) and Cursor, **and states plainly that `wait_for_dictation` does not start the mic** — it waits for the user to dictate via the app's own hotkey/UI (the name could otherwise imply a remote trigger).

---

## 7. Rollout / verification notes

- The Rust change ships in the app's next build; until then, the MCP server can be tested against a hand-run dev build (`npm run tauri dev`) with `local_api_enabled: true` in settings.json.
- Registering the MCP in *this* Claude Code session is possible but not required for the build — unit tests + `test-client.mjs` prove it without a live app.
- No GitHub Release is implied by this work; the Rust change is committed to `main` like other app changes, the MCP folder is a standalone local project in `MCP-Dev/`.
