# Plan: Early-suggestion metrics + technical-debt sweep

**Date:** 2026-10-01 · **Branch:** `feat/metrics-and-debt` (off `main` @ `5bc47ff`)
**Status:** Awaiting approval. No code changes yet.

## Why now

Early suggestions shipped with guessed thresholds (600 ms tick, 1.5 s spacing,
3 hints/turn, 20/min) and no way to know whether they help. At the same time
`main` CI is red on macOS, the manual Interview path still does the slow
embed → Pinecone → LLM chain the early path already bypasses, and the UI
swallows errors after one 3 s toast. Fix the measurement gap first so every
later tuning decision has a number behind it, then clear the debt that makes
the app untrustworthy day-to-day.

Two phases, independently shippable. Phase A is small and should land first
so Phase B's manual-path change can be measured.

---

## Phase A — Measurement (≈ 1 day)

### A1. Local metrics ledger

**Goal:** record every early-hint lifecycle event in-process, persist per
session, zero network.

Files: `src/js/early-suggestions.js`, `src/js/session.js`,
`src-tauri/src/commands/transcript.rs`

- New pure module `src/js/metrics.js` exporting `SessionMetrics`:
  - `mark(event, payload)` with events:
    `question_start` (first System provisional in a turn),
    `hint_fired` (requestId, snapshotLen, revision),
    `hint_first_token` (requestId),
    `hint_done` (requestId, chars),
    `hint_cancelled` (requestId, reason: `correction|epoch|hold|superseded`),
    `endpoint` (turn end), `mic_speech_start` (candidate began answering).
  - `summary()` → per-turn derived numbers:
    - **TTFH** – question_start → first `hint_first_token`
    - **TTFH-after-endpoint** – endpoint → first token (negative = hint
      arrived before the question ended; this is the whole point)
    - hints fired / cancelled / useful (useful = done and not superseded
      before `mic_speech_start`)
    - requests per minute, peak and mean
- `EarlyScheduler` gets an optional `onEvent` callback; no behaviour change.
- `_finalizeSession()` writes `summary()` into the existing transcript
  sidecar JSON under `"metrics"` (schema is already versioned and lenient
  on load, so old files are unaffected).

**Tests** (`tests/metrics.test.js`, vitest, fake timers):
- TTFH computed from first token, not from `hint_done`.
- A hint fired before `endpoint` yields negative TTFH-after-endpoint.
- Cancelled-by-correction hints are excluded from "useful".
- `summary()` on an empty ledger returns zeros, never NaN.

### A2. Debug readout in Settings → AI

Files: `src/index.html`, `src/js/settings-form.js`, `src/styles/main.css`

- Collapsed `<details>` "Early-suggestion stats (last session)" under the
  `early_suggestions` checkbox. Shows the five summary numbers + a
  "Copy as JSON" button. Hidden when there is no last-session data.
- No new setting; the panel reads `this._lastSessionMetrics` set by
  `_finalizeSession()`.

### A3. Cost counter on the session chip

Files: `src/js/session.js`, `src/js/ui.js`

- Track Soniox audio seconds per stream (sum of PCM durations sent) and LLM
  request count. Chip tooltip gains `Soniox 2×03:24 · LLM 7 calls`.
- Persist alongside metrics in the sidecar. No pricing math in-app
  (rates change; the user multiplies).

**Acceptance for Phase A:** after one real Interview session, Settings → AI
shows non-zero TTFH and a hint count that matches what was seen on screen;
`metrics` block present in the saved sidecar; vitest green.

---

## Phase B — Technical debt (≈ 2–3 days)

### B1. Fix macOS CI (blocking, 15 min)

Observed: run `36709122048` fails only on `rust (macos-latest)` at
`cargo clippy -D warnings`:

```
error: method `is_capturing` is never used
  --> src/audio/system_audio.rs:175:12
```

The SCKit `SystemAudioCapture` exposes `is_capturing()` but nothing calls it
on that target (Windows uses `wasapi.rs`, which is called). Remove the method
from `system_audio.rs` only — the `cpal_mic`/`microphone`/`wasapi` ones are
used. Re-run CI; expect green on all three jobs. If a second macOS-only
warning appears underneath, fix it in the same commit rather than adding
`#[allow]`.

### B2. Manual Interview path reuses the fast context path

**Problem:** `suggest_interview_answers` (manual ↻ / brainstorm button) runs
embed → Pinecone query → LLM serially, and requires a Pinecone key even
though the early path grounds on local `select_context_excerpts` with no
Pinecone at all. Two code paths, two prompts, two failure modes.

Files: `src-tauri/src/commands/interview.rs`, `src/js/interview-panel.js`

- Frontend `_runInterviewSuggestions` calls `select_context_excerpts` first
  (same as `_earlyFire`) and passes `contextSnippets`.
- Backend: when `context_snippets` is `Some`, **skip** embedding + Pinecone
  entirely and ground the full-answer prompt on the snippets. Pinecone
  remains the fallback only when snippets are empty *and* a key is present.
- Pinecone key check moves from "always required" to "required only when
  the fallback is actually taken". Error text updated accordingly.
- Keep the structured JSON-array output for the manual path (UI renders
  per-item chips); only the retrieval changes.

**Tests:**
- Rust: `suggest_interview_answers` with `context_snippets: Some(vec![...])`
  and **no** Pinecone key must not return the "Pinecone API key not set"
  error. (Mock LLM via the existing `http_client()` seam or feature-gate
  the network call; do not hit a real endpoint.)
- Rust: empty snippets + no Pinecone key → still returns the clear error.
- Vitest: `_runInterviewSuggestions` passes `contextSnippets` built from
  `select_context_excerpts` output (`[cv] …` / `[jd] …` prefixes).

**Measure (needs Phase A):** compare manual-path TTFH before/after on the
same recorded question. Expect the Pinecone round-trip (typically hundreds
of ms) to disappear.

### B3. Persistent error surface (UI plan U3-13)

**Problem:** every error is `_showToast(msg, 'error')` for ~3 s then gone.
A dropped Soniox socket, a 401 from the LLM, or a mic that failed to open
all leave the user staring at a silent transcript.

Files: `src/index.html`, `src/styles/main.css`, `src/js/window.js`,
`src/js/session.js`, `src/js/early-suggestions.js`

- New `this._lastError = { message, at, source }` set by every `error`-type
  toast (one line in `_showToast`).
- Status area shows a small `⚠` pill next to the session chip while
  `_lastError` is set and younger than the current session. Click → toast
  with the full message (long-lived, dismissible). Cleared on `start()`,
  `+ New`, or click-to-dismiss.
- Soniox mic-stream errors (currently `console.warn` only, by design so
  they don't kill the System stream) **also** set the pill with
  `source: 'mic'`. Nothing changes about not stopping the System stream.
- Reconnect attempts set a transient `⟳` instead of `⚠` and clear on
  success.

**Tests (vitest, DOM stubs as in `early-suggestions.test.js`):**
- Error toast sets `_lastError`; success toast does not.
- Pill hidden after `_startNewSessionFlow()`.
- Mic warning path sets the pill without changing `isRunning`.

### B4. Empty states + mobile hygiene (U3-14, U3-15)

Small, CSS/HTML-heavy, bundle into one commit.

- Sidebar: "No saved conversations yet" row when the list is empty.
- Suggestions panel: when Interview mode is on but `llm_url`/`llm_model`/LLM
  key is missing, replace the empty text with "Add an LLM endpoint in
  Settings → AI to enable suggestions" + a button that opens that tab.
- `body.mobile`: hide `#btn-pin`, `#btn-minimize`, `#btn-close`,
  `#btn-hide-controls`; `--control-bar-height: 52px`.

No new tests beyond a vitest check that the LLM-unconfigured hint is chosen
when `llm_url` is empty.

### B5. Test seam for session lifecycle

**Problem:** `session.js` is the most consequential module (Stop pauses,
`+ New`/close finalize and save, failed save keeps the session alive) and
has **zero** automated coverage because it calls `window.__TAURI__.core.invoke`
directly.

Files: `src/js/session.js`, `src/js/conversations.js`, new
`src/js/ipc.js`, `tests/session-lifecycle.test.js`

- `src/js/ipc.js` exports `{ invoke, Channel }` resolved lazily from
  `window.__TAURI__` at call time (so a test can set
  `window.__TAURI__ = { core: { invoke: vi.fn(), Channel } }` before
  importing). Every module's `const { invoke } = window.__TAURI__.core;`
  becomes `import { invoke } from './ipc.js';`. Mechanical, no logic change.
- Tests (fake `invoke` returning scripted results):
  - `stop()` does not call `save_transcript`.
  - `_startNewSessionFlow()` with content calls save, then resets
    `sessionLog`; chip shows `✓ Saved`.
  - Save rejecting → `sessionLog` intact, chip stays `⏸ Draft`,
    error toast fired (ties into B3).
  - `start()` in `readOnlyMode` is refused without calling capture.
  - Early-suggestions + source `both` → `start_split_capture`, not
    `start_capture`.

### B6. Dead-code removal flagged in U3-12

- Delete `#btn-template` / `chat-template` DOM + JS paths, `.interview-key-*`
  CSS, and `src/js/web-speech-tts.js` if still unreferenced (verify with a
  grep before each deletion).
- `.interview-suggestions-panel.collapsed` CSS is dead (JS only toggles
  `right-panel-collapsed`) — remove.
- Mode switcher pill in the overlay bar is **deferred**: with Meeting gone
  there are only two states and the Settings → AI select is one click away.
  Revisit if a third mode appears.

**Acceptance for Phase B:** CI green on all three jobs; manual ↻ works with
Pinecone key blank; an induced LLM 401 leaves a visible `⚠` until dismissed;
`tests/session-lifecycle.test.js` ≥ 5 cases green; `cargo clippy -D warnings`
clean locally on Windows.

---

## Execution order and commits

| # | Commit | Depends on |
|---|--------|-----------|
| 1 | `fix(ci): drop unused is_capturing on SCKit capture` (B1) | — |
| 2 | `feat(metrics): session metrics ledger + sidecar persistence` (A1) | — |
| 3 | `feat(ui): early-suggestion stats readout + cost tooltip` (A2, A3) | 2 |
| 4 | `refactor(ipc): single invoke seam for testability` (B5 seam only) | — |
| 5 | `test(session): lifecycle coverage` (B5 tests) | 4 |
| 6 | `feat(ui): persistent error pill` (B3) | 4 |
| 7 | `perf(interview): manual path grounds on local excerpts` (B2) | 2 (to measure) |
| 8 | `feat(ui): empty states + mobile hygiene` (B4) | — |
| 9 | `chore: remove dead template/TTS code` (B6) | — |

Each commit: failing test first where a test exists, `vitest` + `cargo test`
+ `clippy` + `fmt` green, no push until the whole branch is reviewed.

## Out of scope (explicitly)

- Tuning the scheduler constants — that is what Phase A's numbers are *for*;
  change them in a follow-up with before/after data.
- Profile/STAR stories, post-session review, speaker-role assignment
  (roadmap item 3).
- Updater signing key and release workflow (roadmap item 5).
- Re-adding any form of Meeting mode.

## Risks

- **B5 touches every JS module's import line.** Mechanical but wide;
  do it in one commit with `node --check` on all files and a full vitest run
  so a typo can't hide.
- **B2 changes what the manual button answers from.** Lexical excerpts can
  miss a relevant CV paragraph that embeddings would have found. Mitigation:
  keep Pinecone as fallback when snippets come back empty; log which path
  was taken in the Phase A metrics so the trade-off is visible.
- **Metrics persistence grows the sidecar.** Summary only (a few hundred
  bytes), never the raw event list.
