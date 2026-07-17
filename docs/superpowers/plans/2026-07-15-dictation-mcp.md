# Dictation MCP Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose the hebrew-dictation app's last transcript to MCP clients (Claude Code / Cursor / automation), with a reliable "is this dictation fresh?" signal.

**Architecture:** Two halves. (A) An additive change to the app's Rust Local API: a monotonic `seq` counter bumped **once per completed utterance** (not per streaming segment — see spec §3.2, this also fixes a latent bug in the shipped endpoint), surfaced as `GET /transcript` → `{text, seq, at}`. (B) A new standalone Node/TypeScript MCP server (`hebrew-dictation-mcp/`) that polls that endpoint over stdio-MCP and exposes three tools. All wait/poll logic lives in Node — the Rust side stays single-threaded and otherwise untouched. No remote-record trigger, no auth token (read-only endpoint).

**Tech Stack:** Rust (Tauri app, `tiny_http`, `serde_json`) · Node 20+ / TypeScript ^5.5 · `@modelcontextprotocol/sdk` ^1.29 · `zod` ^4 · `vitest` (plain Node pool)

**Spec:** `docs/superpowers/specs/2026-07-15-dictation-mcp-design.md`

---

## File Structure

**Half A — app repo (`AI-Tools/MCP-Dev/hebrew-dictation/`):**

| File | Responsibility |
|---|---|
| `src-tauri/src/local_api.rs` | **Owns the whole "last transcript" concept**: the `LastTranscript` struct, `bump_transcript`, `now_unix_ms`, `transcript_json`, plus the HTTP server. All new pure helpers + their unit tests live here — one file, one responsibility. |
| `src-tauri/src/lib.rs` | 5 edits, no new logic of its own — it only calls `local_api`'s helpers: state field type (~:59), state init (~:1912), **remove** the old stamp from `inject_text_defocused` (~:1317), **add** the stamp at the two utterance boundaries — the `inject_text` command (~:1338) and `stop_streaming_transcription` (~:1106). |

**Half B — new sibling project (`AI-Tools/MCP-Dev/hebrew-dictation-mcp/`):**

| File | Responsibility |
|---|---|
| `package.json`, `tsconfig.json`, `vitest.config.ts` | Scaffold. Plain Node vitest — **not** `pro-image-mcp`'s Workers pool config. |
| `src/client.ts` | `DictationClient` + `DictationUnavailableError`. All HTTP + poll logic. Zero MCP imports → unit-testable against a mock HTTP server. |
| `src/index.ts` | MCP wiring only: construct the client, register 3 tools, connect stdio. No business logic. |
| `test/client.test.ts` | Unit tests for `client.ts` against a `node:http` mock. |
| `test-client.mjs` | stdio smoke client (mirrors `pro-image-mcp`'s file of the same name). |
| `README.md` | Registration snippets + the "does not start the mic" note. |

---

## Chunk 1: Rust freshness signal

### Task 1: `LastTranscript` + `bump_transcript`

**Files:**
- Modify: `src-tauri/src/local_api.rs`
- Test: `src-tauri/src/local_api.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Append to `src-tauri/src/local_api.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_transcript_increments_seq_and_replaces_text() {
        let mut last = LastTranscript::default();
        assert_eq!(last.seq, 0, "fresh state starts at seq 0");
        assert_eq!(last.text, "");
        assert_eq!(last.at_ms, 0);

        bump_transcript(&mut last, "שלום", 1_000);
        assert_eq!(last.seq, 1);
        assert_eq!(last.text, "שלום");
        assert_eq!(last.at_ms, 1_000);

        bump_transcript(&mut last, "עולם", 2_000);
        assert_eq!(last.seq, 2, "each injection bumps seq exactly once");
        assert_eq!(last.text, "עולם");
        assert_eq!(last.at_ms, 2_000);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

> All commands in Chunk 1 run from the **repo root** (`hebrew-dictation/`) and use
> `--manifest-path`, so the shell's cwd never drifts and the `git add` paths in each
> commit step stay valid. Do not `cd src-tauri`.

Run: `cargo test --manifest-path src-tauri/Cargo.toml local_api::tests::bump_transcript_increments_seq_and_replaces_text`
Expected: FAIL — compile error, `cannot find type LastTranscript` / `cannot find function bump_transcript`.

- [ ] **Step 3: Write minimal implementation**

Add near the top of `src-tauri/src/local_api.rs`, after the existing `use` lines:

```rust
/// The last completed transcript, plus a monotonic freshness signal. `seq`
/// increments once per completed utterance so a consumer can tell a NEW
/// dictation from the one it already saw. In-memory only: `seq` resets to 0 on
/// app restart, which is safe because every waiter re-reads its baseline `seq`
/// at call time (see the MCP server's `wait_for_dictation`).
#[derive(Clone, Default)]
pub struct LastTranscript {
    pub text: String,
    pub seq: u64,
    pub at_ms: u64,
}

/// Record a freshly injected transcript: replace text, bump seq, stamp time.
pub fn bump_transcript(last: &mut LastTranscript, text: &str, now_ms: u64) {
    last.text = text.to_string();
    last.seq = last.seq.wrapping_add(1);
    last.at_ms = now_ms;
}

/// Unix epoch millis, saturating to 0 if the clock is before the epoch.
pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
```

> **Expect a transient `dead_code` warning** for `now_unix_ms` until Task 3 wires it up
> (it's `pub` inside a *private* module and has no caller yet). This is expected — do
> **not** "fix" it with `#[allow(dead_code)]`, which would then linger forever. It
> disappears on its own at Task 3. `bump_transcript`/`transcript_json` are exempt
> because their tests count as usage.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml local_api::tests::bump_transcript_increments_seq_and_replaces_text`
Expected: PASS (1 passed).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/local_api.rs
git commit -m "feat(local-api): LastTranscript with a monotonic seq counter"
```

---

### Task 2: `transcript_json`

**Files:**
- Modify: `src-tauri/src/local_api.rs`
- Test: `src-tauri/src/local_api.rs` (same inline test module)

- [ ] **Step 1: Write the failing test**

Add inside the existing `mod tests`:

```rust
    #[test]
    fn transcript_json_includes_text_seq_and_at() {
        let mut last = LastTranscript::default();
        bump_transcript(&mut last, "בדיקה", 1_700_000_000_000);

        let v: serde_json::Value = serde_json::from_str(&transcript_json(&last)).unwrap();
        assert_eq!(v["text"], "בדיקה");
        assert_eq!(v["seq"], 1);
        assert_eq!(v["at"], 1_700_000_000_000u64);
    }

    #[test]
    fn transcript_json_on_empty_state_is_valid_json() {
        let last = LastTranscript::default();
        let v: serde_json::Value = serde_json::from_str(&transcript_json(&last)).unwrap();
        assert_eq!(v["text"], "");
        assert_eq!(v["seq"], 0);
        assert_eq!(v["at"], 0);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml local_api::tests::transcript_json`
Expected: FAIL — `cannot find function transcript_json`.

- [ ] **Step 3: Write minimal implementation**

Add to `src-tauri/src/local_api.rs` under `bump_transcript`:

```rust
/// Serialize the transcript state for `GET /transcript`.
/// Additive: `text` keeps its existing meaning, `seq`/`at` are new keys, so
/// any existing consumer reading `.text` keeps working unchanged.
pub fn transcript_json(last: &LastTranscript) -> String {
    serde_json::json!({
        "text": last.text,
        "seq": last.seq,
        "at": last.at_ms,
    })
    .to_string()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path src-tauri/Cargo.toml local_api::tests::transcript_json`
Expected: PASS (2 passed).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/local_api.rs
git commit -m "feat(local-api): serialize transcript as {text, seq, at}"
```

---

### Task 3: Wire the state through the app — stamping on *complete utterances*

Rewiring, no new unit tests (the pure helpers are already covered; these sites are
integration glue). The existing suite is the regression gate, and **Step 5's manual
check is the real behavioral gate** — it is what proves the fragment bug is dead.

> **Read this before touching `lib.rs`.** The naive move — bumping `seq` where the old
> code stamped, inside `inject_text_defocused` — is **wrong**. `streaming_enabled`
> defaults to `true` (`settings.rs:264`), and `streaming.rs:163` calls
> `inject_text_defocused` **once per final segment**. Stamping there makes `seq` tick
> per *fragment* and leaves `text` holding only the last segment. See spec §3.2.
> Instead: strip the stamp out of `inject_text_defocused` and stamp the **complete**
> text at each of its two callers' utterance boundaries.

**Files:**
- Modify: `src-tauri/src/local_api.rs` (the `start` signature + handler)
- Modify: `src-tauri/src/lib.rs` (5 sites: ~:59, ~:1106, ~:1317, ~:1338, ~:1912)

- [ ] **Step 1: Change the server signature + handler**

In `src-tauri/src/local_api.rs`, change `start`'s parameter type and the body-building block:

```rust
pub fn start(port: u16, last_transcript: Arc<Mutex<LastTranscript>>) {
```

and replace the `/transcript` body construction (currently the `let text = last_transcript.lock()...` + `let body = serde_json::json!({ "text": text })` lines) with:

```rust
                let body = match last_transcript.lock() {
                    Ok(last) => transcript_json(&last),
                    // Stay panic-free on a poisoned lock, exactly like the
                    // previous `unwrap_or_default()` behavior.
                    Err(_) => transcript_json(&LastTranscript::default()),
                };
```

- [ ] **Step 2: Change the state type (2 mechanical sites in `lib.rs`)**

1. State field (~line 59):
```rust
    last_transcript: Arc<Mutex<local_api::LastTranscript>>,
```

2. State init (~line 1912):
```rust
            last_transcript: Arc::new(Mutex::new(local_api::LastTranscript::default())),
```

The spawn (~line 1963) needs no textual change — `state.last_transcript.clone()` clones the Arc, whose inner type just changed. Verify it still compiles.

- [ ] **Step 3: REMOVE the stamp from `inject_text_defocused` (~line 1315)**

Delete this whole block — this function goes back to being purely about injection:

```rust
    if result.is_ok() {
        if let Some(state) = app.try_state::<AppState>() {
            if let Ok(mut last) = state.last_transcript.lock() {
                *last = text.to_string();
            }
        }
    }
```

Leave the surrounding `let result = injector::inject_text(...)` and the window-restore code below it untouched; `result` is still returned at the end of the function.

- [ ] **Step 4: ADD the stamp at the two utterance boundaries**

**(a) Non-streaming — the `inject_text` command (~line 1337).** It is currently a
one-line wrapper; it becomes the stamp site for the complete transcript:

```rust
#[tauri::command]
fn inject_text(app: AppHandle, text: String) -> Result<(), String> {
    let result = inject_text_defocused(&app, &text);
    if result.is_ok() {
        if let Some(state) = app.try_state::<AppState>() {
            if let Ok(mut last) = state.last_transcript.lock() {
                local_api::bump_transcript(&mut last, &text, local_api::now_unix_ms());
            }
        }
    }
    result
}
```

**(b) Streaming — `stop_streaming_transcription` (~line 1106).** `session.stop()`
already returns the accumulated full text, and `state: State<'_, AppState>` is in
scope. Replace `let text = active.session.stop().await?; Ok(text)` with:

```rust
    let text = active.session.stop().await?;

    // One seq bump per streaming session = the whole utterance, not per segment.
    // Skip empty sessions so a no-op recording never looks like a new dictation.
    if !text.is_empty() {
        if let Ok(mut last) = state.last_transcript.lock() {
            local_api::bump_transcript(&mut last, &text, local_api::now_unix_ms());
        }
    }

    Ok(text)
```

`state.last_transcript` is a `std::sync::Mutex`, so this `lock()` is sync — fine here because the guard is dropped before the next `await`.

- [ ] **Step 5: Verify the whole suite + build**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: PASS — the previously-green **55 passed / 1 ignored** plus the 3 new tests = **58 passed / 1 ignored**. No pre-existing test may regress.

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: 0 warnings in `local_api.rs` / `lib.rs` (the `now_unix_ms` dead_code warning from Task 1 is now gone).

Run: `cargo clippy --manifest-path src-tauri/Cargo.toml`
Expected: no *new* warnings in the two touched files. 6 pre-existing warnings live in untouched files — leave them alone.

- [ ] **Step 6: BEHAVIORAL GATE — verify the fragment bug is actually dead**

This is the step that proves the whole design. Do not skip it.

Set `"local_api_enabled": true` in `%APPDATA%/hebrew-dictation/settings.json`, confirm
`"streaming_enabled": true` (the default — we are deliberately testing the hard path),
then run `npm run tauri dev` from the repo root.

1. Press Alt+D and dictate **one long multi-segment sentence** (talk for ~10s without stopping, so Deepgram emits several `is_final` segments), then stop.
2. Run: `curl http://127.0.0.1:5757/transcript`

Expected: `{"text":"<THE ENTIRE SENTENCE>","seq":1,"at":<13-digit ms>}`
- ✅ `text` is the **whole** sentence — not the last fragment.
- ✅ `seq` is **1**, not one-per-segment.
3. Dictate a second sentence → `seq` becomes exactly **2**.

If `seq` jumped by more than 1 per sentence, or `text` is a fragment, the stamp is still
firing per-segment — re-check Step 3 actually removed the old block.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/local_api.rs src-tauri/src/lib.rs
git commit -m "feat(local-api): expose seq/at, stamp once per complete utterance

Also fixes a latent bug in the shipped endpoint: with streaming on (the
default), /transcript returned only the last segment of a dictation."
```

---

## Chunk 2: MCP server

### Task 4: Scaffold `hebrew-dictation-mcp/`

**Files:**
- Create: `../hebrew-dictation-mcp/` + `package.json`, `tsconfig.json`, `vitest.config.ts`, `.gitignore`

> All paths in Chunk 2 are relative to `AI-Tools/MCP-Dev/hebrew-dictation-mcp/` (a **sibling** of the app repo, not inside it). Chunk 1 left you in the `hebrew-dictation` repo — Step 1 moves you out of it.

- [ ] **Step 1: Create the directory and make it its OWN git repo**

⚠️ **`git init` is mandatory, not optional.** `MCP-Dev/` itself belongs to the shared
`claude-dev` root repo, whose commits are owned by the nightly backup job — the root
`CLAUDE.md` forbids hand-committing there, and that rule explicitly covers subfolders
that aren't their own repo. Without `git init`, Chunk 2's five commits would land in
that shared repo alongside parallel sessions' work. The app next door
(`hebrew-dictation/`) is itself a nested repo — this follows the same established
pattern.

```bash
cd ..
mkdir -p hebrew-dictation-mcp/src hebrew-dictation-mcp/test
cd hebrew-dictation-mcp
git init
```

Verify you are NOT in the root repo before committing anything:

Run: `git rev-parse --show-toplevel`
Expected: a path ending in `hebrew-dictation-mcp` — **not** `claude-dev`. If it says
`claude-dev`, stop: `git init` did not take, and committing would pollute the shared repo.

- [ ] **Step 2: Create `package.json`**

```json
{
  "name": "hebrew-dictation-mcp",
  "version": "1.0.0",
  "private": true,
  "type": "module",
  "bin": { "hebrew-dictation-mcp": "dist/index.js" },
  "scripts": {
    "build": "tsc",
    "test": "vitest run",
    "smoke": "node test-client.mjs"
  },
  "dependencies": {
    "@modelcontextprotocol/sdk": "^1.29.0",
    "zod": "^4.4.3"
  },
  "devDependencies": {
    "@types/node": "^25.9.0",
    "typescript": "^5.5.2",
    "vitest": "~3.2.0"
  }
}
```

- [ ] **Step 3: Create `tsconfig.json`**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "Node16",
    "moduleResolution": "Node16",
    "outDir": "dist",
    "rootDir": "src",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "declaration": false
  },
  "include": ["src/**/*.ts"]
}
```

- [ ] **Step 4: Create `vitest.config.ts` (plain Node — NOT the Workers pool)**

```ts
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    include: ["test/**/*.test.ts"],
  },
});
```

- [ ] **Step 5: Create `.gitignore`**

```
node_modules/
dist/
```

`package-lock.json` is deliberately **not** ignored — it is committed for reproducible builds.

- [ ] **Step 6: Install and verify the toolchain**

Run: `npm install`
Expected: installs cleanly, no peer-dep errors. Generates `package-lock.json`.

- [ ] **Step 7: Commit**

```bash
git add package.json package-lock.json tsconfig.json vitest.config.ts .gitignore
git commit -m "chore(dictation-mcp): scaffold the MCP server project"
```

---

### Task 5: `DictationClient.fetchTranscript()`

**Files:**
- Create: `src/client.ts`
- Test: `test/client.test.ts`

- [ ] **Step 1: Write the failing test**

Create `test/client.test.ts`:

```ts
import type { AddressInfo } from "node:net";
import { createServer, type Server } from "node:http";
import { afterEach, describe, expect, it } from "vitest";
import { DictationClient, DictationUnavailableError } from "../src/client.js";

let server: Server | undefined;

/** Start a mock Local API on an ephemeral port; returns the port. */
async function startMock(body: () => string): Promise<number> {
  server = createServer((_req, res) => {
    res.writeHead(200, { "Content-Type": "application/json; charset=utf-8" });
    res.end(body());
  });
  await new Promise<void>((resolve) => server!.listen(0, "127.0.0.1", resolve));
  return (server!.address() as AddressInfo).port;
}

afterEach(async () => {
  if (server) {
    await new Promise<void>((resolve) => server!.close(() => resolve()));
    server = undefined;
  }
});

describe("fetchTranscript", () => {
  it("parses text, seq and at from the API", async () => {
    const port = await startMock(() =>
      JSON.stringify({ text: "שלום עולם", seq: 7, at: 1_700_000_000_000 }),
    );
    const client = new DictationClient(port);

    const got = await client.fetchTranscript();

    expect(got).toEqual({ text: "שלום עולם", seq: 7, at: 1_700_000_000_000 });
  });

  it("throws DictationUnavailableError when nothing is listening", async () => {
    // Nothing is bound to port 1, so the connection is refused immediately.
    const client = new DictationClient(1);

    await expect(client.fetchTranscript()).rejects.toBeInstanceOf(
      DictationUnavailableError,
    );
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test`
Expected: FAIL — cannot resolve `../src/client.js`.

- [ ] **Step 3: Write minimal implementation**

Create `src/client.ts`:

```ts
export interface Transcript {
  text: string;
  seq: number;
  at: number;
}

/** The app isn't running, or its Local API is disabled. */
export class DictationUnavailableError extends Error {
  constructor(port: number, cause?: unknown) {
    super(
      `Hebrew-dictation Local API is not reachable on 127.0.0.1:${port}. ` +
        `Make sure the app is running and "local_api_enabled" is true in its settings.`,
    );
    this.name = "DictationUnavailableError";
    this.cause = cause;
  }
}

export class DictationClient {
  constructor(private readonly port: number) {}

  /** Read the current transcript state. Never blocks. */
  async fetchTranscript(): Promise<Transcript> {
    let res: Response;
    try {
      res = await fetch(`http://127.0.0.1:${this.port}/transcript`);
    } catch (err) {
      throw new DictationUnavailableError(this.port, err);
    }
    if (!res.ok) {
      throw new DictationUnavailableError(this.port, `HTTP ${res.status}`);
    }
    const body = (await res.json()) as Partial<Transcript>;
    return {
      text: body.text ?? "",
      seq: body.seq ?? 0,
      at: body.at ?? 0,
    };
  }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test`
Expected: PASS (2 passed).

- [ ] **Step 5: Commit**

```bash
git add src/client.ts test/client.test.ts
git commit -m "feat(dictation-mcp): DictationClient.fetchTranscript"
```

---

### Task 6: `DictationClient.waitForNext()`

**Files:**
- Modify: `src/client.ts`
- Test: `test/client.test.ts`

- [ ] **Step 1: Write the failing test**

Append to `test/client.test.ts` (reuses the `startMock`/`afterEach` harness above):

```ts
describe("waitForNext", () => {
  it("resolves with the new transcript once seq advances past the baseline", async () => {
    let calls = 0;
    const port = await startMock(() => {
      calls += 1;
      // First two polls: unchanged. Third: a new dictation landed.
      return calls < 3
        ? JSON.stringify({ text: "ישן", seq: 5, at: 1 })
        : JSON.stringify({ text: "חדש", seq: 6, at: 2 });
    });
    const client = new DictationClient(port);

    const got = await client.waitForNext(5, 2_000, 10);

    expect(got).toEqual({ status: "new", text: "חדש", seq: 6, at: 2 });
  });

  it("times out when seq never advances", async () => {
    const port = await startMock(() =>
      JSON.stringify({ text: "ישן", seq: 5, at: 1 }),
    );
    const client = new DictationClient(port);

    const got = await client.waitForNext(5, 60, 10);

    expect(got).toEqual({ status: "timeout" });
  });

  it("aborts (does not silently retry) if the API dies mid-wait", async () => {
    // Kill the server from inside the 2nd poll, so the wait is genuinely
    // in-flight when the API disappears — not dead before it started.
    let calls = 0;
    const port = await startMock(() => {
      if (++calls === 2) {
        server!.close();
      }
      return JSON.stringify({ text: "ישן", seq: 5, at: 1 });
    });
    const client = new DictationClient(port);

    await expect(client.waitForNext(5, 2_000, 10)).rejects.toBeInstanceOf(
      DictationUnavailableError,
    );
    server = undefined; // already closed above; keep afterEach from double-closing
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test`
Expected: FAIL — `client.waitForNext is not a function`.

- [ ] **Step 3: Write minimal implementation**

Add to `src/client.ts`:

```ts
export type WaitResult =
  | ({ status: "new" } & Transcript)
  | { status: "timeout" };

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
```

and this method inside `DictationClient`:

```ts
  /**
   * Poll until a transcript NEWER than `sinceSeq` appears, or the timeout
   * elapses. Polling (rather than a server long-poll) keeps the app's
   * single-threaded tiny_http server untouched.
   *
   * If the API becomes unreachable mid-wait, the underlying
   * DictationUnavailableError propagates — deliberately no silent retry.
   */
  async waitForNext(
    sinceSeq: number,
    timeoutMs: number,
    pollMs = 500,
  ): Promise<WaitResult> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const current = await this.fetchTranscript();
      if (current.seq > sinceSeq) {
        return { status: "new", ...current };
      }
      if (Date.now() >= deadline) {
        return { status: "timeout" };
      }
      await sleep(pollMs);
    }
  }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test`
Expected: PASS (5 passed).

- [ ] **Step 5: Commit**

```bash
git add src/client.ts test/client.test.ts
git commit -m "feat(dictation-mcp): waitForNext polls until a fresh dictation"
```

---

### Task 7: MCP server + the three tools

**Files:**
- Create: `src/index.ts`

No unit test here by design — `index.ts` is pure wiring over the already-tested `client.ts`; Task 8's smoke client is its gate.

- [ ] **Step 1: Write `src/index.ts`**

```ts
#!/usr/bin/env node
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { DictationClient, DictationUnavailableError } from "./client.js";

const PORT = Number(process.env.DICTATION_API_PORT ?? 5757);
const client = new DictationClient(PORT);

const server = new McpServer({ name: "hebrew-dictation", version: "1.0.0" });

/** Render any thrown error as a tool error result instead of crashing. */
function toolError(err: unknown) {
  const message =
    err instanceof DictationUnavailableError
      ? err.message
      : `Unexpected dictation error: ${String(err)}`;
  return { isError: true, content: [{ type: "text" as const, text: message }] };
}

server.registerTool(
  "get_last_transcript",
  {
    title: "Get last transcript",
    description:
      "Return the most recent transcript dictated in the Hebrew-dictation app. " +
      "Does not wait — returns whatever was last dictated, which may be old. " +
      "Use wait_for_dictation if you need the user's NEXT utterance.",
    inputSchema: {},
  },
  async () => {
    try {
      const t = await client.fetchTranscript();
      return {
        content: [{ type: "text", text: t.text }],
        structuredContent: { ...t },
      };
    } catch (err) {
      return toolError(err);
    }
  },
);

server.registerTool(
  "wait_for_dictation",
  {
    title: "Wait for dictation",
    description:
      "Wait for the user to dictate something NEW, then return it. " +
      "This does NOT start the microphone: the user dictates with the app's own " +
      "hotkey (Alt+D) or UI. Tell the user to go ahead and speak, then call this. " +
      "Returns a timeout result if nothing new arrives in time.",
    inputSchema: {
      timeout_seconds: z
        .number()
        .int()
        .min(1)
        .max(600)
        .default(120)
        .describe("How long to wait for a new dictation (1-600s)."),
    },
  },
  async ({ timeout_seconds }) => {
    try {
      // Re-read the baseline every call: this is what makes a stale transcript
      // (or a seq reset after an app restart) impossible to mistake for fresh.
      const baseline = await client.fetchTranscript();
      const result = await client.waitForNext(baseline.seq, timeout_seconds * 1000);

      if (result.status === "timeout") {
        return {
          content: [
            {
              type: "text",
              text: `No new dictation within ${timeout_seconds}s.`,
            },
          ],
          structuredContent: { status: "timeout" },
        };
      }
      return {
        content: [{ type: "text", text: result.text }],
        structuredContent: { ...result },
      };
    } catch (err) {
      return toolError(err);
    }
  },
);

server.registerTool(
  "dictation_status",
  {
    title: "Dictation status",
    description:
      "Check whether the Hebrew-dictation app's local API is reachable. " +
      "Use this to diagnose setup problems.",
    inputSchema: {},
  },
  async () => {
    try {
      const t = await client.fetchTranscript();
      return {
        content: [
          {
            type: "text",
            text: `Local API is up on 127.0.0.1:${PORT} (last seq ${t.seq}).`,
          },
        ],
        structuredContent: { reachable: true, port: PORT, seq: t.seq },
      };
    } catch (err) {
      return toolError(err);
    }
  },
);

await server.connect(new StdioServerTransport());
```

- [ ] **Step 2: Verify it compiles**

Run: `npm run build`
Expected: `tsc` exits 0, `dist/index.js` exists.

> If `registerTool` is missing on the installed SDK, check the real export surface
> (`node -e "import('@modelcontextprotocol/sdk/server/mcp.js').then(m => console.log(Object.getOwnPropertyNames(m.McpServer.prototype)))"`)
> and use the equivalent registration method rather than guessing.

- [ ] **Step 3: Verify the unit tests still pass**

Run: `npm test`
Expected: PASS (5 passed) — `index.ts` must not break `client.ts`.

- [ ] **Step 4: Commit**

```bash
git add src/index.ts
git commit -m "feat(dictation-mcp): three MCP tools over stdio"
```

---

### Task 8: Smoke client + README

**Files:**
- Create: `test-client.mjs`, `README.md`

- [ ] **Step 1: Write `test-client.mjs`**

```js
// Smoke test: spawn the built server over stdio, list its tools, call
// dictation_status. Proves the MCP wiring works without a live app
// (dictation_status reports "unreachable" cleanly if the app is closed).
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const transport = new StdioClientTransport({
  command: "node",
  args: ["dist/index.js"],
});
const client = new Client({ name: "smoke", version: "1.0.0" });
await client.connect(transport);

const { tools } = await client.listTools();
console.log("tools:", tools.map((t) => t.name).join(", "));

const status = await client.callTool({ name: "dictation_status", arguments: {} });
console.log("dictation_status:", status.content[0].text);

// Exercise the get_last_transcript handler end-to-end too — it has no unit test
// (it's pure wiring), so this is its only coverage.
const last = await client.callTool({ name: "get_last_transcript", arguments: {} });
console.log("get_last_transcript:", last.content[0].text || "(empty)");

await client.close();
```

- [ ] **Step 2: Run the smoke test**

Run: `npm run build && npm run smoke`
Expected output: `tools: get_last_transcript, wait_for_dictation, dictation_status`, then a `dictation_status:` line and a `get_last_transcript:` line — either real values (app running with the API on) or the actionable "not reachable" message (app closed). **Either is a pass** — what's being proven is that the tools register, dispatch, and surface errors cleanly rather than crashing.

- [ ] **Step 3: Write `README.md`**

````markdown
# hebrew-dictation-mcp

MCP server exposing the [hebrew-dictation](../hebrew-dictation) app's transcripts to
Claude Code, Cursor, and local automation.

## Prerequisites

The app must be running with its Local API enabled — set `"local_api_enabled": true`
in its `settings.json` (off by default) and restart it. Default port `5757`.

## Tools

| Tool | What it does |
|---|---|
| `get_last_transcript` | Returns the last dictated transcript. Never waits — may be old. |
| `wait_for_dictation` | Waits for the user's **next** dictation and returns it. `timeout_seconds` (default 120). |
| `dictation_status` | Reports whether the local API is reachable. For diagnosing setup. |

> **`wait_for_dictation` does NOT start the microphone.** It only waits for a new
> transcript to appear. The user starts dictation themselves with the app's hotkey
> (Alt+D) or its UI. Tell the user to speak, *then* call the tool.

## Install

```bash
npm install && npm run build
```

## Register

Claude Code — add to `.mcp.json`:

```json
{
  "mcpServers": {
    "hebrew-dictation": {
      "command": "node",
      "args": ["C:/Users/אורח/claude-dev/AI-Tools/MCP-Dev/hebrew-dictation-mcp/dist/index.js"],
      "env": { "DICTATION_API_PORT": "5757" }
    }
  }
}
```

Cursor — the same object under `mcpServers` in `~/.cursor/mcp.json`.

## Develop

```bash
npm test          # unit tests (mock HTTP, no app needed)
npm run smoke     # stdio smoke client against the built server
```
````

- [ ] **Step 4: Commit**

```bash
git add test-client.mjs README.md
git commit -m "docs(dictation-mcp): smoke client and README"
```

---

## Done criteria

- `cargo test --manifest-path src-tauri/Cargo.toml` → 58 passed / 1 ignored; `cargo build` → 0 new warnings; `cargo clippy` → no new warnings in the 2 touched files.
- **The behavioral gate (Task 3, Step 6) passed:** with `streaming_enabled: true`, one long multi-segment sentence yields `seq` +1 and the **complete** sentence in `text` — not a fragment, not one bump per segment.
- `hebrew-dictation-mcp` is its **own git repo** (`git rev-parse --show-toplevel` ends in `hebrew-dictation-mcp`, not `claude-dev`) — no Chunk 2 commit landed in the shared root repo.
- `hebrew-dictation-mcp`: `npm test` → 5 passed; `npm run smoke` lists 3 tools and prints both a `dictation_status:` and a `get_last_transcript:` line without crashing.
- README carries the registration snippet and the "does not start the mic" note.
