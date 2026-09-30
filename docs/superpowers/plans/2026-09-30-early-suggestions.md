# Interview Early Suggestions — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Streamed, cancellable answer hints that appear while the interviewer is still speaking — driven by a dedicated System-audio Soniox stream separate from the Mic stream.

**Architecture:** Split `both` capture into two tagged IPC channels feeding two independent `SonioxClient` instances; per-source transcript state in `TranscriptUI`; a periodic scheduler (600 ms tick) that detects stable question prefixes and fires single-flight, epoch-tagged generation requests; an async SSE-streaming LLM path in Rust emitting deltas over a Tauri channel.

**Tech Stack:** Tauri 2, Rust (reqwest async + tokio + futures-util stream), vanilla JS prototype-mixin frontend, Vitest, cargo test.

**Spec:** `docs/superpowers/plans/2026-09-30-ui-enhancement-plan.md` — section "Interview early suggestions — design for review" (approved in conversation).

## Global Constraints

- One session = one file; Stop stays a pure pause; save failures never drop `sessionLog`.
- `sessionLog` entries may add optional `source` and `id` fields; old sidecars + markdown must still parse (existing tests must stay green).
- Single-flight: max 1 in-flight generation, 1 pending snapshot, ≤3 speculative starts/turn, ≤20 generation starts/rolling minute, ≥1.5 s between starts.
- Early suggestions requires `translationMode === 'soniox'` AND source `system`|`both`; Mic-only and Local keep manual suggestions only.
- Runtime TTS narration must be OFF while early suggestions is active (system loopback would transcribe the app's own voice).
- No logging of API keys, raw prompts, transcript text, or CV excerpts — timings/counts/request IDs only.
- New persisted setting `early_suggestions` (bool, default `false`) must pass strict save validation: update `settings.rs`, `settings.js` defaults, and form populate/save together.
- Epoch/gen tagging: every async result carries `sessionEpoch + turnId + revision + requestId`; stale results and stale cancels are dropped, never applied.
- Verify gates stay green: `node --check`, `npm test`, `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check`.

## Review Focus

1. **Provisional text that shrinks** (Soniox corrects earlier words): scheduler must treat it as a revision/correction, not append-only growth — test in Task 6.
2. **Partial startup failure**: System capture starts, Mic fails — must roll back System and surface which source failed — test in Task 1.
3. **Malformed/stalled SSE stream**: split UTF-8, missing `[DONE]`, error payloads mid-stream — parser must not hang or emit garbage — tests in Task 4.
4. **Reconnect overlap**: both streams reconnecting near-simultaneously — callbacks from superseded connections must be ignored — test in Task 2.
5. **Mic echo / candidate speech**: a committed Mic segment while a hint is visible auto-holds updates; Mic speech alone must never trigger generation — test in Task 6.

---

### Task 1: Split audio capture per source (Rust)

**Files:**
- Modify: `src-tauri/src/commands/audio.rs:8-188`
- Test: unit tests inside `src-tauri/src/commands/audio.rs`

**Interfaces:**
- Produces: `start_split_capture(system_channel: Channel<Vec<u8>>, mic_channel: Channel<Vec<u8>>) -> Result<(), String>` — two independent IPC channels, one per source. `stop_capture` unchanged signature, now stops all forwarders.
- `AudioState.active_receiver` becomes `Mutex<Vec<AudioForwarder>>` (one entry per active forwarder).

- [ ] **Step 1: Write the failing test**

In `audio.rs` tests: a shared `forward_batches(receiver, channel, stop_flag)` helper extracted from the forwarding loop — test that a `source` variant "both" registers two forwarders (assert `active_receiver` vec length 2 after `start_split_capture` with stub receivers — use a `start_with_receivers` internal fn taking pre-made `mpsc::Receiver`s so no real audio devices are touched).

```rust
#[test]
fn split_capture_registers_two_forwarders() {
    let (tx1, rx1) = mpsc::channel::<Vec<u8>>();
    let (tx2, rx2) = mpsc::channel::<Vec<u8>>();
    let (ch1, ch2) = (Channel::new(|_| Ok(())), Channel::new(|_| Ok(())));
    let state = AudioState::default();
    start_forwarders(&state, rx1, ch1).unwrap();
    start_forwarders(&state, rx2, ch2).unwrap();
    assert_eq!(state.active_receiver.lock().unwrap().len(), 2);
    drop((tx1, tx2));
}
```

- [ ] **Step 2: Run `cargo test split_capture` — expect FAIL** (`AudioState` field type mismatch / missing fn).

- [ ] **Step 3: Implement**

- `AudioState { system_audio, microphone, active_receiver: Mutex<Vec<AudioForwarder>> }`; add `impl Default`.
- Extract forwarding loop (batch 200 ms / flush-on-stop) into `fn spawn_forwarder(rx: mpsc::Receiver<Vec<u8>>, channel: Channel<Vec<u8>>) -> AudioForwarder`.
- `start_capture` uses `spawn_forwarder` once; `start_split_capture` starts `system_audio.start()` + `microphone.start()`, spawns two forwarders; if mic `start()` fails after system started, call `sys.stop()` and return `Err("microphone: {e}")` with the source named.
- `stop_capture_inner` drains the whole vec.

- [ ] **Step 4: Run `cargo test` — PASS; `cargo clippy --all-targets` clean.**

- [ ] **Step 5: Register command in `src-tauri/src/lib.rs` `invoke_handler` and commit.**

```bash
git add src-tauri/src/commands/audio.rs src-tauri/src/lib.rs
git commit -m "feat(audio): split system/mic capture into separate IPC channels"
```

---

### Task 2: Second Soniox client + source-tagged wiring (frontend)

**Files:**
- Modify: `src/js/soniox.js:31-50,519-524` (export a second instance), `src/js/session.js` (`_startSonioxMode`, `_stopCapture`), `src/js/app.js` (callback wiring ~519-536)

**Interfaces:**
- Produces: `export const sonioxMicClient` in `soniox.js`. In App: `this._sourceClients = { system: sonioxClient, mic: sonioxMicClient }`; every transcript callback passes a `source` argument: `addOriginal(text, speaker, language, source)`, `addTranslation(text, source)`, `setProvisional(text, speaker, language, source)`, `clearProvisional(source)`.

- [ ] **Step 1: Write the failing vitest**

`tests/early-suggestions.test.js` — new file. Assert `sonioxMicClient` is an independent instance with its own `isConnected`:

```js
import { sonioxClient, sonioxMicClient } from '../src/js/soniox.js';

test('mic client is an independent SonioxClient instance', () => {
    expect(sonioxMicClient).not.toBe(sonioxClient);
    sonioxClient.isConnected = true;
    expect(sonioxMicClient.isConnected).toBe(false);
});
```

- [ ] **Step 2: `npm test` — FAIL (export missing).**

- [ ] **Step 3: Implement**

- `soniox.js`: `export const sonioxMicClient = new SonioxClient()`.
- `app.js` wiring: per-client callbacks closing over `source` (`'system'` / `'mic'`); `onStatusChange` for mic maps to a lighter path — mic failure shows a warning toast ("Mic stream lost — auto-hold unavailable") but does not flip main status to error while system is connected.
- `session.js` `_startSonioxMode`: when `currentSource === 'both' && this._isSuggestionsMode()` → `invoke('start_split_capture', { systemChannel, micChannel })`, route each channel's PCM to its client's `sendAudio`; else the existing merged `start_capture` path (mic/system/`both` all still work in normal mode).

- [ ] **Step 4: `npm test` PASS; `node --check` all touched files.**

- [ ] **Step 5: Commit**

```bash
git add src/js/soniox.js src/js/session.js src/js/app.js tests/early-suggestions.test.js
git commit -m "feat(interview): independent Soniox client per audio source"
```

---

### Task 3: Per-source transcript state in TranscriptUI

**Files:**
- Modify: `src/js/ui.js` (constructor 14-31, `addOriginal` 63-90, `addTranslation` 95-121, `setProvisional`/`clearProvisional` 146-162, render+getFullSessionText)
- Test: `tests/early-suggestions.test.js`

**Interfaces:**
- Produces: segment/log objects gain `source: 'system'|'mic'|null` and `id` (monotonic per UI instance). `transcriptUI.provisionalText` remains a getter-equivalent — returns the **system** provisional (session chip unchanged). New: `provisionalBySource`, `addOriginal(...,source)`, `addTranslation(text,source)` aligning to oldest untranslated **of that source**, `committedTextBySource(source) -> string`.

- [ ] **Step 1: Failing tests**

```js
ui.addOriginal('hello', 'S1', 'en', 'system');
ui.setProvisional('draft', 'S2', 'vi', 'mic');
expect(ui.provisionalBySource.mic.text).toBe('draft');
expect(ui.provisionalText).toBe('');            // system tail untouched

ui.addOriginal('xin chao', null, 'vi', 'mic');
ui.addTranslation('mic-translated', 'mic');      // aligns to mic seg, not system
const micSeg = ui.sessionLog.find(s => s.source === 'mic' && s.original === 'xin chao');
expect(micSeg.translation).toBe('mic-translated');
const sysSeg = ui.sessionLog.find(s => s.source === 'system');
expect(sysSeg.translation).toBeNull();
```

- [ ] **Step 2: `npm test` — FAIL.**

- [ ] **Step 3: Implement**

- Constructor: `this.provisionalBySource = { system: {text:'',speaker:null,language:null}, mic: {...} }`; `provisionalText` getter → `this.provisionalBySource.system.text`; add setter writing the system entry (existing callers `clearProvisional()`/`provisionalText` reads keep working — `clearProvisional(source)` clears one source when given, both when omitted... no: spec keeps single calls safe — default `source='system'`).
- `addOriginal`/`addChatMessage`/`addTranslation` accept `source` (default `null` for legacy callers); push `{id: this._nextSegId++, source}` into both `segments` and `sessionLog`.
- `addTranslation` matches `s.status==='original' && s.source===source`.
- `_render` shows each source's provisional tail; mic lines get a subtle `data-source` attr (no visual change required beyond existing speaker labels — keep minimal).
- `committedTextBySource(source)`: joins `sessionLog` originals+translations filtered by source, chronological.
- Old `.segments.json` without `source`/`id` must still parse — `conversations.js` loader treats missing fields as `null`.

- [ ] **Step 4: `npm test` PASS (all 10 existing + new).**

- [ ] **Step 5: Commit**

```bash
git add src/js/ui.js src/js/conversations.js tests/early-suggestions.test.js
git commit -m "feat(transcript): per-source segments and provisional state"
```

---

### Task 4: Async SSE-streaming LLM path with cancellation (Rust)

**Files:**
- Modify: `src-tauri/Cargo.toml` (reqwest `stream` feature), `src-tauri/src/services/llm.rs`, `src-tauri/src/commands/interview.rs`, `src-tauri/src/lib.rs`
- Test: unit tests in `src-tauri/src/services/llm.rs`

**Interfaces:**
- Produces:
  - `pub async fn complete_suggestions_stream(client: &reqwest::Client, url, key, model, prompt, cancel: Arc<AtomicBool>, on_delta: impl FnMut(String)) -> Result<String, String>`
  - `pub fn sse_feed(buf: &mut Vec<u8>, chunk: &[u8]) -> Vec<String>` — pure incremental SSE data-line extractor; testable without network.
  - `#[tauri::command] async fn suggest_interview_answers_stream(req: SuggestInterviewAnswersRequest, request_id: u32, channel: Channel<StreamEvent>, state: State<'_, SuggestStreamState>, ...)` emitting `StreamEvent::{Start,Delta{text},Done{full},Insufficient,Cancelled,Error{message}}` — all serialized with `requestId`.
  - `#[tauri::command] fn cancel_suggestion_stream(request_id: u32, state)` — sets the flag; unknown IDs are no-ops (stale cancel never aborts newer work).

- [ ] **Step 1: Failing tests for `sse_feed`**

```rust
#[test]
fn sse_feed_handles_split_utf8_and_multi_events() {
    let mut buf = Vec::new();
    let (a, b) = ("data: {\"c\":\"hél".as_bytes(), "lo\"}\n\ndata: [D".as_bytes());
    assert!(sse_feed(&mut buf, a).is_empty());
    assert_eq!(sse_feed(&mut buf, b), vec!["{\"c\":\"héllo\"}".to_string()]);
}
#[test]
fn sse_feed_tolerates_malformed_line() {
    let mut buf = Vec::new();
    let out = sse_feed(&mut buf, b": junk\r\ndata: ok\n\n");
    assert_eq!(out, vec!["ok".to_string()]);
}
```

- [ ] **Step 2: `cargo test sse_feed` — FAIL.**

- [ ] **Step 3: Implement**

- `Cargo.toml`: reqwest features += `stream`; `bytes = "1"` if needed for `bytes_stream`.
- `sse_feed`: append bytes, yield complete `data:` payloads up to last `\n\n` (or `\n` per line tolerance), keep remainder; UTF-8 via `String::from_utf8_lossy` **per completed line only** (never split mid-char — slice on `\n` byte boundaries, lossy per line).
- `complete_suggestions_stream`: POST same body + `"stream": true`; iterate `response.bytes_stream()`; feed `sse_feed`; for each payload: `[DONE]` → finish; parse `choices[0].delta.content`; check `cancel` flag between chunks → return `Err("__cancelled__")` sentinel distinguished by caller; non-2xx → read body once → error string (status + first 200 chars, no echo of prompt).
- `interview.rs`: `SuggestStreamState { cancels: Mutex<HashMap<u32, Arc<AtomicBool>>> }` managed in `lib.rs`; command inserts flag, runs stream in spawned task writing `StreamEvent`s to channel, removes flag on terminal event; `suggest_interview_answers` (blocking) stays for Meeting/manual path.
- The early path prompt is **plain-text answer** (not JSON array) — new `req.stream_hint: Option<bool>`; when true, prompt asks for 2–3 short talking points as plain text, one per line, no JSON.

- [ ] **Step 4: `cargo test` PASS, clippy clean.**

- [ ] **Step 5: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/services/llm.rs src-tauri/src/commands/interview.rs src-tauri/src/lib.rs
git commit -m "feat(interview): cancellable SSE streaming for early suggestions"
```

---

### Task 5: `early_suggestions` setting + guards

**Files:**
- Modify: `src-tauri/src/settings.rs` (field + default + test payload), `src/js/settings.js` (`DEFAULT_SETTINGS`), `src/index.html` (AI tab checkbox + hint), `src/js/settings-form.js` (populate/save), `src/js/tts.js:15-56`

**Interfaces:**
- Produces: `settings.early_suggestions` (bool). `_earlySuggestionsEligible()` (implemented Task 6) is the single gate used by TTS/scheduler: `early_suggestions && translationMode==='soniox' && (currentSource==='system'||'both') && _isSuggestionsMode()`.

- [ ] **Step 1: Failing tests** — settings.test.js: `settingsManager` round-trips `early_suggestions: true`; settings.rs test payload gains `"early_suggestions": true` (proves strict-save accepts it).

- [ ] **Step 2: Run — FAIL (unknown field rejected / undefined).**

- [ ] **Step 3: Implement** — Rust `pub early_suggestions: bool` + `#[serde(default)]`-style default `false` in `Default`; JS default `false`; checkbox `check-early-suggestions` in AI tab next to suggestion type with hint text "Streams LLM calls while the interviewer speaks — needs System or System+Mic audio; increases API cost"; populate + save wiring; `_toggleTTS` early-return with toast "TTS is off while Early suggestions is active" when eligible; enabling the checkbox while `ttsEnabled` → `tts.disconnect(); audioPlayer.stop(); this.ttsEnabled=false; _updateTTSButton()` + explanatory toast.

- [ ] **Step 4: All tests PASS.**

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/settings.rs src/js/settings.js src/index.html src/js/settings-form.js src/js/tts.js tests/
git commit -m "feat(settings): early_suggestions flag + TTS loopback guard"
```

---

### Task 6: Early-suggestion scheduler + Hold/Resume UI

**Files:**
- Create: `src/js/early-suggestions.js` (prototype mixin, `Object.assign`'d in `app.js`)
- Modify: `src/js/interview-panel.js` (`_onInterviewSpeakerFinal` 540-558 routes through scheduler when eligible; render gains revision-aware draft area), `src/index.html` (Hold/Resume buttons + status label in suggestions header), `src/styles/main.css`, `src/js/app.js` (import/mixin, init tick)

**Interfaces:**
- Consumes: `transcriptUI.committedTextBySource`, `provisionalBySource`, `sonioxClient`, `suggest_interview_answers_stream`, `cancel_suggestion_stream`, `settings.early_suggestions`.
- Produces: `_earlySuggestionsEligible()`, `_earlyTick()` (600 ms interval while running), `_earlyState` `{epoch, turnId, revision, inFlight: {requestId, snapshot}, pending, capsUsed[], holdMode: 'none'|'auto'|'manual'}`, `window.__early` debug handle for tests.

- [ ] **Step 1: Failing vitest — scheduler logic extracted pure**

```js
import { stablePrefixOk, normalizeSnapshot, EarlyScheduler } from '../src/js/early-suggestions.js';

test('stable prefix needs 2 identical observations and ≥4 words', () => {
    const s = new EarlyScheduler({ now: () => t });
    // feed same snapshot twice → eligible; single obs → not
});

test('shrinking snapshot = correction → invalidates in-flight', () => {
    // snapshot "tell me about deadlock" then "tell me about" → revision++, inFlight cancelled
});

test('caps: 3 per turn, 20/min, 1.5s spacing', () => { /* fake clock */ });
test('mic-only input never produces a request', () => { /* source='mic' fed → no fire */ });
```

- [ ] **Step 2: `npm test` — FAIL.**

- [ ] **Step 3: Implement `early-suggestions.js`**

- `EarlyScheduler` class: pure logic (inject `now()`, `fire(snapshot)`, `cancel(requestId)`); App mixin wraps it with real IO.
- Turn detection: system provisional reset-to-empty→new text = new `turnId`; committed system segment appended after candidate speech also bumps `turnId`.
- `_earlyTick` only when `isRunning && _earlySuggestionsEligible()`; eligibility function itself is exported for Task 5 gate + tests.
- Fire path: `invoke('suggest_interview_answers_stream', {req: {..., streamHint: true}, requestId, channel})`; channel events routed by `requestId`; `Delta` appends to draft card (early styling class), `Done` promotes draft → replaces current card only if epoch+turn match and no hold; `Insufficient` → status "Waiting for more of the question…"; stale requestId → drop.
- Hold: committed Mic segment while a card is visible → `holdMode='auto'` + label "Held while you speak"; Hold button → `'manual'`; Resume or new system turn clears `'auto'` only.
- Endpoint refinement: on system `<end>`/final boundary — allow one final fire if snapshot ≠ last answered.
- `stop()`/`_createNewSession()`/`_openConversationReadOnly`/mode change → `++epoch`, cancel in-flight, keep card text on stop (per spec).

- [ ] **Step 4: `npm test` PASS; `node --check`.**

- [ ] **Step 5: Commit**

```bash
git add src/js/early-suggestions.js src/js/interview-panel.js src/js/app.js src/index.html src/styles/main.css tests/
git commit -m "feat(interview): early-suggestion scheduler with hold/resume"
```

---

### Task 7: Pre-warmed CV/JD context for the early path

**Files:**
- Modify: `src-tauri/src/commands/interview.rs`, `src-tauri/src/db/mod.rs`, `src/js/interview-panel.js` (`_runInterviewSuggestions` early path passes prepared excerpts)
- Test: `tests` + Rust unit test for the lexical selector

**Interfaces:**
- Produces: `#[tauri::command] fn select_context_excerpts(user_id, query, max_chars: usize) -> Result<Vec<ContextExcerpt>, String>` where `ContextExcerpt { doc_type: String, text: String }` — reads `documents.extracted_text`, chunks at `INGEST_CHUNK_MAX_CHARS` reuse, scores by token-overlap with `query`, returns CV-tagged and JD-tagged excerpts separately (JD never labeled as candidate experience).

- [ ] **Step 1: Failing Rust test** — `score_excerpt("deadlock transaction", chunk)` ranks a chunk mentioning "deadlock" above an unrelated one; empty query returns bounded first-chunks fallback.

- [ ] **Step 2: `cargo test` — FAIL.**

- [ ] **Step 3: Implement** — lexical scoring (normalized token overlap count), `select_context_excerpts`, frontend: on Interview start + early enabled, warm an in-memory excerpt cache keyed by `(docId hash)`; stream requests send top excerpts inline in `req.transcriptContext` envelope or new `req.context_snippets` field; Pinecone path stays for the final/manual refinement only.

- [ ] **Step 4: Tests PASS.**

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/interview.rs src-tauri/src/db/mod.rs src/js/interview-panel.js
git commit -m "feat(interview): local lexical context excerpts for fast hints"
```

---

### Task 8: Regression + verification sweep

**Files:**
- Modify: `tests/early-suggestions.test.js`, docs plan checkboxes

- [ ] **Step 1:** Add the remaining spec-required vitest cases not covered above: stale `requestId` delta dropped; epoch bump on `stop()`; hold released only by new system turn or Resume; `save`-failure keeps session (existing test still passes with `source` fields present).

- [ ] **Step 2:** Full gate: `node --check` all JS, `npm test`, `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check`.

- [ ] **Step 3:** `npm run tauri dev` — manual smoke: Interview + Both sources; speak a long question, watch for early hint mid-sentence; speak over it (mic) → "Held while you speak"; Resume; `+New` saves one file with both sources.

- [ ] **Step 4:** `npm run tauri build` — fresh portable exe.

- [ ] **Step 5:** Update plan docs (mark section implemented, record measured latencies from smoke test). Commit.

```bash
git commit -m "test+docs: early suggestions verification sweep"
```

---

## Sequencing Notes

- Tasks 1→3 are the audio/transcript foundation and must land in order.
- Task 4 (streaming) and Task 5 (settings flag) are independent of each other; Task 6 needs 2–5; Task 7 needs 4; Task 8 last.
- If Task 4's async migration proves invasive, the blocking client may remain for Meeting/manual — only the early path must stream.
