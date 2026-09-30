# Codebase Review & Enhancement Plan — MyJavis

Date: 2026-09-30
Scope: `src/` (JS frontend, ~6.7k lines), `src-tauri/src/` (Rust backend, ~3.4k lines)
Status: pending

## Executive Summary

The app works (verified: builds and runs on Windows) and has genuinely good bones —
clean capture abstractions per platform, a well-thought-out Soniox client with
make-before-break session resets, and decent IPC hygiene on transcripts. The main
problems are **a settings-persistence bug that silently discards fields**, **API keys
logged in plaintext and stored inconsistently in two files**, **mic capture stubbed
out on macOS**, and **a 3,167-line god class** in `app.js`. There are zero tests and
no CI gate for PRs.

## Findings

### P0 — Bugs & security

| # | Issue | Evidence | Impact |
|---|-------|----------|--------|
| 1 | **Settings fields silently dropped on save.** JS sends `translation_type`, `language_a`, `language_b`, `language_hints_strict`, `endpoint_delay`, `font_family` is in struct but `translation_type` etc. are not — serde drops unknown fields, so two-way config and endpoint delay are lost every save/restart. | `commands/settings.rs` → `Settings` struct in `settings.rs` lacks the fields; `app.js:813-889` sends them | Two-way mode "forgets" itself on restart |
| 2 | **API keys logged to console.** `console.log('...settings:', JSON.stringify(settings))` dumps all 5 API keys. | `app.js:1184` | Keys leak into devtools/logs |
| 3 | **Secrets double-stored & inconsistent.** `interview_set_api_key` writes `secrets.json`; `save_settings` also persists `llm_api_key`/`pinecone_api_key` into `settings.json`. Commands read only from settings → `secrets.rs` is a dead-end write path. Both files plaintext. | `secrets.rs`, `commands/secrets.rs`, `interview.rs:582`, `app.js:2591` | Confusing contract; keys in 2 plaintext locations |
| 4 | **Mic capture unimplemented on macOS/Linux.** `#[cfg(not(windows))]` arm returns `"not implemented"`. `cpal` dep is declared but never used — README advertises it. | `audio/microphone.rs:166-170` | Mic/"Both" modes broken on macOS |
| 5 | **XSS via stored settings.** `_addTermRow` interpolates `source`/`target` into `innerHTML` without escaping; `_addGeneralRow` escapes correctly — inconsistent. A `"` or `<` in a saved term corrupts/injects DOM. | `app.js:1036-1046` vs `1053` | Stored settings → DOM injection |
| 6 | **Stop button ≠ keyboard stop.** Button calls `_stopCapture()` (no autosave); `⌘/Ctrl+Enter` calls `stop()` (autosaves session). | `app.js:347` vs `600` | Divergent behavior, transcript save depends on how you stop |

### P1 — Correctness & performance

| # | Issue | Evidence |
|---|-------|----------|
| 7 | Naive resampling everywhere: WASAPI system audio uses nearest-neighbor (`mono[src_idx]`); macOS ScreenCaptureKit decimates `step_by(3)` with no anti-alias filter; mic uses linear interp. Three different-quality paths feeding the same STT engine. | `wasapi.rs:464-470`, `system_audio.rs:43-47`, `microphone.rs:293` |
| 8 | Audio bytes to local pipeline sent via `invoke` as `Array.from(Uint8Array)` — JSON array, ~4-10× bloat, 5 calls/sec. | `app.js:1382` → `send_audio_to_pipeline(data: Vec<u8>)` |
| 9 | CV/JD upload sends file bytes as JSON array (2 MB PDF → ~8 MB IPC payload). Should pass a file path or use Tauri dialog + `fs::read`. | `app.js:2695-2699` → `InterviewFilePart.bytes: Vec<u8>` |
| 10 | `check_permissions` is a stub returning `"unknown"`. | `commands/audio.rs:187-194` |
| 11 | Dead deps in Cargo.toml: `cpal` (unused), `keyring` (abandoned per secrets.rs comment). | `Cargo.toml:22,32` |
| 12 | `local_pipeline.rs`: `pkill -f local_pipeline.py` kills any matching process; hardcoded `/Users/phucnt` HOME fallback; `/tmp` log path. macOS-gated so low blast radius, but fragile. | `local_pipeline.rs:48,83` |
| 13 | PROPVARIANT blob `CoTaskMemAlloc`'d but never freed (small leak per capture start); stale contradictory comment claims heap corruption. | `wasapi.rs:149-187` |
| 14 | `h1.join().unwrap()` panics if DB thread panics; subtitle labels hardcode "EN:"/"VI:" regardless of configured languages. | `interview.rs:809`, `ui.js:538-539` |
| 15 | Version drift: `get_platform_info` → `0.3.0`, console banner `v0.5.0`, real version `1.0.0`. | `lib.rs:21`, `app.js:127` |

### P2 — Architecture & maintainability

| # | Issue |
|---|-------|
| 16 | `app.js` god class: 3,167 lines, ~60 instance fields — sessions, settings form, TTS, sidebar, interview RAG UI, mobile, updater, shortcuts, drag all in one class. |
| 17 | Dual source of truth for settings schema (JS `DEFAULT_SETTINGS` vs Rust `Settings`) — direct cause of finding #1. |
| 18 | Zero tests. No `#[cfg(test)]` in Rust, no JS test framework. CI only runs release builds on tags — no PR-time `cargo test`/`clippy`/lint. |
| 19 | `println!/eprintln!` logging, no `tracing`; secrets can leak via logs. |
| 20 | Saved-transcript round trip: markdown is re-parsed heuristically for read-only view — fragile; store a JSON sidecar instead. |

## Enhancement Plan

### Phase 1 — Correctness & security hotfixes (P0) ✅ DONE 2026-09-30
**Effort: ~1 day. No architecture changes.**

1. ✅ Added missing fields to Rust `Settings`: `translation_type`, `language_a`, `language_b`, `language_hints_strict`, `endpoint_delay`. Three unit tests added (full-payload round-trip, defaults fallback, no-keys-in-persisted-JSON) — all passing.
2. ✅ Removed `JSON.stringify(settings)` console dump (`app.js` `start()`). Audited remaining logs — no other key leakage found.
3. ✅ Unified secrets in `secrets.json`: `SecretSlot` extended to all 5 providers; `Settings::save()` offloads keys to the store and writes a stripped file; `get_settings` hydrates keys; `Settings::load()` migrates legacy in-file keys on startup. `interview.rs` now reads via `secrets::get_secret`. Frontend contract unchanged.
4. ✅ `_addTermRow` now escapes via `_escAttr` (matches `_addGeneralRow`).
5. ✅ Stop button now calls `this.stop()` — converged with ⌘/Ctrl+Enter path (auto-saves session).
6. ✅ `get_platform_info` returns `env!("CARGO_PKG_VERSION")`; init banner uses `app.getVersion()`.

**Verification:** `cargo test` — 3/3 pass; `cargo clippy` — no new warnings (10 pre-existing in interview.rs/local_pipeline.rs, tracked in Phase 5); dev build running.

### Phase 2 — Audio pipeline hardening ✅ DONE 2026-09-30
1. ✅ New `audio/cpal_mic.rs` — cpal input → mono mix → filtered resample → s16le, extracted from the proven Android impl. Now `MicCapture` for all unix targets (macOS/Linux/Android); fixes the macOS "not implemented" stub. Windows keeps its bespoke WASAPI path.
2. ✅ New `audio/resample.rs` — shared streaming resampler: Hamming FIR decimator for integer ratios (48k→16k is now anti-aliased, replacing `step_by(3)` and nearest-neighbor) + boundary-continuous linear interpolator. Wired into wasapi/microphone/system_audio/cpal_mic. 5 unit tests (in-band preservation, alias attenuation, chunk continuity).
3. ✅ `send_audio_to_pipeline` takes a raw `tauri::ipc::Request` body (`invoke(cmd, uint8Array)`); JSON-array fallback kept for Android (raw IPC unsupported there).
4. ✅ Path-based ingest: `InterviewFilePart.path` → `fs::read` Rust-side with 25 MB cap, empty-file guard, `user_id` sanitization. Frontend uses `tauri-plugin-dialog` picker (added dep + `dialog:allow-open` capability) with `<input type=file>` fallback.
5. ✅ `check_permissions`: Windows — loopback "granted", mic probes default capture device; macOS — `SCShareableContent::get()` probe + cpal input probe; Android — honest "unknown".
6. ✅ PROPVARIANT comment corrected — blob is CoTaskMem-allocated so `Drop` frees it via `PropVariantClear`; the old "must be forgotten" comment was stale.
7. ✅ `local_pipeline.rs`: `pkill` cfg-gated to macOS + narrowed to resolved script path; `dirs::home_dir()` replaces hardcoded `/Users/...`; log → `temp_dir()/myjavis_pipeline.log`; per-platform python/PATH (`python` on Windows, no PATH override).
8. ✅ macOS `Info.plist` already had `NSMicrophoneUsageDescription` — verified.

**Verification:** `cargo test` — 8/8 pass; `cargo clippy` — 6 warnings remain, all pre-existing in `interview.rs`/`edge_tts.rs` (Phase 5); dev build boots on Windows (PID 11672). macOS paths (`system_audio.rs`, `cpal_mic.rs`, SCKit permission probe) compile-gated — need on-device verification on a Mac.

### Phase 3 — Frontend refactor (split `app.js`) ✅ DONE 2026-09-30
**Approach: prototype mixins.** Methods moved verbatim into `export const xMethods = {...}` objects in sibling modules, merged via `Object.assign(App.prototype, ...)` — preserves `this` semantics, zero behavior change. Extraction was driven by `scripts/extract_app_methods.cjs` (lexically-aware brace matching; dry-run coverage check refuses to write if any method is unassigned/dropped). Pre-split backup kept at `scripts/app.js.pre-split.bak`.

Modules produced:
- `session.js` (737 lines, 21 methods) — start/stop/end lifecycle, capture, transcript save, status, source/mode switching
- `interview-panel.js` (978 lines, 36 methods) — suggestions panel, uploads/ingest, chat, dock/undock, streaming render
- `settings-form.js` (519 lines, 15 methods) — populate/save, term/general rows, provider UIs, dim chips, about tab
- `conversations.js` (199 lines, 6 methods) — sidebar list, read-only view, session meta
- `tts.js` (115 lines, 5 methods) — provider select/configure/toggle/speak
- `window.js` (82 lines, 4 methods) — position persistence, pin, font size
- `updater-ui.js` (83 lines, 3 methods) — update check + toast
- `shortcuts.js` (95 lines) — keyboard shortcuts
- `app.js` stub (569 lines) — imports, constructor, init, `_bindEvents`, `_showView`, `_showToast`, `_insertIntoTextarea`, `Object.assign`, bootstrap

Each module imports only the singletons it references (scanner-detected).

**Verification:** `node --check` all 18 JS files pass; 97/97 methods accounted for exactly once (script-enforced); dev build boots (PID 6580). Manual smoke test still advised: start/stop, settings save, TTS, interview suggestions, upload dialog.

### Phase 4 — Settings schema single source of truth ✅ DONE 2026-09-30
Implemented **Option A variant** — strict validation on save, lenient on load:
- `save_settings` takes `serde_json::Value`; payload keys are checked against the struct's own serialized field set (derived at runtime — no manual list to drift). Unknown keys → loud error naming offenders, so frontend/struct drift fails fast instead of silently dropping.
- `load` stays lenient — `deny_unknown_fields` on the struct would have made a hand-edited or newer-version `settings.json` fail and fall back to defaults, wiping user prefs.
- `app_mode` tolerates `null` (JS `DEFAULT_SETTINGS` sends it) via a `deserialize_null_default` helper.
- Tests: `rejects_unknown_fields`, `accepts_all_known_frontend_keys` (full 35-key payload incl. `null`), plus the Phase-1 round-trip tests.
- Option B (ts-rs type generation) deferred — revisit if the schema grows.

**Transcript sidecar:** `save_transcript` accepts optional `segments` and writes `<name>.segments.json` next to the `.md`; new `read_transcript_segments` command; `delete_transcript` removes the sidecar. Read path (`_openConversationReadOnly`) prefers the sidecar, falls back to `_parseSavedTranscriptToSegments` for pre-sidecar files.

### Phase 5 — Test & CI foundation ✅ DONE 2026-09-30
1. **Rust tests** (20 total): `chunk_text_with_overlap` (word boundaries, overlap carry-forward, oversized words, empties), `parse_suggestions_from_llm` (JSON rows/strings/fallback/unknown type), resampler, settings round-trip + strict-save.
2. **vitest** added (devDep `^3`, `npm test`): `tests/setup.js` stubs `window.__TAURI__`; `tests/conversations.test.js` covers the markdown transcript parser + session meta; `tests/settings.test.js` covers load/merge/save/error paths.
3. **`.github/workflows/ci.yml`**: runs on PR + push. Frontend job: `node --check` all modules + vitest. Rust job: `cargo fmt --check`, `clippy --all-targets -D warnings`, `cargo test` — **matrix windows-latest + macos-latest** so the macOS-gated code (SCKit, cpal) gets compile-checked in CI since no local Mac exists. One-time `cargo fmt` applied to make fmt gating pass.
4. **`tracing`**: `tracing-subscriber` with `EnvFilter` (RUST_LOG, default info) initialized in `run()`; all 20 `println!`/`eprintln!` sites converted to `info!`/`warn!`/`error!` with levels matching severity. No secret-bearing strings are logged — maintain that when adding fields.
5. Also done: all 6 pre-existing clippy warnings fixed (`IngestCtx` param struct for the 11-arg `ingest_one_doc`, lifetime elision, `nth()` byte lookup, char-array pattern, `from_ref`, `//!` doc), dead `keyring` dep removed. `cargo clippy --all-targets` is now **zero-warning**.

## Success Criteria
- [x] `Settings` round-trips every field the form sends (test proves it) — plus strict rejection of unknown keys
- [x] `grep -r "api_key" src/js` shows no console logging of secrets; keys live in exactly one store
- [ ] Mic works on macOS — implemented via shared cpal path; needs on-device or CI-macOS verification
- [x] `app.js` < 600 lines (569); 97 methods distributed across 8 modules, script-verified
- [x] `cargo test` (20) and `vitest` (10) run green locally; CI workflow lands with this change

## Risks
- **Secrets migration**: must not lose existing keys — write migration that copies settings.json → secrets.json once, then strips. Rollback = keep reading settings as fallback for one release.
- **app.js split**: high regression surface (event wiring order matters). Mitigate by extracting one module at a time and manually smoke-testing start/stop/TTS/suggestions after each.
- **Resampler swap**: perceptible quality change possible; A/B against recorded PCM before shipping.
- **Soniox session reset**: don't touch `soniox.js` make-before-break logic during refactor — it's subtle and correct.

## Open Questions
- Should `stop()` auto-save always, or should the button remain "pause"? (Finding #6 — needs product decision.)
- Local (MLX) mode is macOS-only and touches `scripts/` — is Windows/Linux local mode on the roadmap? Affects Phase 2 item 3 priority.
- Is `web-speech-tts.js` still reachable? It exists but isn't imported by `app.js` — candidate for deletion if dead.
