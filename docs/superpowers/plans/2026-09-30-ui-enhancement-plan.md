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
