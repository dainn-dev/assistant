# UI/UX Enhancement Plan — MyJavis

**Date:** 2026-09-30 · **Scope:** `src/index.html` (885 lines), `src/styles/*.css` (~2,930 lines), `src/js/*` interaction layer · **Status:** U1 + U2 done (2026-09-30); U3–U4 pending
**Companion to:** `2026-09-30-codebase-enhancement-plan.md` (Phases 1–5 done)

## Product-shape ambiguity (decide first)

The codebase can't decide what it is:

- `tauri.conf.json`: `width 600 × height 400` but **`minWidth 1024 × minHeight 768`** — the declared "compact subtitle strip" can never exist; the floor is a big desktop window.
- `transparent: false` — the glassmorphism `backdrop-filter: blur(30px)` everywhere has nothing behind the window to composite. Dead visual intent.
- `decorations: false` + `resizable: true` — undecorated windows have no native resize grips, and the custom `#resize-handle` is `ns-resize` only → **users cannot resize width at all.**
- `alwaysOnTop: true` forced at startup (matches pin default, fine for overlay, odd for a 1024×768 window).

**Decision:** ~~(A) true floating subtitle overlay — small default, transparent, height-resize only, or (B) proper desktop app window~~ → **Decided: (A)** — compact transparent overlay. Implemented in U1: `710×320` default, `360×140` min, `transparent: true`, `alwaysOnTop` kept.

## Findings

### P0 — broken or silently losing data

| # | Finding | Evidence |
|---|---------|----------|
| 1 | **Window sizing contradicts design** — see above. 1024×768 minimum defeats the overlay concept; width is un-resizable. | `tauri.conf.json`, `#resize-handle` (`ns-resize`) |
| 2 | **`check_permissions` is never called** — the Phase-2 probes (SCKit on macOS, mic device on Windows) are dead code. Permission denial surfaces only as a cryptic toast after capture fails. | zero call sites in `src/js` |
| 3 | **`+ New` discards transcript silently** — `_createNewSession()` clears the session with no autosave and no confirm. Meanwhile *deleting* a saved conversation *does* `confirm()`. Destructive asymmetry. | `conversations.js:166` vs `app.js:178`, `session.js:606` |
| 4 | **Settings Back / Escape discards edits with no warning** — typed API keys, context rows, voice choices vanish. No dirty tracking; two Save buttons but Back isn't guarded. | `app.js:193`, `shortcuts.js:41` |
| 5 | **Keyboard hints are wrong** — tooltips advertise `⌘1/⌘2/⌘3`, `⌘T`, and Start says `(Space)`; the real bindings are Ctrl/Cmd+Enter (Space is unbound), and `⌘` glyphs show on Windows. Placeholder shows `⌘ Enter`. | `index.html:44-68`, `ui.js:181`, `shortcuts.js` |
| 6 | **Font-size/color quick controls unreachable in normal mode** — they live inside `#chat-panel`, displayed only when `app_mode === 'Interview'`. Default and Meeting modes get no font control despite settings existing. | `settings-form.js:268-281`, `index.html:163-230` |

### P1 — flow & IA problems

| # | Finding |
|---|---------|
| 7 | **Interview/Meeting mode is undiscoverable** — the `#btn-template` dropdown was removed from HTML; JS still queries it (`interview-panel.js:112`). Only path to enable modes is Settings → **Display** → Mode — the wrong tab. Dead `.chat-template` CSS (~90 lines) + dead `.interview-key-*` CSS (~60 lines) + dead `web-speech-tts.js`. |
| 8 | **Read-only replay has no banner** — opening a saved conversation looks identical to live state; Start silently dims to 0.35 opacity. No "Back to live" affordance except `+ New` (which also nukes content — see #3). |
| 9 | **Source buttons don't persist** — `_setSource` sets `this.currentSource` only; `settings.audio_source` is untouched, so the Settings radio goes stale and the choice reverts on restart. Two controls, two truths. |
| 10 | **Settings IA** — Mode + Suggestion type sit under the **Display** tab; `" AI"` tab label has a literal leading space; 4 hand-maintained `<select>` language lists (~50 options each) where two-way A/B only offer the "Popular" subset — can't pick e.g. Thai↔German. |
| 11 | **Errors evaporate** — 5s toast is the only error surface; `status-dot` goes red but `status-text` keeps saying "Ready". No persistent last-error, no retry affordance. |
| 12 | **`font_family` has no UI control** — applied at runtime (`--app-font-family`), persisted, but nothing sets it. Dead capability. |

### P2 — polish

| # | Finding |
|---|---------|
| 13 | `overlay_opacity` fades the **whole window** (`overlayView.style.opacity`) — text and controls included — rather than just the background. With `transparent:false` it's a dimmer, not translucency. Either fix alpha compositing or rename "Brightness". |
| 14 | Global `user-select: none` — transcript text can't be partially selected; only the Copy button works (all-or-nothing). |
| 15 | Mobile: pin/minimize/close buttons meaningless on Android but still rendered; 48px touch targets exceed the 42px control-bar height → cramped/overflowing bar. |
| 16 | No empty state in the conversation list (blank sidebar). No language search in 50-option selects. Toast covers transcript center-bottom. |

## Implementation plan

### Phase U1 — Correctness hotfixes (~½ day, no layout changes) — ✅ Done (2026-09-30)
1. **Window config** ✅ — option (A): `560×320`, min `360×140`, `transparent: true` (`tauri.conf.json`).
2. **`+ New` guard** ✅ — `transcriptUI.hasSessionContent()` → `confirm("Discard the current unsaved transcript?")` (`app.js`).
3. **Dirty-form guard** ✅ — `_snapshotSettingsForm()` serializes all controls (id-less dynamic rows keyed by position); Back/Escape → `_closeSettings()` confirms; snapshot reset on successful save (`settings-form.js`, `shortcuts.js`).
4. **Key hints** ✅ — `_applyShortcutHints()` rewrites `⌘`→`Ctrl+` in tooltips off-macOS and sets the Start title to `Ctrl+Enter`/`⌘+Enter`; placeholder glyph is platform-aware (`ui.js`); dropped the unbound `(Space)`.
5. **Persist `_setSource`** ✅ — `settingsManager.save({audio_source})`; toast now says "restarting capture" when switching mid-run (`session.js`). Also removed `ttsEnabled=false` from `_applySettings` — it was clobbering the live TTS toggle on every settings save.
6. **Permission wiring** ✅ — `check_permissions` probed once on first `start()`; denial → actionable toast + error status; live status line under Audio Source in Settings → Translation (`perm-status-hint`).
7. **Bonus** ✅ — fixed `" AI"` tab label typo (was U2.9).

### Phase U2 — Layout fixes — ✅ Done (2026-09-30)
7. **Font/color controls freed** ✅ — moved out of `#chat-panel` into `#display-controls` row under the transcript, toggled by a new **Aa** button (`btn-display-controls`) in the control bar — reachable in every mode and in read-only view.
8. **Read-only banner** ✅ — `#readonly-banner` ("Viewing saved conversation · Back to live") shown inside transcript area on `_openConversationReadOnly`, hidden in `_createNewSession`; "Back to live" runs the shared `_startNewSessionFlow` (stop-if-recording → save-first → reset).
9. **Settings IA** ✅ — Session Mode + Suggestion type moved to the AI tab (label already fixed in U1); Display gains a real `select-font-family` so `font_family` is settable.
10. **Shared language list** ✅ — `LANG_NAMES`/`POPULAR_LANGS` consts in `settings-form.js`; `_initLanguageSelects()` builds all four selects (source adds `auto`; A/B get the full list). ~200 lines of duplicated `<option>` markup deleted from index.html.
11. **Opacity fix** ✅ — `--overlay-alpha` CSS var drives `--bg-primary`'s alpha channel; `_applySettings` sets the var instead of `element.style.opacity`, so text/controls stay opaque while the card background goes translucent.
12. **Quick-control persistence** ✅ — font dots save `font_color` (new Settings field, `#ffffff` default), `_adjustFontSize` saves `font_size`; `_applySettings` re-syncs the Aa toolbar UI from settings.

### Phase U3 — Flow polish
12. **Mode switcher in overlay**: restore a compact mode pill/dropdown (None / Interview / Meeting) in the control bar or chat row so modes are discoverable without opening Settings. Remove dead `#btn-template`/`chat-template` code paths and CSS, dead `.interview-key-*` CSS, and `web-speech-tts.js`.
13. **Persistent error surface**: `status-text` shows last error summary (truncated); click → full error in toast/modal. Keep transient toasts for transient events.
14. **Empty states**: "No saved conversations" placeholder in sidebar; "Add an LLM key to enable suggestions" hint row inside the suggestions panel when unconfigured.
15. **Mobile hygiene**: hide pin/minimize/close on `body.mobile`; raise `--control-bar-height` to ~52px under mobile.

### Phase U4 — Delight (optional, later)
- Language selects with type-ahead (`<input list>` + datalist, or tiny custom combobox).
- Toast stack (max 2, queued) with icon per type; move to top-center under the control bar.
- `prefers-reduced-motion` media query to disable pulse/wave/shine animations.
- High-contrast transcript theme preset + true opacity slider once `transparent:true` lands.

## Verification
- Manual matrix after each phase: resize to min/max, pin, settings dirty-cancel, `+New` with content, permissions denial path (macOS needs a device test).
- New vitest cases: `_setSource` persistence, settings dirty-snapshot diff, parser for hint glyph selection.
- Keep `cargo test` + `vitest` + `clippy` green (CI gates now exist).

## Risks
- **Window-shape change (U1.1) is the riskiest**: transparent windows behave differently across Windows/macOS (blur regions, shadows) — test on both before shipping; keep `(B)` fallback if transparency misbehaves.
- Dirty-form guard must not double-prompt after a successful Save — reset the snapshot post-save.
- Language-list generation must preserve the exact option values already persisted in users' `settings.json`.

## Interview early suggestions — design for review

**Status:** Product direction and two independent Soniox streams approved in conversation. The specification below awaits review; no implementation of this feature has started. It extends Interview mode, not the remaining U3/U4 work. Detailed implementation planning follows design approval.

### Intent and scope

Show useful, explicitly provisional answer hints while the interviewer is still speaking. Do not wait for a complete question, final translation, or a manual click. Update the hints as the question becomes clearer without repeatedly replacing text the candidate is reading.

The selected use case is an online interview with System + Mic capture and headphones. System audio is treated as the interviewer; microphone audio as the candidate. These are source roles, not verified human identities. System audio may include other applications and microphone echo can still occur. In-person shared-microphone role detection, per-application loopback capture, acoustic echo cancellation, and predictive Local MLX support are outside the first version.

First delivery targets the current Windows desktop/Soniox workflow. Keep Meeting, ordinary translation, and Local MLX behavior compatible. Do not advertise two-source early suggestions on other platforms until their capture path has been tested.

### Decisions and alternatives

- Selected: separate capture channels and independent Soniox clients, merging only transcript/context records. This retains source identity even when people overlap.
- Rejected for this version: infer interviewer/candidate solely from speaker IDs on a combined stream. Speaker IDs do not identify source or role and may change after reconnect.
- Retain manual suggestions as an explicit fallback; do not silently enable expensive automatic calls for existing installations.
- Two recognition connections and speculative LLM requests can increase cost. Explain this when enabling the feature; cancellation does not guarantee the provider stops billing immediately.

### 1. Audio routing and lifecycle

Add a dedicated split-capture command for the Interview early-suggestion path. It accepts separate System/Mic IPC channels, keeping the existing capture command usable by other modes. Each selected source forwards its own 16 kHz mono PCM; do not concatenate samples from two independent clocks into one audio stream.

A session coordinator owns the selected source captures, Soniox clients, forwarding workers, and an epoch. Start is single-flight. A partial startup failure stops everything started by that attempt and surfaces the failed source; do not label a partially initialized pair as fully recording. Stop waits for forwarding workers to terminate, drains available final recognition results with a bounded timeout, and retains remaining provisional text per source without duplicating promoted text. Stop still does not create a transcript file.

Each source connection owns its reconnect/rotation timers and a connection generation. Callbacks from closed or superseded generations are ignored. Reconnect clears that source's uncertain provisional tail and marks a context gap; it must not replay pending audio or transcript as a new question. Source changes, relevant recognition-setting changes, and mode changes are serialized with capture teardown/restart. No new start can race a pending stop.

### 2. Transcript and question assembly

Normalize recognition events with session epoch, source, connection generation, local sequence, token timing where available, original/translation channel, provisional/final state, language, and endpoint marker. Preserve source-local token order. Final recognition tokens are committed deltas; the provisional tail is a replaceable snapshot, not additional text to append repeatedly.

Maintain committed text, provisional text, and translation alignment separately for System and Mic. Merge display records by session-relative timing with a deterministic sequence tie-breaker. Match translation only within its source/connection and compatible utterance boundary; do not use the current global oldest-untranslated lookup. Do not assume one original callback equals one translated callback. Preserve untranslated content if reliable alignment is not yet available.

Add optional source and stable segment identifiers to saved structured segments. Old sidecars and Markdown-only transcripts still load. The one-session/one-file rule and save-failure preservation remain unchanged. Predicted questions and generated hints never become spoken transcript entries automatically.

The System question buffer combines the current turn's committed original text with its stable provisional prefix. Consume explicit endpoint signals separately from translation arrivals: final tokens are not necessarily the end of a question. Start a new turn after an endpoint plus subsequent System speech, or after candidate speech followed by new System speech. Carry a bounded preceding dialogue excerpt so multi-part questions do not lose context.

### 3. Early-suggestion scheduler

Use a periodic evaluation tick (initial value: 600 ms), not a debounce that restarts on every token. Continuous speech must still trigger a request before its endpoint. An eligible snapshot has meaningful content in a prefix stable across at least two observations. Initial lexical gate: at least four word-like units, with an alternative of at least twelve CJK characters; ignore punctuation-only changes. This is a noise filter, not proof that the question is understood. The LLM may return insufficient-context instead of guessing.

Initial request policy, to be tuned with replay tests:

- One active answer-generation request and one replaceable pending snapshot; never an unbounded queue.
- At least 1.5 seconds between generation starts, at most three speculative starts per turn, and at most twenty generation starts per rolling minute. Secondary-language generation counts against the same limit.
- Only schedule a revision if the stable content materially grows or changes. Identical normalized snapshots, provisional-to-final promotion of identical text, and translation callbacks must not create duplicate requests.
- Ordinary append-only growth updates the pending snapshot and allows a useful in-flight answer to finish. Corrections to already-used content or a new turn invalidate and cancel obsolete work. This prevents constant cancellation from starving the first answer.
- On endpoint, allow one final refinement when the complete question differs from the last answered snapshot, subject to the same concurrency and minute limit. Keep the current answer visible while waiting. If capped, show that automatic updates are paused rather than silently continuing to call APIs.

The numeric values are starting policies, not measured latency promises. This scheduler runs only for live Interview sessions with Early suggestions enabled and an eligible System source; history playback must never trigger requests.

### 4. Candidate speech and readable UI

Keep existing answer cards and add a compact Early suggestions toggle plus Hold/Resume updates control inside the Interview panel, not the already crowded main toolbar. The setting defaults off for existing users; enabling explains source roles, cost, and transmission of provisional speech to the configured LLM provider.

State labels are Listening, Early hint, Updating, Answer updated, Held while you speak, Held by you, and Paused/error. Early hint means the question may change; a finalized transcript does not make the generated answer factually verified.

Show short talking points first. A revision starts in a separate draft area and replaces the old card only when a usable new clause is available. Render untrusted model text as text, never executable HTML. Preserve text selection and scroll position. Hold pins the visible version and cancels unnecessary generation; Resume uses the newest valid context, not a backlog.

Stable non-empty Mic recognition marks candidate speech and automatically holds an existing visible answer. A brief silence alone does not unfreeze it; new System turn activity or Resume releases the automatic hold. A manual hold persists until explicitly resumed. If no hint is visible when candidate speech begins, allow the current still-valid request to supply a first hint, then hold it; Mic content never starts a new answer request. Acoustic-energy-only triggers are excluded initially to avoid holding on keyboard noise. Recognition-based hold has inherent latency and is not guaranteed to detect speech onset immediately.

Collapsing the panel does not change the explicit Early suggestions toggle. Turning the toggle off cancels automatic work but leaves the last card and manual generation available. Stop cancels work while preserving the card; New clears the question context/cards. Switching to history or leaving Interview invalidates requests and prevents late updates.

### 5. Real streaming and cancellation

Introduce an asynchronous, cancellable Interview generation path while retaining the existing manual/Meeting command for compatibility. Reuse an HTTP client for connection pooling. The existing reqwest/tokio dependencies are suitable; enable only the features needed for asynchronous streaming/cancellation during implementation.

Use an invocation-scoped Tauri channel with typed start, text-delta, done, insufficient-context, cancelled, and error events. Every event includes session epoch, turn ID, revision, and request ID. The frontend accepts only the current permitted request; stale errors must not overwrite the status of newer work. The backend keeps window/session-scoped cancellation state, aborts HTTP work on cancel or window teardown, and cleans up entries after any terminal event. Stale cancellation must never abort a newer request.

Decode the provider's OpenAI-compatible SSE response incrementally, including arbitrary byte boundaries, split UTF-8, multiple events per chunk, terminal markers, and provider errors. Buffer limits and deadlines prevent a malformed or stalled stream from retaining work indefinitely. Do not parse incomplete final-answer JSON on the frontend or simulate streaming with typewriter timers.

Stream the primary answer as plain text in the configured suggestion language. For the existing both-language option, show the primary answer immediately and translate the completed, still-current answer in a subsequent bounded request. Secondary text is version-bound and cannot attach to a newer answer. Never block the first visible answer waiting for the second language. Manual/Meeting structured responses remain supported.

An endpoint that does not support streaming uses an explicit buffered fallback after detecting that incompatibility, not automatic duplicate requests for every network error. It can still use early triggering, but the UI must not claim token streaming. Authentication failures pause automatic generation until configuration changes or explicit retry; rate limits honor provider retry hints and pause instead of retrying every scheduler tick.

### 6. Fast context and grounding

Prepare bounded CV/JD context before the first question using extracted text already stored in SQLite. Keep CV evidence separate from JD requirements: a JD requirement is not evidence of candidate experience. Prefer source excerpts over generated biography claims; do not invent projects, metrics, or technologies missing from the evidence.

Build an in-memory, revision-keyed set of document chunks when Early suggestions is enabled. Short documents can fit directly within the prompt budget; longer documents use bounded local lexical selection for the immediate hint. The first hint must not wait for query embedding and Pinecone. Existing semantic retrieval can enrich a later refinement, once per materially changed question, with its own timeout and cache. Failure of enrichment must not erase a usable grounded hint.

Cache entries are scoped to the current user/profile, active document IDs/content revisions, question snapshot, languages, model, and relevant configuration. Upload, replace, remove, user/profile change, or New invalidates the appropriate caches and in-flight revisions. Removed documents must stop appearing in prompts, including stale Pinecone hits. Add active-document filtering rather than trusting previously indexed chunks. Do not retain raw audio in these caches.

Pass a bounded current-session conversation excerpt with explicit interviewer/candidate roles; do not rely on the current cross-session last-six-message query alone. Candidate provisional speech remains replaceable; only committed speech is eligible for persisted dialogue history. Supply transcript and document content as untrusted data, not instructions. Early output should acknowledge insufficient context instead of confidently predicting the unspoken end of the question.

### 7. Safety of operation and compatibility

- Early suggestions requires Soniox plus System or Both source. Mic-only and Local MLX keep manual suggestions and show why automatic early hints are unavailable. Changing into an unsupported combination cancels automatic work without losing transcript content.
- Disable runtime TTS narration while Early suggestions is active, stop queued audio, and explain why: system loopback would otherwise transcribe the app's own output. Do not silently rewrite the persisted provider/voice preference or automatically start playback when leaving this mode.
- Recommend headphones and muting unrelated media; source routing is not echo cancellation or per-app audio isolation. A failed System connection pauses automatic hints with a visible source-specific warning. If Mic disconnects after successful startup, System can continue with a warning that automatic hold is unavailable; do not silently relabel System speech as candidate speech.
- Update frontend defaults, Rust defaults/strict-save validation, form population/save, and dirty-form tracking together for the persisted Early suggestions boolean. Hold is per-session runtime state, not a persisted preference.
- Log only timings, counts, request identifiers, source health, and sanitized error categories. Do not log API keys, raw prompts, CV excerpts, transcripts, or raw provider bodies that could contain private content.

### 8. Verification and acceptance

Use deterministic recognition replays and fake timers before live API testing. Required cases:

1. A long continuous System utterance with stable partial prefixes starts generation and can display a streamed hint before the endpoint, without a click or translation result.
2. Mic-only speech never starts automatic generation; overlapping sources retain independent provisional tails and correct translation ownership.
3. Repeated provisional snapshots, final promotion, and late translation do not duplicate text or requests. A negation/correction to an already-used prefix invalidates its previous answer revision.
4. Sustained token updates do not starve the scheduler. Single-flight, pending replacement, per-turn caps, minute caps, and primary/secondary-language ordering are enforced.
5. Out-of-order events and errors from cancelled requests cannot mutate the new answer. Stop, New, history, reconnect, settings changes, and mode switches invalidate the correct epochs and release timers/tasks.
6. Hold/Resume, candidate auto-hold, missing first answer, unavailable Mic, and continuing System questions have explicit UI outcomes. Keyboard users can operate all added controls.
7. SSE parser tests cover split bytes/UTF-8/events, stream errors, incomplete EOF, deadlines, and non-streaming fallback. Cancelling aborts backend HTTP work, not just UI animations.
8. CV/JD changes and removal invalidate context and filter stale retrieval results; JD requirements never become claimed experience. No raw content or secrets appear in logs.
9. Old settings/transcripts still load; Stop remains pause-only; Save & New produces one session file with both sources; save failure preserves content. Meeting and normal translation regressions are covered.
10. Run frontend syntax checks and Vitest, then Rust tests, Clippy, formatting checks, and a desktop build. Live Windows smoke test uses two distinct source phrases, overlapping speech, a mid-question correction, and source/network failure. macOS/Android support is not inferred from Windows success.

Measure speech-to-first-partial, eligible-prefix-to-request, request-to-first-useful-text, endpoint-to-first-useful-text, stale cancellation count, revision count, and request/token usage. Record early-lead time when a useful hint arrives before the endpoint. Report measured median/tail latency and correctness observations rather than promising a fixed response time. Live tests that consume external APIs require the user's configured account and should use a bounded test session.

### Delivery order after design approval

1. Source-safe audio/transcript foundation with replay and routing tests; no automatic LLM calls yet.
2. Cancellable real streaming and versioned card rendering, initially exercised through manual requests.
3. Early scheduler, local context preparation, grounding, Hold/Resume, and visible failure/rate-limit states.
4. End-to-end regressions and bounded live measurements; then rebuild the portable executable.

This is a design/delivery sequence, not approval to implement or publish. Review this section before producing the detailed task-level implementation plan.
