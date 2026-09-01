# Hebrew Dictation — Session Handoff

> **Next session: read this + `memory/hebrew-dictation.md` + `memory/hebrew-dictation-changelog.md` to continue.**

---

## 🚀 v2.13.4 — RELEASED 2026-09-02 · why v2.13.3 never reached Henry

Henry reported (2026-09-02) "I don't see an update option" — investigation found his
running app was still **v2.13.2**, launched hours after v2.13.3 published. Root cause was
a chain of two silent gaps, not a fluke:

1. **`APP_VERSION` was a hardcoded const stuck at `"v2.11.0"`** through 12+ later releases
   — every settings screen and every feedback-email body lied about what was actually
   installed. Now read live via Tauri's `getVersion()` (`@tauri-apps/api/app`), which
   cannot drift from the running binary.
2. **No manual "check for updates" control existed anywhere.** The only trigger was the
   silent automatic check at launch, whose `catch` block swallowed every error with zero
   user-facing feedback — a real failure was indistinguishable from "no update exists".
   Added a "בדוק עדכונים עכשיו" button in Settings; the auto-check still fails silently
   (a launch shouldn't nag on a transient blip), but a manual check always reports a
   result now — found, up to date, or the actual error.

**Consequence worth flagging:** this means the v2.13.3 day-ordinal fix below was
**still unverified against Henry's real usage** at the time this was found — he'd been
stuck on 2.13.2 the whole time. First dictation after he actually updates to 2.13.4 (which
carries both fixes) is the real test of both.

Also from this session: a live "אצלי" (dropped-word) report couldn't be captured via the
MCP transcript tool (only exposes the LATEST dictation, and several attempts happened
between polls) — logged as unconfirmed, not fixed. Re-attempt with one dictation at a time
if this comes up again.

---

## 🚀 v2.13.3 — RELEASED 2026-09-01 · Deepgram day-name→Spanish-ordinal fix

Ran step 0א from `HANDOFF-TRANSCRIPTION-QUALITY.md` live with Henry (5 real dictations,
compared intended text / on-screen text / raw MCP transcript). Found the actual root
cause: Deepgram's `smart_format` reformats Hebrew day-names into the SPANISH ordinal
convention — "ביום שני" → "ביום 2º" (U+00BA, literally "2nd" in Spanish). Confirmed 3x
live, always right after "ביום". Screen matched the MCP transcript every time — this was
never an injection bug, and never needed the LLM-corrector/Gemini-unfreeze path the
original brief was written around.

Fix: `replace=` query params (7-pair table, Hebrew-only) added to all 4 Deepgram call
sites — 3 batch + streaming (the default path). Deterministic, zero LLM risk. Full
writeup + what was measured but NOT fixed (API→abi garbling, no dropped-words repro):
`memory/hebrew-dictation.md`'s v2.13.3 section.

⚠️ **Not verified against real Deepgram before shipping** (no key available outside the
running app). First dictation after auto-update on Henry's machine is the real test.

---

## 🚀 v2.13.2 — RELEASED 2026-08-31 · macOS fix wave (real user report)

A Mac user reported: works once, only inside the app, then "no model loaded" forever. A
16-agent adversarially-verified audit confirmed 5 bugs; all fixed and shipped the same
session. Full mechanics + the refuted TCC/ad-hoc-signing claim: the v2.13.2 section of
`memory/hebrew-dictation.md`. Commits `ecb2449` (fixes) + `d02de6a` (bump).

The five, in one line each:
1. Accessibility permission was undiscoverable (query-only check, error swallowed in
   frontend AND streaming) → prompt at startup + `is_accessibility_trusted` + surfacing.
2. Injection defocus dance had Windows semantics → app-level [NSApp hide:] on macOS
   (`MACOS_APP_HIDDEN` latch, `macos_unhide_if_needed` in window-show paths).
3. No `RunEvent::Reopen` → Dock click restored nothing after close-to-tray. Added.
4. `merge_frontend_update` clobbered the wizard's `onboarding_completed:true` (BOTH
   platforms) → latch + dedicated command + initApp self-heal + 2 regression tests.
5. RAM pre-flight rejected the small model forever on macOS (sysinfo subtracts the
   compressor) → guard now Windows-only.

**⚠️ Fixes 2+3 follow documented AppKit behavior but were NOT verified on real hardware
— the reporting Mac user is the tester. Ask Henry for his feedback before building
further macOS work on top of them.**

Release verified live: latest.json carries windows-x86_64 + darwin-aarch64 on 2.13.2,
all assets 200, site bumped and confirmed live (5 v2.13.2 refs on the page).
⚠️ New shared-tree trap variant hit during the site bump: Henry-Website was checked out
on another session's `feature/webinar-copilot`, so the commit landed there first.
Recovered (reset branch, cherry-pick to main, restored their checkout). **Check
`git branch --show-current` before committing in Henry-Website.**

---

## 🚀 v2.13.0 — RELEASED 2026-08-25

Everything in the two 2026-08-25 sections below shipped here, plus the whole narration feature
(56 commits that had reached `main` on 08-17 but were never cut as a release). Minor, not patch:
narration is a new user-facing feature.

Release commit `0824935` · [v2.13.0](https://github.com/aihenryai/hebrew-dictation/releases/tag/v2.13.0) = Latest · signed exe + `.sig` + `latest.json`.

Verified live, not assumed: updater endpoint returns `2.13.0` → the new Windows asset; installer
URL 200; `darwin-aarch64` block asserted byte-identical **in code before writing** (still v2.10.1,
mac auto-update intact); website shows the new link, `softwareVersion 2.13.0`, and narration both
in `featureList` and in the prerendered `content`, with zero stale `2.12.1` strings.

**Two things the documented release recipe gets wrong — fix the recipe, not just the symptom:**

1. **The version lives in THREE website places, not two.** The recipe says "tsx + prerender
   JSON-LD". There is also the prerendered human-readable `content` block, which said
   "הורידו את הגרסה האחרונה (v2.12.1)" and described the app as transcription-only long after
   the other two were correct. It is exactly what AI answer engines read. **Grep the DEPLOYED
   HTML for the previous version string before calling a release done** — the source tree looked
   complete while the live page was stale.
2. **Deploy-verification polls need a marker unique to the commit being verified.** The first
   poll matched a phrase the *previous* commit had already shipped inside `featureList`, so it
   reported success on attempt 1 against the old build. A substring that predates your change is
   not a deploy check.

**Known gap, disclosed to Henry before he approved the release:** the local-mode P0 is
test-covered but was never exercised live by a human who actually picked local mode — Henry
structurally cannot (he always has a Deepgram key). If a user reports local mode broken on
2.13.0, start there.

---

## 🔧 2026-08-25 (later) — narration playback + terminal window — UNCOMMITTED

Three reports from Henry about the narration screen. All three resolved.

### 1. Play button dead on every generated clip (`0:00 / 0:00`, greyed out)

Root cause: `src-tauri/tauri.conf.json` -> `app.security.csp` had **no `media-src`
directive**. CSP falls `media-src` back to `default-src`, which was `'self'` — and `'self'`
does not cover `blob:`. `App.tsx` feeds `<audio src>` from `URL.createObjectURL(blob)`, so the
webview rejected every clip with `MEDIA_ELEMENT_ERROR: Media load rejected by URL safety check`
and rendered the control disabled.

Fix: appended `media-src 'self' blob:` to the CSP. `data:` left blocked on purpose — the app
only ever uses blob URLs, so there is no reason to widen further.

Triage note for the future: "save as WAV" kept working the whole time because
`saveNarrationWav` passes `clip.bytes` through `invoke` — an entirely separate path from
`clip.url`. **Save works but playback doesn't = look at the webview/CSP layer, not the engine.**

### 2. A terminal window appears whenever narration runs

Root cause: the crate set `CREATE_NO_WINDOW` **nowhere**. A windows-subsystem (GUI) process owns
no console, so every console-subsystem child it spawns gets a brand-new *visible* console.

Fixed at all four production spawn sites — the remaining three `Command::new` hits in the crate
are inside `#[cfg(test)]`:
- `narration_process.rs` — python sidecar (the one Henry actually sees)
- `narration_provision.rs` — powershell `Expand-Archive`, `uv venv`, `uv pip install`

Constant is `pub(crate) const CREATE_NO_WINDOW` in `narration_process.rs`.
**Rule going forward: every new `Command::new` in this crate sets it.**

Second layer on the sidecar only: prefer `Scripts\pythonw.exe` (GUI-subsystem interpreter — can
never own a console), falling back to `python.exe` when absent so engines provisioned by older
builds still start. The reason for the belt-and-braces: the venv's `python.exe` is a *shim* that
re-execs the real uv-managed interpreter, and that hand-off isn't guaranteed to inherit our
creation flags.

### 3. "Will narration work for someone without Python/Node installed?" — yes, verified

Nothing was changed here; this is the audit result, checked in code rather than assumed:
- WebView2 ships **inside** the installer (`webviewInstallMode: offlineInstaller`).
- `uv venv --python 3.11 --managed-python` — `--managed-python` is the flag that guarantees it.
  Without it uv would happily satisfy `3.11` from a system install, silently making the engine
  depend on the user's Python.
- `uv` itself: downloaded pinned + SHA256-verified (0.12.1), unpacked via Windows' built-in
  `Expand-Archive`.
- Pip deps pinned exact: `onnxruntime==1.28.0`, `numpy==2.4.6`, `phonikud==0.4.1`,
  `phonikud-onnx==1.0.6`, `tokenizers==0.23.1`.
- Everything under app-data. No PATH edits, no registry writes.

**Only genuine requirement: an internet connection during first-time narration setup (~300MB+).**

### ⚠️ Verification lesson (cost most of this session)

The CSP hypothesis was "confirmed" three times by a broken harness before it was actually proven:
- **The agent's built-in browser pane and claude-in-chrome both refuse blob media
  unconditionally** — they reported "blocked" even with *no CSP at all*. Only a no-CSP negative
  control exposed this. Use `agent-browser` (unmanaged headless Chromium) for media testing.
- The synthetic test WAV was **8-bit PCM**, which Chromium won't decode — the control was failing
  for an unrelated reason. Fixtures must match the real artifact (the engine emits 16-bit PCM).
- Serving the fixed CSP verbatim kept `script-src 'self'`, which blocked the test page's own
  inline script — assertions hung on "running", readable as neither pass nor fail.

Final proof: same page, same 16-bit WAV — `LOADED dur=0.50` + **enabled** play button under the
fixed CSP; `ERR code=4` + disabled button under the original.

**Takeaway: a failing test means nothing until a passing case is demonstrated in the same harness.**

### State

`cargo check` + `clippy` clean (1 pre-existing warning at `narration.rs:106`), **122 tests pass,
0 fail**, `tsc --noEmit` clean. Uncommitted, stacked on the earlier 2026-08-25 pile in the same
working tree. Version deliberately not bumped.

Regression test added: `narration::tests::csp_allows_blob_media_so_narration_clips_can_play`
parses `tauri.conf.json` and asserts `media-src` lists `blob:`. Verified it actually fails when
the directive is removed and passes when restored — a test that can only ever pass would not
have been worth adding.

Local unsigned test installer (224MB, 2026-08-25 22:13):
`src-tauri	arget
eleaseundle
sis\הכתבה בעברית_2.12.1_x64-setup.exe`
The fixed CSP was confirmed present in the built `hebrew-dictation.exe` by byte-searching the
binary — a config change that silently fails to reach the bundle looks identical in source.
`npm run tauri build` exits 0 and then prints a signing error: expected, `TAURI_SIGNING_PRIVATE_KEY`
is intentionally unset because this is a local test artifact, not a release.

✅ **Live-verified by Henry 2026-08-25** — he installed this build and confirmed narration plays
in-app and no terminal window appears. Both bugs closed behaviorally.

**Open — needs Henry, not code:** whether to cut a real release. Worth weighing: the published
2.12.1 does not have these fixes, so every current user still hits the dead play button and the
popup console. The code side is done; only the release decision remains.

---

## 🔧 2026-08-25 — End-to-end audit + fix pass (5 waves) — UNCOMMITTED, locally tested only

Henry asked for a full scan (efficiency, bugs, security, design, UX, install flow) after
getting reports that the app "works on my machine" but not for some users, plus two symptoms
he sees himself: the floating bar is always on top, and it sometimes doesn't appear at all
despite being enabled in settings. Full findings are in the plan file
`C:\Users\אורח\.claude\plans\cosmic-popping-candy.md` (not part of this repo) — this entry is
the durable summary.

**Root cause of "works on my machine, not for users" — P0, highest confidence:** the
onboarding wizard's "local" branch left `streaming_enabled` at its default `true` without
disabling it (unlike the Groq branch, which does). `beginRecording` branched on
`streaming_enabled` alone, ignoring `transcription_mode` — so anyone who picked local mode
still tried to open a Deepgram websocket and failed with "מפתח Deepgram לא מוגדר" on every
single dictation. Henry always has a Deepgram key, so he never reproduced it. Fixed with a
single source of truth, `isStreamingSession()` (`App.tsx`), used by both start and stop so
they can never disagree — plus the wizard/settings-picker fixes so the flag doesn't get left
stale in the first place.

**Five waves, all verified (`cargo test --lib` 121 passed / 0 failed, `npx tsc --noEmit` clean,
`hebrew-dictation-mcp` vitest 17 passed):**
1. **P0 above** + `update_settings({patch})` binding bug (silently no-oped, `App.tsx:932`) +
   a settings-clobber bug in `initApp` that wrote factory defaults over real settings on the
   legacy `language:"auto"` migration path + Alt+D no longer a silent dead key when no
   engine is configured or the app is mid-transcribe (`canRecordRef` guard).
2. Floating-bar fixes — see the live-testing correction below, this wave's approach was
   **partially reverted** same day.
3. Install/first-run: onboarding wizard now resizes the main window (300×260 → 480×640) for
   the engine-choice step; local-mode wizard offers an actual model-download button+progress
   instead of a dead-end message; `model.rs` downloads now retry (up to 4 attempts) with real
   HTTP Range resume instead of restarting from zero on a dropped connection.
4. Security: local API (`local_api.rs`, port 5757) now requires a bearer token
   (`local_api_token` file in app-data, regenerated per launch) + `Host` header validation
   against DNS rebinding — `hebrew-dictation-mcp`'s `client.ts` updated to read and send it,
   backward-compatible with older app builds. Narration sidecar (port 5758) adoption now
   requires proving knowledge of a per-launch nonce (`current_nonce` file) via a
   double-check (`is_verifiably_ours` — must accept the right nonce AND reject a wrong one;
   a single accept-check would have passed for any process that ignores auth entirely, this
   was caught writing the test for it). `POST /synthesize` and `GET /info` on the Python
   sidecar both enforce the same nonce now. `narration_provision.rs`'s pip packages pinned to
   exact versions (`numpy==2.4.6` — deliberately NOT numpy's own latest 2.5.2, which has no
   Python-3.11 Windows wheel and would have broken provisioning outright; verified against
   PyPI before picking the number).
5. Hygiene: removed dead code (`InjectionMethod` enum, unused `arboard` dep, unreachable
   `stop_via_toolbar` command). Narration-setup failure messages split the Hebrew summary
   from raw uv/pip stderr into a separate `dir="ltr"` block instead of dumping Latin text into
   an RTL paragraph. Added a disk-space precheck (`check_disk_space`, 1200MB floor) before
   provisioning starts. **Explicitly deferred, on purpose:** a cancel button for narration
   setup (needs real subprocess-abort support), splitting the 3,861-line `App.tsx` monolith
   (high regression risk with no live visual QA pass), and a light-mode palette (the app is
   NOT "light-only" as one finding claimed — `App.css:1` is dark by default always; what's
   missing is an optional light variant for `prefers-color-scheme: light` users, which is a
   product decision, not a bug).

**Live-testing findings, from Henry actually clicking through the wave-1-4 dev build
(`npm run tauri dev`) — these are corrections ON TOP of the waves above, same day:**
- **Floating-bar always-on-top: wave 2's fix was wrong, reverted.** Wave 2 made the toolbar's
  always-on-top follow the same "חלון מעל הכל" setting as the main window. Henry tested it:
  turning the toggle off made the bar invisible DURING an active recording (it fell behind
  whatever app has focus — expected Windows z-order behavior for a non-topmost,
  non-focus-stealing window, but useless for a status bar you need to see while dictating).
  **Confirmed product decision: the floating bar must always stay on top, unconditionally,
  independent of the toggle.** Reverted `set_window_always_on_top` back to main-window-only;
  `show_toolbar_window`/`show_idle_button_inner` back to hardcoded
  `set_always_on_top(true)`. The settings toggle's Hebrew label was also rewritten — it used
  to say "show the RECORDING window above everything," which is what caused the confusion in
  the first place, since it never actually controlled the recording window even before wave 2.
- **Global hotkey registration does not retry.** `setup_global_shortcuts` runs once at Tauri
  startup; if it loses the race (e.g. an old installed instance is still holding `alt+d`), the
  NEW instance's Alt+D is dead until that instance is fully restarted — closing the
  conflicting process afterward does not help an already-running instance. Not code-fixed
  (would need explicit retry-on-focus or similar); worth knowing if "Alt+D doesn't work" comes
  up again — check for a second `hebrew-dictation.exe` process first.
- **NSIS `deleteAppData` checkbox text was visually truncated in the live installer/uninstaller.**
  Screenshot evidence from Henry. Root cause: the upstream English string is a short label
  ("Delete the application data", 28 chars); a past session expanded the Hebrew translation
  into a ~240-char paragraph explaining what gets deleted and that API keys are safe in
  Credential Manager. NSIS checkboxes are fixed-width, non-wrapping — the paragraph overflowed
  and rendered cut off. Fixed in `src-tauri/nsis/Hebrew.nsh` by shortening back to a label
  matching the English original's length; the detailed explanation was dropped from this
  control entirely (no safe way to preserve it in a single-line checkbox — would need a
  different UI surface, e.g. a MessageBox, if wanted back). **Only takes effect in an
  uninstaller generated by a build AFTER this fix** — an already-installed old version's
  `uninstall.exe` still has the old text until it's replaced by installing a new build.
  `scripts/check-nsis-translations.py` still passes (24/24 coverage).

**Current state — nothing committed, nothing pushed, nothing released:**
- All changes above are uncommitted local modifications (`git status` — 11 modified files
  under `src/` and `src-tauri/`, `Cargo.lock`/`Cargo.toml` for the `arboard` removal).
- Henry has a **local unsigned installer** built and tested at
  `src-tauri\target\release\bundle\nsis\הכתבה בעברית_2.12.1_x64-setup.exe` (built twice —
  second build picks up the NSIS string fix). This is NOT a GitHub release: `npm run tauri
  build` was run without `TAURI_SIGNING_PRIVATE_KEY`, so the updater `.sig` step fails
  (harmless — the bundle itself builds fine before that step; existing installs' auto-updater
  is untouched and unaware this build exists). Version number was deliberately NOT bumped —
  this is a same-version local test artifact, not a release candidate.
- **Still open / needs Henry's decision, not code:** whether to do a real release (version
  bump, sign with `~/.tauri/hebrew-dictation.key`, GitHub release, website update — the full
  recipe below) once he's satisfied with local testing; code signing (Authenticode, cost
  decision); full hash-pinning (`--require-hashes`) for the narration pip deps, beyond the
  version-pinning done in wave 4; whether a light color-scheme variant is wanted at all.

---

## ✅ 2026-08-15 — SHIPPED (unreleased): Hebrew narration — "צור קריינות", local, free, offline

Text → Hebrew speech, entirely on-device. Reachable from the home screen ("🔊 צור קריינות").
**Not yet released** — it is on `main`, unreleased, and Henry has used it but has not signed off
on the final round.

**Architecture — read this before touching anything.** The app provisions an isolated Python
environment into app-data using `uv` (never the user's Python — `--managed-python` enforces
that, and an A/B test proved that without it uv silently picked up the machine's system
CPython). It then runs **our own sidecar script** as a local HTTP server on `127.0.0.1:5758`
and talks to it over the same API piper's server used (`GET /info`, `POST /synthesize` → WAV).

**The engine is Phonikud, NOT piper.** This is the single most important fact here, and every
spec/plan doc in `docs/superpowers/` still describes the old design. piper phonemizes from
nikud, which encodes vowels but **not stress** — that is what made the output sound flat with
words running together, and no amount of parameter tuning fixes it. Phonikud (Interspeech
2026) emits stress-marked IPA. Henry A/B'd the shipped voice against two Phonikud checkpoints
and judged `michael` "much better". Because those voices declare `phoneme_type: "raw"` and
expect IPA, piper's own HTTP server cannot drive them — hence our own script
(`src-tauri/resources/narration_server.py`, bundled via `include_str!`, written to app-data
during provisioning). `piper-tts` is no longer installed at all.

| File | Role |
|---|---|
| `src-tauri/resources/narration_server.py` | the sidecar: text → nikud+stress → IPA → VITS ONNX → WAV |
| `src-tauri/src/narration.rs` | HTTP client, `NarrationParams`, argv builder |
| `src-tauri/src/narration_process.rs` | spawn/adopt/restart/shutdown + Windows Job Object |
| `src-tauri/src/narration_provision.rs` | provisioning, voice catalog, stage events, script sync |
| `src-tauri/src/lib.rs` | 6 Tauri commands (setup, generate, save, delete, list voices, set voice) |

**Cost:** ~695MB on disk, ~592MB RAM while the sidecar runs. The bulk is the 308MB
diacritizer. One-time install, ~7 reported steps.

**Known limitation, measured not guessed:** Latin words and digits inside Hebrew text garble
(0/4 exact ASR round-trips) — the phonemizer is Hebrew-only. The UI says so. The
short-utterance instability the old engine had is **gone** (4/4 exact, vs 0/6 before) — do not
re-add that warning. **No female Hebrew voice exists** in any free/open engine as of
2026-08-15; every open Hebrew voice traces to two or three male speakers. That ceiling is
real — see the spike-findings addendum.

**Traps that already bit, do not reintroduce:**
- Tauri exposes a Rust `snake_case` command arg to JS as `camelCase`. Passing `snake_case`
  binds **nothing** and an `Option<T>` silently arrives as `None`. This shipped once already
  (every saved file got a generic name). ⚠️ **There is still one live instance of this bug in
  `src/App.tsx:920`** — `invoke("update_settings", { patch: ... })` binds nothing, so that
  streaming-migration never persists and `.catch(() => {})` hides it. It is pre-existing and
  outside narration, so it was left alone. **Do not "fix" it by renaming `patch` →
  `newSettings`**: that would pass a partial object as a full `AppSettings` and serde-default
  every unlisted field. Send the full settings object with the one field changed.
- `uv venv` **errors** on an existing directory. Without `--clear`, any failure after the venv
  step made every retry fail there forever.
- Provisioning early-returns when the marker says Ready, so anything written only on the full
  path (the sidecar script) never reaches existing installs. `sync_server_script()` runs on
  both paths for exactly this reason.
- Only `NarrationError::Unreachable` may trigger a sidecar restart. A 5xx proves the process is
  alive (the script catches synthesis errors and keeps serving) and a timeout usually just
  means the text was long; restarting on those killed a healthy engine and paid a full cold
  start to fail identically.
- The sidecar script must validate its own input — it listens on a real TCP port any local
  process can reach. Before this was added, `sentence_silence: 1e9` made numpy attempt a
  **40 TiB allocation**, a negative value crashed on negative dimensions, and `length_scale: 0`
  returned HTTP 200 with 0.01s of garbage presented as success. All four now clamp/fall back.

## ✅ 2026-08-17 — RESOLVED: delete-engine "Access is denied (os error 5)" + settings-screen delete + review round 2

**The 2026-08-15 leading hypothesis (Job Object not covering the re-exec'd grandchild) was
WRONG.** Root cause, confirmed live on Henry's machine before writing any code: the two
processes from the bug report (PID 37508/28224, port 5919) were **still alive two days later**,
parented by `bash.exe`, not the app. They'd been started **by hand from a shell** while fuzzing
`narration_server.py` (see the sidecar-hardening entry above) — the app's
`AppState.narration_server` handle was `None`, so `delete_narration_engine` skipped its shutdown
step entirely and called `remove_dir_all` directly on a directory a live process had open. The
Job Object was never involved because the app never spawned that process.

**The fix (`f1ba6a4`):** `delete_narration_engine` no longer trusts the in-memory handle as the
sole authority. It now sweeps the whole engine directory for any live process by **executable
path** (never by process name — a `python.exe` name-match would kill Henry's other MCP servers),
kills what it finds, verifies death by polling, and only then deletes. `shutdown()` got the same
treatment: it now kills the sidecar's re-exec'd grandchild via the process tree, not just the
direct child, fixing a related bug in `synthesize_with_restart` where the Job Object's drop-timing
could let a "restarted" sidecar health-check the still-alive old process. `provision_narration_engine`
runs the same sweep before `uv venv --clear`, so a reinstall after a partial delete isn't blocked
by the same locked venv that blocked the delete.

**Settings-screen delete button — shipped (`f1ba6a4`).** New `narration_engine_footprint` command
+ a "מנוע קריינות" settings section, deliberately gated on **bytes-on-disk**, not the
`narrationReady` marker — the narration screen's own delete button lives inside a `narrationReady`
branch and disappears exactly when a half-deleted engine (marker gone, ~600MB still on disk) most
needs a way to clear it. This was the actual trap in Henry's original bug: after the failed
delete, there was no reachable button to try again.

**Review round 2 — the one that never ran, now complete (`3341105`).** Three independent agents,
one per dimension, all with zero prior review history:
- **Provisioning** (`narration_provision.rs`, first-ever review): 0 Critical, 3 Important. Fixed:
  `remove_dir_all_with_retry` now runs inside `spawn_blocking` (was blocking a tokio worker
  thread for the multi-second delete of ~700MB); `provision_narration_engine` gained a
  module-level async mutex against concurrent double-provisioning corrupting the shared
  `<dest>.tmp` download path. **Deliberately NOT fixed:** `ensure_uv_available` only checks
  whether `uv.exe` exists, not its version — a future `UV_VERSION` bump would silently keep using
  the old binary on any already-provisioned machine. Real bug, but `UV_VERSION` has never been
  bumped since ship and the fix needs a small version-sidecar-file design better made against an
  actual version bump to test with, not invented under review-fix pressure. Two Minor findings
  (placeholder-guard only covers 2 of 6 hash constants; the uv zip is deleted before the
  extraction success check) skipped as non-user-facing.
- **Frontend** (first-ever review): 0 Critical, 3 Important. Fixed: Generate/voice-switch/
  delete-engine buttons on the narration screen now cross-disable (Generate was missing
  `narrationDeleting`/`narrationSwitchingVoice` in its guard; Delete was missing
  `narrationGenerating`) — each gap let a synthesis call race a sidecar mid-teardown or
  mid-restart. Split `narrationEngineNotice` out of the shared `narrationSaveNotice` state: both
  the settings-screen and narration-screen delete confirmations lived inside the exact
  conditional block their own success turns false (`narrationFootprint > 0` /
  `narrationReady`), so a successful delete unmounted the confirmation in the same render pass
  it was meant to be read in.
- **The fix itself, adversarially** (commit `f1ba6a4`): 0 Critical, 1 Medium (the delete command
  holds the `narration_server` mutex across the whole sweep+kill+size+delete sequence — up to
  20-40s on slow disks with no progress feedback; judged intentionally correct, not fixed here —
  a real fix needs a cancellation token or progress event, not a quick patch), 1 Minor
  (`remove_dir_all_with_retry` retries blindly on any IO error kind, not just lock-race errors —
  low risk since the sweep already confirmed the directory is process-free by the time it runs).
  **Confirmed: the fix resolves the actual reported incident** — verified against a new `#[ignore]`d
  regression test that reproduces the exact incident shape (spawns an untracked sidecar by hand
  the way that shell did, proves the running exe is undeletable, proves the sweep clears both
  the shim and the re-exec'd grandchild).

Tests: **103 pass** (was 99 at the 2026-08-15 handoff), 5 ignored (network/full-engine, run with
`--ignored`). `tsc`/`vite build`/`clippy` all clean, same 7 pre-existing clippy warnings as
before this session, zero new ones.

**What Henry still has not confirmed:** the reworked install screen (never tested — unrelated to
this bug, carried over from the 2026-08-15 handoff). Everything about the delete flow itself —
the original bug, the settings-screen button, the cross-disable races — was fixed and covered by
tests this session, but not yet clicked through live by Henry.

**Pushed to `origin/main` this session** (see below) — the local-only backlog from 2026-08-15
is resolved.

---

## 🔬 2026-07-19 — RESEARCH: on-device vs cloud for Hebrew cleanup → recommend switching Groq → Gemini

Full cited report (3 research legs — Hebrew models, Windows/Rust runtime, cloud alternative): **`docs/research/2026-07-19-hebrew-cleanup-provider.md`**. Triggered by the macOS app [ghost-pepper](https://github.com/matthartman/ghost-pepper) (on-device dictation + cleanup). No code written.

**The finding:** on-device Hebrew cleanup is viable but lands in the worst quadrant — the only trustworthy ≤8B Hebrew model is **DictaLM-2.0-Instruct 7B**, but 7B on a CPU-only mid-range laptop is *tens of seconds* per cleanup (painful), while the fast 1–3B tier has weak Hebrew. Plus a real engineering landmine (whisper.cpp + llama.cpp both static-link `ggml` → Windows `LNK2005`; must run llama.cpp as a **sidecar**, not a 2nd FFI crate).

**The recommendation:** the rate-limit pain is a **Groq-specific artifact** (100k TPD is unusually low). **Switch Smart Cleanup Groq → Gemini 2.5 Flash-Lite** — free/pennies, a ~3-line change (Gemini has an OpenAI-wire endpoint; `enhance_inner` already speaks it; the `finish_reason` fix carries over), and a *measured Hebrew quality upgrade* (Gemini beats Groq's Llama-3.3-70b on the rewriting proxy AND on Nikud — Groq scores a catastrophic 4.05). ⭐ **This also un-shelves the translation feature** — its only blocker was Groq's daily-token cap, which Gemini doesn't have. **The provider question left open in the translation spec is now answered: Gemini.** On-device stays a *later, opt-in "private cleanup" tier* (DictaLM-7B sidecar, explicitly slower — the CallLocal analogue), only if there's demand.

**⏸️ PAUSED 2026-07-29 — Henry's call, not decided/approved yet.** He wants to hold off: it's a dramatic-enough change (swapping the shipped Smart Cleanup provider) that he wants to sit with it, and correspondingly the translation revival waits too (it was only unblocked by this same decision). His stated hesitation was whether Gemini API keys need a credit card — **checked, they don't**: Google AI Studio issues Gemini API keys with no credit card and no expiration on the free tier (1,500 req/day, 15 RPM on Flash-class models incl. Flash-Lite) — that's a genuinely no-card path, separate from Vertex AI which does require billing. So the credit-card concern is resolved whenever he wants to revisit; the pause itself stands regardless. **Do not implement the switch or revive translation until Henry explicitly says go.**

---

## ✅ 2026-07-19 — FIXED: `enhance_inner` silently accepted truncated Groq completions (`dd082f8`)

Found while designing translation; **independent of that feature — it was a real defect in the shipped Smart Cleanup path.** `enhance_inner` read `choices[0].message.content` without checking `finish_reason`, so a completion Groq cut off at its output cap returned partial text that — being shorter than the input — passed `validate_output`'s only guards (empty, >2× length). Silent truncated cleanup, no error.

Fixed via a pure `parse_completion(body, raw)`: `finish_reason == "length"` → new `EnhanceError::Truncated`, then the existing content validation. Matched explicitly on `"length"` (not `!= "stop"` — the file's `as_str().unwrap_or("")` reads an absent field as `""`, which would fail every response). 5 parser-level tests on canned bodies including the absent-field trap. `cargo test` **68 / 1 ignored**, clippy clean. Not released — ships with the next build.

---

## 🧊 2026-07-19 — EN→HE translation: spec written and 3× reviewed, then SHELVED (Henry's call)

Spec: `docs/superpowers/specs/2026-07-19-en-to-he-translation-design.md` — brainstormed with Henry, three full review rounds, all issues fixed. **No code written.** Stopped at the pre-implementation gate because the scope grew far beyond the original "add an `EnhanceMode` variant" framing *and* a feasibility check came back badly.

**⛔ The feasibility finding — read this before reviving the feature.** Groq free tier for `llama-3.3-70b-versatile`: **12,000 TPM / 100,000 TPD** ([official](https://console.groq.com/docs/rate-limits)). A 1-hour podcast ≈ 60,000 chars ≈ 20 chunks ≈ **~50,000 tokens**, so:
- TPM caps throughput at ~4-5 chunks/minute → a 1-hour file takes **4-5 minutes minimum**, no matter how fast Groq is. Pacing is required, not just backoff.
- **One podcast burns half the daily token budget.** Two per day hits the cap.

→ **If revived, decide the provider first.** Henry has `OPENAI_API_KEY` and `GEMINI_API_KEY` too; bulk translation is a completely different load profile from cleaning one dictated sentence, and the two need not share a provider. Do not start implementing before this is settled.

**What the spec settled (Henry's decisions, don't re-litigate):** picked files only (not dictation, recordings, or meeting modes) · per-action checkbox, never a persistent global mode · failures are loud, English left intact · Hebrew replaces the English · chunked translation (a single call is provably unworkable — spec §4.1).

**Non-obvious traps the reviews found, all already written into the spec:**
1. The 2× output ceiling **rejects correct short translations** — `"AI"` → `"בינה מלאכותית"` is 6.5×. The ceiling must become mode-owned.
2. **`ivrit-*` whisper models force Hebrew regardless of the language argument** (`whisper.rs` ~82-86, ~187-192), so an English file would decode to garbage and then be "translated". Needs a pre-flight guard.
3. **Translation desynchronizes `transcript` from `segments`**, so SRT export would emit English subtitles under a Hebrew transcript. Needs a `translated` flag in `isSrtEligible` — *not* reuse of `edited`.
4. Batch **hardcodes `language: "he"`** (`App.tsx:1006`) and never reads the global setting.
5. Meeting-mode text carries `"אני: "` / `"הצד השני: "` as plain content an LLM would mangle.

---

## ✅ 2026-07-29 — v2.12.1 FULLY RELEASED (reliability release, no consumer-facing feature)

App `main` @ `bfd93ed`-equivalent (app repo HEAD `a789ab3`, pushed). **GitHub Release [v2.12.1](https://github.com/aihenryai/hebrew-dictation/releases/tag/v2.12.1)**, signed `.exe` + `.sig` + `latest.json` (darwin-aarch64 block preserved byte-identical, still v2.10.1). Updater endpoint verified 200 → `2.12.1`, both platform blocks present. **Website** (`Henry-AI-website` `bfd93ed`) bumped: `DOWNLOAD_URL` + both version badges in `HebrewDictation.tsx`, `softwareVersion`/`downloadUrl`/`installUrl` + the "latest version" mention in `prerender-seo.js`. Verified live: `bintechai.com/hebrew-dictation` 200 w/ `softwareVersion:2.12.1`, `henry-ai-website.pages.dev/hebrew-dictation` 301→bintechai.com (pre-existing canonical redirect, not new) resolving 200 w/ 2.12.1, GitHub download link 200. Contains the local-api `/transcript` fix, the Smart Cleanup `finish_reason` truncation fix, and the 2026-07-29 injection-gate fix above. No "מה חדש" feature cards added — there's no new user-facing capability this round, matching the release-recipe note that a version bump alone is correct when that's genuinely all that changed.

⚠️ **The MCP server can now be exercised** — 2.12.1 (once the user installs it) returns `{text, seq, at}`, satisfying the MCP's payload validator. Mic-verify (below, #1) still needs Henry.

## (superseded by the above) 📦 2026-07-19 — v2.12.1 RELEASE PREPARED (version bumped, green) — awaiting Henry's signed build

Version bumped **2.12.0 → 2.12.1** in all 4 files (package.json, Cargo.toml, tauri.conf.json, Cargo.lock, commit `e1fc6ec`). Release candidate verified: `cargo test` **68/1**, `tsc && vite build` clean. **Not pushed, not built, not released** — the signed build needs Henry's `TAURI_SIGNING_PRIVATE_KEY`.

What 2.12.1 contains: the `/transcript` `{text, seq, at}` rework (was returning fragments under streaming) + the `finish_reason` truncation fix in Smart Cleanup. No consumer-facing UI change — this is a reliability release plus a dev-only opt-in API.

**Henry's remaining steps are a runbook** (build → rename → merge latest.json keeping the darwin block → `gh release create` → website version bump only): it was written to the session scratchpad (`v2.12.1-runbook.md` + `v2.12.1-release-notes.md`). If that scratchpad is gone, the recipe is in `memory/hebrew-dictation.md` and the v2.12.0 changelog entry; the only new wrinkles this round are (a) the website "מה חדש" headline is an editorial call since there's no consumer feature, and (b) `prerender-seo.js` showed no dictation version string — verify before bumping it.

⚠️ **The MCP server cannot work until 2.12.1 is installed** — the running 2.12.0 returns `{text}` only, which the MCP's payload validator correctly rejects. So the mic-verify (below, #1) and MCP registration both wait on this build.

---

## 🆕 2026-07-19 — dictation MCP server + a latent `/transcript` bug fixed. NOT RELEASED.

Full brainstorm → spec → plan → subagent-TDD, two review gates per task. Spec: `docs/superpowers/specs/2026-07-19-dictation-mcp-design.md`. Plan: `docs/superpowers/plans/2026-07-19-dictation-mcp.md`.

**App (`main`, `43cf9fd`→`0b5dbee`, 63 tests / 1 ignored, 0 warnings):**
- `GET /transcript` now returns **`{text, seq, at}`** (additive — `text` still there). `seq` = monotonic counter, `at` = unix ms.
- 🐞 **Latent bug fixed:** `streaming_enabled` defaults **true**, and `streaming.rs:163` injects **per final segment**. The old stamp lived inside `inject_text_defocused`, so the shipped `/transcript` returned only the **last fragment** of a streaming dictation — i.e. it has been returning fragments to anyone using it since it shipped. Stamping moved to the two **utterance boundaries**: the `inject_text` command (`lib.rs`) and `stop_streaming_transcription`. `local_api::record_utterance` is now the single mutation door (`bump_transcript`/`transcript_json`/`now_unix_ms` are private, so the empty-guard can't be bypassed).
- ⚠️ **`seq` counts completed-transcript EVENTS, not spoken utterances** — a paste of an older batch result bumps it too and stamps `at` with *paste* time. This distinction escaped into docs **four times** during the build (field comment → struct doc → module header → tool description). If you touch this, grep the whole crate for the claim; don't just fix the site you were pointed at.

**MCP server — NEW: `AI-Tools/MCP-Dev/hebrew-dictation-mcp/` (its OWN git repo, `574319d`→`e184adb`, 12 tests):**
- Node/TS, `@modelcontextprotocol/sdk` ^1.29, **stdio**. `src/client.ts` (all logic, zero MCP imports, unit-tested vs a real local mock HTTP server) + `src/index.ts` (pure wiring). README has the `.mcp.json` snippet.
- Tools: **`get_last_transcript`** (non-blocking) · **`wait_for_dictation`** (blocks until `seq` exceeds a per-call baseline) · **`dictation_status`** (diagnostics).
- ⚠️ **It never starts the mic.** It only watches the counter; you dictate with the app's own hotkey. The name misleads — the descriptions say so explicitly.
- ⚠️ **`timeout_seconds` defaults to 55, not 120, on purpose.** The MCP SDK's `DEFAULT_REQUEST_TIMEOUT_MSEC = 60000`, so a client aborts the tool call at 60s with a generic error before a longer wait could return gracefully. Measured: 55.4s graceful, 4.6s margin. The 1..600 range is real but >55 needs the *client's* request timeout raised. Proper fix (progress notifications) deferred — needs a `progressToken` the client may never send; analysis is in a `// TODO` in `index.ts`.

### ⏳ What's left — the core chain is done (1-3 all resolved 2026-07-29); only minor/deferred items remain

1. ✅ **RESOLVED 2026-07-29 — mic-verify end-to-end, done with Henry live.** `local_api_enabled: true` set, app updated to 2.12.1, three separate Alt+D→speak→Alt+D cycles captured by polling `/transcript`: `seq` 1→2→3→4, each bump exactly +1, each `text` a complete, non-truncated sentence (e.g. `"אוקיי, אני מכתיב 12343567 אני מכתיב ואני מדבר אני עוצר"` — ends cleanly at "אני עוצר", not mid-word). Falsifies both the per-fragment-bump risk and the 5s-trailing-message truncation risk from spec §6. The mic → Deepgram/streaming → `record_utterance` → `/transcript` chain is confirmed working on real hardware.

2. ✅ **RESOLVED 2026-07-29 — registered.** `claude mcp add hebrew-dictation -s user -e DICTATION_API_PORT=5757 -- node "<repo>/dist/index.js"` (**user scope**, so it's available across every project/session, not just this one). `claude mcp list` shows `hebrew-dictation: ... - ✓ Connected`. Lives in `~/.claude.json`, not `.mcp.json` (Claude Code's per-project file) — the README's exact snippet works too if a project-scoped copy is ever wanted instead/in addition. **Every dependency in the MCP chain is now live and proven**: app released (2.12.1) → local API mic-verified (#1) → server registered and connected (#2). ⚠️ A session already running when this was registered won't see the new tools — they show up starting with the next fresh Claude Code session.

3. ✅ **RESOLVED 2026-07-29 — the non-streaming injection gate.** `inject_text` used to record only `if result.is_ok()`, while `stop_streaming_transcription` recorded unconditionally — so with `streaming_enabled: false`, a dictation whose OS-level paste failed (`injector.rs:42,45`; `App.tsx:495-501` swallows it with an empty `catch {}`) never bumped `seq`, and `wait_for_dictation` would time out as if the user hadn't spoken. Fixed by recording unconditionally in `inject_text` too (`lib.rs`), matching the streaming path and the module's own "last completed transcript" doc comment — a transcription completing is independent of whether the clipboard-paste into the active field then succeeded. `cargo build`/`cargo test` (68/1) / `cargo clippy` all clean, no new warnings. Released in v2.12.1 (see below).

4. **Known UX seam (inherent, not a bug).** The app injects into whatever holds OS focus — when you dictate *for the agent*, that's the Claude Code prompt box. The sentence lands in your input box **and** returns through the tool, so you must clear the box before pressing enter. Unavoidable while "no remote-record trigger" stays a non-goal.

5. **Deferred, recorded in TODOs:** `outputSchema` for the three tools (purely additive later) and `structuredContent` shape consistency (**not** additive once automation writes against it — currently a non-issue only because the package is `"private": true`, publishing is a spec non-goal, and the sole consumer is in-workspace). Minor: `dictation_status`'s `reachable` can never be `false` (the unreachable path returns `isError` instead); `record_utterance` is arguably misnamed given `seq` counts events not utterances; README doesn't name the `local_api_port` setting.

---

## ✅ v2.12.0 — FULLY RELEASED (2026-07-14) — meeting transcription

**Released end-to-end + verified each hop (not assumed):**
- **App:** `main` @ `b91e076`, version bumped in all 3 files (package.json / Cargo.toml / tauri.conf.json). All work pushed to `origin/main`.
- **GitHub Release [v2.12.0](https://github.com/aihenryai/hebrew-dictation/releases/tag/v2.12.0)** = **Latest**, signed `.exe` (204MB) + `.sig` + `latest.json`. ✅ **macOS block in `latest.json` preserved byte-identical** (still points to v2.10.1 — mac auto-update NOT broken). Updater endpoint `releases/latest/download/latest.json` verified HTTP 200 → `2.12.0`, both platform blocks present.
- **Auto-update IS LIVE:** every Windows user on 2.11.0 gets 2.12.0 on next launch.
- **Website** `bintechai.com/hebrew-dictation` (repo `Henry-AI-website` @ `c1f8d7d`) → 2.12.0, real "מה חדש" copy (2 meeting-mode cards replacing SRT/batch), new answer-first `<section>` in `prerender-seo.js` content + 2 `featureList` entries. Both domains verified live HTTP 200 with the new content. (No arrows — site RTL rule.)

**What shipped in 2.12.0** (full brainstorm→spec→plan→subagent-TDD, specs/plans in `docs/superpowers/{specs,plans}/2026-07-12-recording-modes-ux.md`):
1. 🔒 **`CallLocal` — private on-device meeting:** mic+system → `audio::mix_to_mono` → **forced-local whisper**, no speaker separation, audio never leaves the machine. Pre-record model guard. `cargo test` 55/1, two-stage review ✅.
2. 📞 **`CallCloud`** (renamed from `Call`) — speaker separation "אני:/הצד השני:". **Real-audio VERIFIED in a live Zoom call 2026-07-13.**
3. 🎛 **Source-selector regroup:** two groups (הקלטה רגילה / פגישות) + a **light `engine-toggle`** (☁ ענן / 💾 מקומי) at the top that dims for meetings. Meeting cards benefit-labeled, symmetric desc lines ("כל אחד בנפרד" vs "יחד, ללא הפרדה לדוברים"). ⚠️ the "· קובעות מנוע בעצמן" header note was tried and **Henry rejected it as redundant — do not re-add**.
4. ⚙ **Settings reachable from the batch screen too** — labeled turquoise `.btn-settings-labeled` (⚙ הגדרות) on home + batch header; tracks a **return view** so "חזור" from settings returns to origin. ⚠️ **CSS class-name collision lesson:** a labeled settings button was first named `.btn-settings`, which already existed (a 36px round icon rule) → it was forced into a circle with the label spilling out of the frame. Renamed to `.btn-settings-labeled`. **Before adding a class, grep App.css for the name.** The `.container` is only **340px** wide — measure header/controls rows for overflow (DOM measurement harness works when screenshots don't).
5. 🐞 **Onboarding API-key silent-discard bug FIXED** (`4f4ac3c`): the `.wizard-card` div's `onClick` reset the form and the key `<input>`/"בדוק" button were nested inside it, so a click bubbled up and wiped the typed key → `set_api_key` never called, no error. Fixed 3 ways (early-return on re-click, `stopPropagation`, and `completeOnboarding` now re-reads `get_settings` to VERIFY the key landed). Full write-up in `memory/hebrew-dictation.md`.

### ⏳ ONLY open item — real-audio manual verify of the LOCAL meeting mode (Henry, can't automate)
`CallCloud` was verified in Zoom; **`CallLocal` (🔒 פגישה — פרטית במכשיר) has NOT been exercised on real audio yet.** With a whisper model downloaded (Henry has `small`): install 2.12.0 → תמלול קובץ → 🔒 פגישה — פרטית במכשיר → record while system audio plays → stop → expect ONE local mono transcript, **no "אני:/הצד השני:"**. Low-risk (reuses the same dual-recorder capture Zoom already proved; only the mix+local-transcribe tail is unexercised on live audio), but not yet confirmed. If it fails, that's the thing to debug next session.

### Backlog (unchanged, not scheduled)
English→Hebrew translation · Windows code-signing cert (kills SmartScreen "unknown publisher") · Local-API MCP wrapper (#1 local API shipped, #2 MCP adapter pending) — see the "Voicebox comparison" section below.

---

## ✅ v2.11.0 — FULLY RELEASED (2026-07-05)

**Released end-to-end:** app on `main` (commit `0078eed` HEAD at release time) · GitHub Release [v2.11.0](https://github.com/aihenryai/hebrew-dictation/releases/tag/v2.11.0) with signed `.exe` + `.sig` + `latest.json` · website `bintechai.com/hebrew-dictation` updated with real "מה חדש" copy (not just version bump) · Henry manually verified everything in `npm run tauri dev` before release.

**Windows only.** macOS stays on v2.10.1 pending Yogev's next build — expected lag, not a bug (see macOS release recipe in `memory/hebrew-dictation.md` "Known Limitations").

## ⚠️ Found after release: real Mac user hit "app is damaged, move to Trash"

Not Hebrew/localization, not a Claude Code mistake — classic Gatekeeper "damaged" dialog (stricter than the usual "unidentified developer" one, no "Open Anyway" bypass). Likely cause: the `.app` was zipped for distribution with plain `zip`/Finder-compress instead of `ditto`, which can corrupt the ad-hoc signature; combined with the browser-download quarantine flag, Gatekeeper refuses to open it. **Before the next mac build request to Yogev, send him the packaging instructions in `memory/hebrew-dictation.md`'s macOS release recipe section (`ditto -c -k --sequesterRsrc --keepParent` + `codesign`/`spctl` verification) — don't let this repeat.** Website fixed in **three passes** (`151224c` → `51ea4fb` → `46d2edc`): first buried the Mac fix as a gray aside inside a box titled "Windows: SmartScreen" (Henry couldn't find it even after it shipped); second gave macOS its own equally-weighted, clearly-titled box; third swapped a "type xattr -cr then drag the file in" two-step for one direct copy-paste command with the exact filename Henry confirmed from the user's screenshot (`~/Downloads/הכתבה\ בעברית.app`) — he explicitly prefers a ready-to-run command with a stated assumption over a fool-proof multi-step flow, see `memory/feedback_copypaste_full_command.md`. **Lesson 1:** platform-specific help text needs an equally prominent, correctly-labeled home, never nested in the other platform's box. **Lesson 2:** this "one ready command, not a two-step dance" preference applies to instructions Henry forwards to end users too, not just commands he runs himself.

## 🍎 macOS audit + fixes (2026-07-07) — "no audio" root cause found & fixed in-repo

A Mac user (on the lagging v2.10.1 build) kept hitting **"לא נקלט קול מהמיקרופון … הגדרות Windows ← פרטיות ← מיקרופון"**. Full audit (systematic-debugging + codebase sweep) found the app has **zero macOS configuration** — no `bundle.macOS`, no `Info.plist`, no entitlements, and **not one `#[cfg(target_os="macos")]` anywhere**. That single gap cascades into every Mac problem below.

### ✅ Fixed in-repo this session (Windows-verified: 29/29 tests green; `generate_context!` accepts the new config)
- **P1 · mic permission = root cause of "no audio".** Capture is **native** (cpal→CoreAudio, NOT WebView getUserMedia), so macOS TCC gates it. With no `NSMicrophoneUsageDescription`, the OS denies the mic → cpal gets **silence** → the `is_effectively_silent` guard (`lib.rs:262`) fires. Fix: added `src-tauri/Info.plist` (`NSMicrophoneUsageDescription` — Tauri auto-merges it) + `src-tauri/Entitlements.plist` (`com.apple.security.device.audio-input` — required under Tauri's default hardened runtime) + a `"macOS"` block in `tauri.conf.json` (`entitlements` + `minimumSystemVersion: 10.15`). NB: the Info.plist key helps immediately; the entitlement matters once hardened-runtime/signing is on.
- **P2 · error text sent Mac users to *Windows* Settings.** String was hardcoded, no platform branch. Fix (TDD): pure `mic_permission_path_for(os)` helper in `lib.rs` (macOS → "הגדרות המערכת ← פרטיות ואבטחה ← מיקרופון"), wired at `lib.rs:262`, new test `mic_permission_path_is_platform_specific`.

### ⚠️ MUST verify on a real Mac (can't from Windows)
Rebuild the Mac app with the new config → does the **mic-permission prompt** appear on first record and audio capture? **Immediate triage for the current user:** does **"הכתבה בעברית"** appear under **System Settings → Privacy & Security → Microphone**? Listed → toggle on (instant relief). **Not listed** → confirms the missing-usage-string root cause; needs the rebuild.

### 🔧 Still open — needs macOS-specific code + a Mac to test (NOT started)
- **P3 · text injection (enigo→CGEvent) needs Accessibility permission.** ✅ *Partial (this session):* added a `#[cfg(target_os="macos")]` `AXIsProcessTrusted()` guard in `injector::inject_text` that returns actionable Hebrew guidance (→ הגדרות המערכת ← פרטיות ואבטחה ← נגישות) instead of silently typing nothing. Windows-verified (cfg-excluded there; hint unit-tested) — but **the macOS FFI itself is unverified; must compile-check on a real Mac build.** ⏳ *Remaining:* the guidance only reaches the **command/batch** path — the default **streaming** path does `let _ = inject_text_defocused(...)` (swallows the Err), so streaming users still get no feedback. Needs a proactive check/banner (`AXIsProcessTrustedWithOptions` prompt, or a startup/dictation-start check). `inject_text_defocused`'s 80ms hide/restore (`lib.rs:993-1042`) is also Windows-tuned.
- **P6 · default hotkey `alt+d` = Option+D on Mac** (dead-key → `∂`); registers but poor UX. Consider a `#[cfg(target_os="macos")]` Cmd-based default (avoid Cmd+D = bookmark). UI hardcodes "Alt + D" strings too.
- **P7 (low) · no cpal sample-format negotiation** (`audio.rs` assumes f32) — a non-f32 default input would fail.

### 🏗️ Needs Yogev's Mac + Apple Developer ID (build/signing, not code)
- **P4 · "app is damaged" (Gatekeeper)** — no signing/notarization (ad-hoc sig, fragile; see the packaging section above). Real fix: Developer ID signing + notarization + `ditto`. Same signing-cert decision as Windows SmartScreen (both un-decided).
- **P5 · no Mac auto-update** — `bundle.targets: ["nsis"]` emits Windows artifacts only, so `latest.json` has no macOS entry. Add a macOS bundle target on the Mac build.

### ❓ Coordination blocker — confirm before assuming the fix ships
**How does Yogev build the Mac version?** `targets: ["nsis"]` (Windows-only) means his Mac build overrides the target somehow. Builds **from this repo** on a Mac → P1 config flows in automatically. Uses a **separate config/fork** → the new `Info.plist` + `Entitlements.plist` + `bundle.macOS` block must be ported to his setup.

## 🎯 NEXT UP: meeting transcription — Zoom/Meet audio + speaker diarization

Both ideas were flagged from the island-io/mila comparison. Diarization (#1) is now **code-complete** (below); system-audio (#2) is next and still needs a design pass.

### ✅ 1. Speaker diarization — CODE-COMPLETE this session (TDD, 2026-07-07), one live check before ship
Done via red-green-refactor (the Deepgram parser had **zero** tests before — now covered). Touched only `srt.rs` + `api_transcribe.rs` (+ `whisper.rs` sets `speaker: None`), exactly as scoped:
- `TimedWord`/`TimedSegment` gained `speaker: Option<u32>` (`#[serde(default)]` on the segment so it survives the export IPC round-trip). `chunk_words_to_cues` now splits a cue on speaker change; `flush_cue` stamps each cue's speaker. With diarization off every word is `None`, so `None != None` never fires → behavior byte-identical to before.
- Extracted `parse_deepgram_words()` (pure, unit-tested) reads `w["speaker"]`; `transcribe_deepgram_batch` calls it and now sends `&diarize=true`. **Cloud-only** by design — whisper.cpp has no diarization (mirrors the existing "streaming is Deepgram-only" precedent).
- **Output = auto-label (Option A):** `render_srt` prefixes cues with `דובר 1:` / `דובר 2:` **only when a file actually has ≥2 speakers** — single-speaker dictation stays byte-for-byte clean, multi-speaker calls get labels with no toggle. Counted per-file (a clean file in a mixed batch export isn't labeled just because a sibling had two speakers). To switch to always-on / a manual checkbox = flip one condition in `render_srt`. Numeric labels only (Deepgram 0 → "דובר 1"), no name matching.
- **No TypeScript change needed:** App.tsx passes `segments` back to `export_srt` opaquely (pass-through, not reconstructed), so `speaker` survives without touching the TS interface. Surfacing speakers in the UI transcript view = optional follow-up, not v1.
- **Tests: 28/28 green** (`srt` 12, incl. 3 new speaker tests; `api_transcribe` 2 new parser tests — first ever for that parser). No new compiler/clippy warnings in the touched files.
- ⏳ **SHIP-GATE (only open item):** one real diarized request — `nova-3` + `he` + `diarize=true` on a **2-speaker Hebrew** clip — to confirm Deepgram actually populates `speaker` in `words[]` (the flagged 2026-05 "Batch Diarization v2" nuance) and the exported SRT shows `דובר 1:/דובר 2:`. Needs Henry's Deepgram key + a two-person recording (or just run the app on any 2-voice audio → export SRT). **Not yet run. Not yet committed.**

### 2. System-audio capture for meetings — ✅ ALL 20 TASKS DONE + reviewed 2× + PUSHED · ✅ REAL-AUDIO VERIFIED 2026-07-13 (Henry, live Zoom call, CallCloud/"פגישה בענן" — speaker separation "אני:/הצד השני:" worked). NOT released to users.

> **State:** brainstormed → spec approved (`docs/superpowers/specs/2026-07-09-system-audio-capture-design.md`, `e44977e`) → **20-task TDD implementation plan** authored + adversarially reviewed (`docs/superpowers/plans/2026-07-09-system-audio-capture.md`) → **fully implemented under strict TDD via subagent-driven-development (2026-07-10).**
> **Design locked:** three sources (`Mic`/`System`/`Call`). Call captures mic + system separately → stereo WAV (L=mic, R=system) → Deepgram `multichannel=true` → "אני"/"הצד השני". Batch-only v1, cloud-only (multichannel is Deepgram-only), Windows-only via the `wasapi` crate.
> **DONE (2026-07-10):** Chunks 3-6 (Tasks 6-20) all landed as atomic commits `68a688a` → `0073d8b`, plus a post-review Critical fix `af30355`. `cargo build` = **0 warnings**, `cargo test` = **50 passed, 1 ignored** (the `#[ignore]`d loopback capture), frontend `tsc && vite build` = clean. The full Mic/System/Call flow is wired end-to-end: source selector (Windows-gated) → `start_batch_recording(source)` → WASAPI loopback + cpal mic → `stop_call_recording` → `interleave_stereo` → `samples_to_wav_stereo` → `transcribe_deepgram_multichannel` → "אני:/הצד השני:" text + per-file `SpeakerLabelStyle` SRT export.
> **⚠️ wasapi 0.23 gotcha (fixed):** `get_default_device` is a `DeviceEnumerator` **method**, not a free fn — plan snippet was wrong, shipped code uses `DeviceEnumerator::new().and_then(|e| e.get_default_device(&Direction::Render))`.
> **⚠️ Final-review Critical (fixed `af30355`):** the Cancel button renders for every source but `cancel_batch_recording` only stopped the mic → a cancelled System/Call left the loopback thread running + bricked all future System/Call starts (re-entrancy guard) until restart. Cancel now drains the system recorder too.
> **✅ REAL-AUDIO VERIFIED 2026-07-13:** Henry ran CallCloud ("פגישה בענן") in a live Zoom call — speaker separation into "אני:/הצד השני:" worked. This proves the whole chain end-to-end (WASAPI loopback + dual-recorder capture + stereo interleave + Deepgram multichannel). The `#[ignore]`d `loopback_captures_playing_audio` unit test remains optional/never-run, but the behavioral gate it stood in for is now PASSED via the live call.

The original research that led here:
Today's mic capture uses `cpal` (cross-platform). To also capture the OTHER side of a Zoom/Meet call (what's playing through the speakers, not just the mic), Windows needs **WASAPI loopback capture** — a distinct API mode, not just "another cpal input device." `cpal`'s own WASAPI backend is not confirmed to expose loopback directly; the dedicated [`wasapi`](https://docs.rs/wasapi) crate does, with a documented loopback capture example (simultaneous capture + render on separate threads) — that's the concrete starting point, verify at implementation time whether newer `cpal` has closed this gap. Real design questions Henry needs to weigh in on before implementation starts, not just engineering: (a) mix mic + loopback into one stream, or keep them as two channels/two transcripts merged after the fact (affects diarization quality — mixing loses the "which side of the call" signal for free, that a separate-channels approach would preserve); (b) new permission/UX flow (Windows will very likely surface its own "app is capturing audio" indicator, similar to screen-recording prompts) — needs product decisions, not just code. Recommend brainstorming this properly (spec+plan, like SRT export got) rather than jumping straight to implementation, given the open design questions.

**Suggested order:** ~~diarization first~~ ✅ code-done (pending one live 2-speaker check) → ~~system-audio design pass~~ ✅ spec + reviewed 20-task plan done (2026-07-09) → ~~implement the plan~~ ✅ **DONE + pushed (`60b086c`, 2026-07-12)**.

---

## ✅ DONE 2026-07-12 — 4th recording mode (`CallLocal`) + source-selector UI regroup

**State:** brainstorm (all 4 design Qs closed with Henry) → spec (`docs/superpowers/specs/2026-07-12-recording-modes-ux.md`, reviewer-approved) → 6-task TDD plan (`docs/superpowers/plans/2026-07-12-recording-modes-ux.md`, reviewer-approved) → **fully implemented via subagent-driven TDD, committed + PUSHED to origin/main (`6949ed7`→`75863af`, 2026-07-12; Henry approved the push). NOT released to users (no GitHub Release / installer / auto-update).** `cargo test` = **55 passed / 1 ignored**, `cargo build` = **0 warnings**, clippy = clean for touched code (6 warnings are all pre-existing in untouched files), `tsc && vite build` = clean. Final two-stage review (spec-compliance + code-quality) = both ✅, no Critical/Important.

**What shipped (per the design below):** enum `Call`→`CallCloud` + new `CallLocal`; pure `mix_to_mono` (audio.rs, avg+silence-pad); `CallLocal` drains BOTH recorders → mixes to mono → existing mono file path → **forced-local whisper**; pre-record model guard (symmetric to CallCloud's Deepgram-key guard); meeting-specific silence message. Frontend: two labeled groups ("הקלטה רגילה" / "פגישות"), benefit-led meeting cards ("עם זיהוי דוברים" / "פרטית במכשיר"), context-dependent cloud/local selector (shown only for mic/system), standalone transparency note deleted.

**⏳ ONLY open item — MANUAL-VERIFY on real Windows audio (Henry, can't automate):** with a whisper model downloaded, `npm run tauri dev` → batch view → pick **פגישה — פרטית במכשיר**, speak while system audio plays, stop → expect one **local mono** transcript (no "אני/הצד השני"). Also: no-model → guard error fires *before* recording; cancel mid-CallLocal then start a meeting again works (system-recorder drain, the af30355 regression gate); the cloud/local cards appear only for mic/system. Then push when satisfied.

---

## 📐 Original design (approved by Henry 2026-07-12) — for reference

**Goal:** add a 4th mode and reorganize the batch source selector for MAXIMUM UX clarity (Henry's explicit #1 priority — "מאוד מאוד חשוב שחוויית המשתמש תהיה מאוד מאוד ברורה").

**The 4 modes, in 2 visual groups:**
- **קבוצה א׳ — הקלטה רגילה:** 🎙 מיקרופון · 🔊 אודיו מערכת (both mono; both keep the cloud/local choice)
- **קבוצה ב׳ — פגישות:**
  - 📞 **פגישה בענן** — mic+system SEPARATED (stereo → Deepgram multichannel), labeled "אני:"/"הצד השני:". = today's `Call`. Cloud-only.
  - 🔒 **פגישה מקומית** — mic+system MIXED into ONE mono transcript, transcribed LOCALLY (whisper), **NO speaker separation**. = the NEW mode. Privacy: audio never leaves the machine.

**Why the new local mode:** `Call` is cloud-only (multichannel is Deepgram-only), so a privacy-conscious user can't transcribe a meeting without uploading audio. Mode 4 fills that gap; trade-off = losing "who said what" (local whisper has no diarization).

**Open design questions for the (brief) brainstorm→spec:**
1. Exact button labels + group headers / visual separation.
2. **The cloud/local mode-card interaction (the crux):** today a separate "מצב תמלול" cloud/local selector (`batchMode`) exists. Once the meeting buttons ENCODE cloud vs local, that separate selector becomes redundant/confusing for meetings. Decide: hide cloud/local cards for meeting modes? apply them only to Mic/System? Henry's clarity bar lives here.
3. Mode-4 mechanics: MIX mic+system to mono (new `mix_to_mono(mic, system)` — average the two 16k-mono buffers, pad shorter side with silence like `interleave_stereo` does) → existing `write_wav_16k_mono` → frontend `transcribe_file` with LOCAL mode. Force local, or default-local-allow-cloud?
4. Backend `RecordingSource` naming: today `Mic`/`System`/`Call`; add a 4th (`Call`=cloud, add `CallLocal`/`Meeting`) or rename to `CallCloud`/`CallLocal` for clarity.

**Technical starting points (grounded in shipped code):**
- `recorders_for_source` (batch.rs): mode 4 drives BOTH recorders → `(true, true)`, like Call.
- Mode-4 stop path = closer to the System file-path than to Call's inline multichannel: stop both → `mix_to_mono` → `write_wav_16k_mono` → return path → `transcribe_file` (local). Possibly a mixing branch inside `stop_batch_recording_to_file`.
- Windows-only for both meeting modes (WASAPI). Reuse the shipped `SystemAudioRecorder`.
- The Call cloud-transparency note added in `dfb14d7` (App.tsx) becomes moot/relocated once cloud-vs-local is explicit in the buttons — revisit it.

**Process (same discipline as the shipped feature):** brainstorming (Henry DECLINED mockups → go straight to a tight spec) → `docs/superpowers/specs/2026-07-1X-recording-modes-ux.md` → spec-review loop → user review → `superpowers:writing-plans` → `superpowers:subagent-driven-development` TDD (red→green→atomic commit, controller verifies each, adversarial review at the end). Direct-to-main, no PR.

**Baseline to build on:** `main` == `origin/main` @ `60b086c`; `cargo test` 50 passed +1 ignored; frontend `tsc && vite build` clean; signed installer at `src-tauri/target/release/bundle/nsis/הכתבה בעברית_2.11.0_x64-setup.exe`.

---

### What shipped

1. **SRT subtitle export** from batch file transcription — per-item and combined (multi-file, cumulative time offset, no gap) export, both cloud (Deepgram `words[]` bucketing, ~10 words/4s per cue) and local (whisper.cpp native `max_len(42)`/`split_on_word` segmentation) routes. New `srt.rs` pure module (9 unit tests) + `export_srt` command (mirrors `export_history`'s save-dialog pattern) + frontend "🎬 SRT" buttons gated by a shared `isSrtEligible` predicate (hidden if the transcript was hand-edited — segments would no longer match). Built via spec+plan+6 reviewed implementation tasks — see `docs/superpowers/specs/2026-07-03-srt-export-design.md` / `docs/superpowers/plans/2026-07-03-srt-export.md` for full detail if extending this later (e.g. the "Out of scope" section lists what v1 deliberately skipped: history export, filename-matches-video, configurable chunking, VTT).

2. **Floating idle-button focus bug, fixed at the actual root cause.** Symptom: dictating via the floating button (mouse click, not Alt+D) sometimes injected text nowhere instead of the target app. First fix attempt (extend the `inject_text` Tauri command's window-hide/restore trick to the "toolbar" window too) had **zero effect** — because `streaming_enabled` defaults to `true`, and streaming's live per-segment injection lives in a completely separate call site (`streaming.rs::handle_message`) that bypassed the command entirely. Real fix: extracted the hide→wait→inject→restore logic into a shared `inject_text_defocused(app, text)` helper in `lib.rs`, used by **both** the command and the streaming path. Henry confirmed fixed in both modes.

3. **Per-item action-button row layout** — 5 buttons (inject/copy/TXT/Word/SRT) no longer fit one row; fixed with a deliberate 2-row grouping (quick actions / export formats). Henry explicitly rejected icon-only labels (confused users historically) and plain flex-wrap ("doesn't look intentional") — don't re-propose either without new information.

### Gotcha for next release

`npm run tauri build` needs **both** `TAURI_SIGNING_PRIVATE_KEY` **and** `TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""` exported — the key file is in `rsign`-encrypted format even with a blank password, and omitting the password var silently skips the updater `.sig` generation with **no error message** (the `.exe` still builds fine). Cost me one full rebuild cycle this round — set both from the start next time.

## Not done — flagged for future, NOT scheduled

- **English → Hebrew translation** (upload/dictate in English, auto-translate to Hebrew) — Henry-confirmed roadmap idea from 2026-07-02, not started.
- **Windows SmartScreen / code-signing certificate** — researched 2026-07-05: as of 2026, EV certs no longer bypass SmartScreen instantly (Microsoft closed that loophole in 2024) — OV and EV now both need the same reputation-building period. Azure Trusted/Artifact Signing ($9.99/mo, cheapest option) is **not available to Israel** (individual devs limited to US/Canada; orgs to US/Canada/EU/UK). Realistic path: a standard OV cert (~$200-400/yr, e.g. Sectigo) — replaces "Unknown Publisher" with a verified name and starts the reputation clock, but doesn't eliminate the warning immediately. Henry has not yet decided whether to purchase — ask before assuming this is wanted.

## 🔮 Backlog — Voicebox comparison (Henry, 2026-07-07): programmatic access

Henry compared hebrew-dictation to Voicebox and flagged three items to "add to the plans." Ordered by leverage — none scheduled yet:

**1. Local API — ✅ SHIPPED (2026-07-09), then materially reworked 2026-07-19 (see the top section).** `src-tauri/src/local_api.rs` — a `tiny_http` server on `127.0.0.1:5757`, **opt-in** (`local_api_enabled` in settings.json, off by default; port via `local_api_port`), bind failure non-fatal.

> ⚠️ **This bullet used to claim:** *"`GET /transcript` returns the last injected transcript… Verified wired: `inject_text_defocused` writes `last_transcript` after every successful injection (dictation and streaming)."* **That wiring was real but WRONG, and this sentence is what recorded the bug as verified-correct.** `streaming_enabled` defaults true and streaming injects **per segment**, so `/transcript` returned only the *last fragment* of a dictation. Fixed 2026-07-19: `GET /transcript` now returns `{text, seq, at}`, and stamping happens at the two **utterance boundaries** (the `inject_text` command; `stop_streaming_transcription`), never inside `inject_text_defocused`. Full detail in the top section.

Remaining polish: no UI toggle, no WebSocket live stream. (Unit tests: now 7 in `local_api.rs`.) **This unblocked #2 (the MCP wrapper) — now built.** Original research below —

**1-orig. Local API (REST/WebSocket on `127.0.0.1`) — highest leverage, the real gap.** The app is UI+hotkey only, no programmatic surface; Voicebox exposes `POST /generate` on `127.0.0.1:17493`. A small local server here (e.g. `GET /transcribe` → last transcript, or a WebSocket live stream) would let the **cloud-agent and the video pipeline consume dictation without driving the UI**. The transcription pipeline already exists (Deepgram/Groq/whisper-rs via `run_transcribe_file`) and the app already runs tokio → it's "wrap the existing pipeline in an embedded server." ⚠️ **Impl note:** `tauri-plugin-http` is an *outbound* client (a fetch replacement), **not** a server — to host an endpoint use embedded `axum`/`tiny_http` on a background task, not that plugin. **Design point first:** localhost still lets any local process — and browser pages via `fetch('http://127.0.0.1:…')` — reach it; gate anything that can trigger recording or return history behind a token (a read-only "last transcript" endpoint is low-risk). **Enabler for #2.**

**2. MCP server around the dictation — ✅ BUILT 2026-07-19 (see the top section).** Lives at `AI-Tools/MCP-Dev/hebrew-dictation-mcp/` — **its own git repo**, deliberately, so its commits never land in the shared `claude-dev` root repo the nightly job owns. Node/TS + `@modelcontextprotocol/sdk` ^1.29, stdio, three tools. The predicted shape was right: pull-based, no server→model push. NOT released/registered anywhere yet — see the top section for what's left.

*Original research:* Voicebox's MCP lets agents *speak* in the cloned voice; the inverse here gives Claude Code/Cursor voice *dictation into the agent session* (dictate-into-agent, not just into a text field). Best built as a thin wrapper over #1's local API rather than re-embedding the transcription logic — so **#1 first, then #2 is a small adapter.**

**4. The actual Voicebox-shaped feature (TTS) — ✅ BUILT 2026-08-15, see the top section.** Shipped as "צור קריינות": local, free, offline Hebrew text→speech. **The 2026-07-29 research in `HANDOFF-TTS-VOICE-GENERATION.md` is now historical — its conclusion was superseded.** That file recommended **VoxCPM2**, which was subsequently tested and **rejected on intonation** along with MiniMax, ElevenLabs IVC, RVC and Deepdub. What actually shipped is Phonikud stress-marked IPA driving a VITS ONNX voice, which is a different answer to a problem that turned out to be about *stress marking*, not about model size. Read the top section, not that file, before touching this.

**3. LLM text-cleanup — ⚠️ ALREADY EXISTS, do NOT rebuild.** Henry's read was "the app injects raw STT with no punctuation/disfluency cleanup," and he rightly YAGNI-flagged it. Code check: it's **already shipped as "Smart Cleanup / רישוף חכם" (`enhance.rs`, spec `docs/superpowers/specs/2026-06-15-smart-cleanup-design.md`)** — runs the transcript through Groq Llama-3.3-70b to strip fillers (אהה/אמ/יעני/כאילו), repetitions and false-starts and fix Hebrew punctuation, with a hallucination guard (>2× raw length → reject; plus a truncation guard added 2026-07-19 — `finish_reason=="length"` → reject, see the top section) and graceful fallback to raw text on any error. It's **opt-in** (`enhance_enabled` setting), wired via the `enhance_text` command. The cloud batch path also already sends `smart_format=true&punctuate=true`, so cloud transcripts are already punctuated. → **Not an engineering gap.** The only real questions: (a) product — should Smart Cleanup be more discoverable / default-on? (b) does it cover the **streaming**-inject path or only batch? (streaming bypasses the command path per the v2.11.0 focus-bug fix — verify). Nothing to build unless Henry wants it always-on.

## Key facts (unchanged, still accurate)

- Signing: `~/.tauri/hebrew-dictation.key` — **encrypted format, needs `TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""` explicitly** (see gotcha above; supersedes any earlier "no password" note).
- Repo: `aihenryai/hebrew-dictation`. Website: `aihenryai/Henry-AI-website` (Cloudflare auto-deploy on push to main).
- Dev run for testing: `npm run tauri dev` (repo root) — Rust changes need the process restarted, not just HMR (HMR only covers the frontend).
- Kill orphaned dev processes: PowerShell `Get-CimInstance Win32_Process | ? CommandLine -match 'hebrew-dictation' | % { Stop-Process -Id $_.ProcessId -Force }`.
