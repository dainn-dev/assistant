# Profile, Post-Session Review & Updater Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Interview mode a persistent candidate Profile (summary + STAR stories) that grounds every suggestion, add a one-shot post-session review of a saved interview, and make the auto-updater actually deliver releases.

**Architecture:** Three independent parts, each shippable on its own. (A) Profile lives in the existing SQLite `InterviewDb` next to `documents`, and feeds the existing `select_context_excerpts` lexical path so both manual and early suggestions pick it up with no new prompt plumbing. (B) Review is a single Rust command that reads the `<name>.segments.json` sidecar, makes one LLM call, and caches `<name>.review.json`; the frontend renders it in the right panel while in read-only mode. (C) Updater is config + workflow repair: correct endpoint, resilient release job, signing key procedure, version bump script.

**Tech Stack:** Tauri v2 (Rust: rusqlite, reqwest, serde), vanilla JS modules merged onto `App.prototype`, Vitest, GitHub Actions, `tauri-plugin-updater`.

**Spec:** This plan is self-contained; the product direction is item 3 ("Profile + post-session review") and item 5 ("Auto-updater thật") of the roadmap discussed 2026-10-01. Early-suggestion design: `docs/superpowers/plans/2026-09-30-ui-enhancement-plan.md` §Interview Early Suggestions.

## Global Constraints

- No API keys serialized into `settings.json`, logged, or included in review/profile files. Review JSON stores `model` name only.
- Every LLM call that a user did not explicitly click for is forbidden in this plan: Profile drafting and Review generation are button-triggered, never automatic.
- Review and profile text are user data: stored under the app data dir (`transcript_dir` / SQLite), never in the repo.
- Backend parses LLM JSON tolerantly via the existing `extract_json_array_slice` pattern in `src-tauri/src/commands/interview.rs:317`; never `unwrap` on model output.
- Rust gates: `cargo test`, `cargo fmt -- --check`, `cargo clippy --all-targets -- -D warnings`. JS gates: `npm test -- --run`, `node --check` on every touched module.
- Frontend IPC goes through `src/js/ipc.js` (`invoke`, `createChannel`) — never destructure `window.__TAURI__.core` directly.
- Commit after every task; one task = one commit unless stated.

## Review Focus

Input classes the tasks' tests do not exercise but a real user will hit; each has a pinned test in the owning task:

1. **Sidecar without `source` tags** (transcripts saved before 2026-09-30) — review must still run, with roles labelled by `speaker` and the prompt told roles are unknown. → Task B1 test `dialogue_falls_back_to_speaker_labels`.
2. **Session with no candidate audio** (System-only recording) — review must not score answers that do not exist; `score` is `None` and the UI shows "—". → Task B1 test `no_mic_segments_yields_null_scores_hint`, Task B3 render test.
3. **Profile story that duplicates a CV chunk** — excerpt budget must not be eaten by two copies of the same text. → Task A3 test `story_and_cv_duplicates_keep_one`.
4. **LLM returns prose instead of JSON for profile draft** — draft dialog shows the error, saves nothing. → Task A2 test `draft_parse_rejects_non_json`.
5. **Release tag pushed with only Windows secrets configured** — a GitHub Release with `latest.json` listing only `windows-x86_64` must still be produced. → Task C3 verification (workflow dry run on a `v*-rc` tag).

---

## Part A — Candidate Profile

### Task A1: Profile storage + commands

**Files:**
- Modify: `src-tauri/src/db/mod.rs` (migration + 3 fns)
- Create: `src-tauri/src/commands/profile.rs`
- Modify: `src-tauri/src/commands/mod.rs`, `src-tauri/src/lib.rs` (register 3 commands)
- Test: inline `#[cfg(test)]` in `db/mod.rs` using `Connection::open_in_memory()`

**Interfaces:**
- Produces (Rust, `db`):
  ```rust
  pub struct ProfileItem { pub id: String, pub user_id: String, pub kind: String, pub title: String, pub content: String, pub updated_at: String }
  pub fn list_profile_items(conn: &Connection, user_id: &str) -> Result<Vec<ProfileItem>, String>  // ordered: summary first, then by updated_at DESC
  pub fn upsert_profile_item(conn: &Connection, item: &ProfileItem) -> Result<(), String>
  pub fn delete_profile_item(conn: &Connection, user_id: &str, id: &str) -> Result<(), String>
  ```
- Produces (Tauri commands, camelCase args): `list_profile(userId) -> Vec<ProfileItem>`, `save_profile_item(item: ProfileItem) -> ()`, `delete_profile_item(userId, id) -> ()`.
- `kind` is one of `"summary" | "story" | "strength"`. Exactly one `summary` per user: `upsert` with `kind == "summary"` first deletes any existing summary row for that user.

- [ ] **Step 1: Write failing tests in `db/mod.rs`**

```rust
#[test] fn profile_roundtrip_and_order() {
    // insert story, strength, summary (in that order) → list returns summary first, 3 items
}
#[test] fn profile_single_summary_per_user() {
    // upsert two summaries with different ids → list has exactly one summary, the latest content
}
#[test] fn profile_delete_scoped_to_user() {
    // delete with wrong user_id leaves the row
}
```

- [ ] **Step 2: Run `cargo test profile_` → FAIL (fn not found)**
- [ ] **Step 3: Add migration**

```sql
CREATE TABLE IF NOT EXISTS profile_items (
    id TEXT PRIMARY KEY, user_id TEXT NOT NULL, kind TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '', content TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_profile_user ON profile_items(user_id);
```
Implement the three fns; `updated_at` is set by the caller (commands use `chrono::Local::now().to_rfc3339()`).

- [ ] **Step 4: `cargo test profile_` → PASS**
- [ ] **Step 5: Create `commands/profile.rs` with the 3 commands** (lock `InterviewDb`, delegate). Register in `lib.rs` `generate_handler!`.
- [ ] **Step 6: `cargo clippy --all-targets -- -D warnings` && `cargo fmt -- --check` → clean**
- [ ] **Step 7: Commit** — `feat(profile): persistent profile_items table + CRUD commands`

### Task A2: Draft profile from CV/JD (one LLM call)

**Files:**
- Modify: `src-tauri/src/commands/profile.rs`
- Test: inline in `profile.rs`

**Interfaces:**
- Produces: `draft_profile_from_documents(userId) -> Vec<ProfileItem>` (Tauri command). Returns **unsaved** items with fresh `id`s (`uuid` or timestamp-based like existing message ids), `kind` ∈ {summary, story, strength}. Reads `db::get_documents` for the user; errors `"Upload a CV first (Settings → AI)."` when no `cv` doc.
- Produces (pure): `fn parse_profile_draft(raw: &str, user_id: &str, now: &str) -> Result<Vec<ProfileItem>, String>`.
- Prompt (plain, exact return contract):
  > Return ONLY a JSON array. Items: `{"kind":"summary","title":"","content":"<3–4 sentence first-person summary>"}` once; 5–8 `{"kind":"story","title":"<≤8 words>","content":"Situation: … Task: … Action: … Result: …"}`; 3–5 `{"kind":"strength","title":"<skill>","content":"<one sentence evidence from CV>"}`. Use only facts present in the CV; if the JD is present, prefer stories relevant to it. No markdown fences.

- [ ] **Step 1: Failing tests**

```rust
#[test] fn draft_parse_accepts_fenced_array() { /* ```json [..] ``` → 2 items, kinds preserved, ids non-empty, user_id set */ }
#[test] fn draft_parse_rejects_non_json() { /* "Here is your profile: …" → Err containing "JSON" */ }
#[test] fn draft_parse_drops_unknown_kind() { /* kind "hobby" filtered out, others kept */ }
```

- [ ] **Step 2: `cargo test draft_parse` → FAIL**
- [ ] **Step 3: Implement `parse_profile_draft` using `extract_json_array_slice`; implement the command**: build prompt from CV (`doc_type == "cv"`) text truncated to 12 000 chars + JD text truncated to 4 000 chars; call `llm::complete_suggestions(&client, &llm_url, &llm_key, &llm_model, &prompt)`; return parsed items. LLM settings resolved the same way `suggest_interview_answers` does (`settings::load` + `secrets`).
- [ ] **Step 4: Tests PASS; clippy/fmt clean**
- [ ] **Step 5: Commit** — `feat(profile): draft summary/STAR stories/strengths from uploaded CV`

### Task A3: Profile feeds excerpt selection

**Files:**
- Modify: `src-tauri/src/commands/interview.rs:884-929` (`select_context_excerpts`), prompt at `:820-833` and the early-hint prompt at `:1027`
- Test: inline tests next to `score_excerpt` tests

**Interfaces:**
- Consumes: `db::list_profile_items` (A1).
- Produces: `select_context_excerpts` now also returns `ContextExcerpt { doc_type: "summary" | "story" | "strength", text }`. `summary` is always first when present and does not count toward lexical ranking (charged to budget). Stories/strengths are scored with `score_excerpt(query, &format!("{title} {content}"))`, text emitted as `"{title}: {content}"`. Frontend already maps `[${docType.toUpperCase()}] text` → `[SUMMARY]`, `[STORY]`, `[STRENGTH]` appear in prompts with no JS change.
- Pure helper: `fn dedupe_excerpts(items: Vec<(usize, String, String)>) -> Vec<(usize, String, String)>` — drops a chunk whose lowercase text is contained in an earlier kept chunk or vice-versa (first kept wins).

- [ ] **Step 1: Failing tests**

```rust
#[test] fn excerpts_put_summary_first() { /* in-memory db w/ summary + cv doc → out[0].doc_type == "summary" */ }
#[test] fn story_and_cv_duplicates_keep_one() { /* cv chunk == story content → only one excerpt with that text */ }
#[test] fn story_scored_by_title_and_content() { /* query matching title only still ranks the story above an unrelated cv chunk */ }
```
`select_context_excerpts` takes `State<InterviewDb>`; split the body into `fn select_excerpts_inner(conn: &Connection, user_id: &str, query: &str, max_chars: usize) -> Result<Vec<ContextExcerpt>, String>` so tests call it with an in-memory connection.

- [ ] **Step 2: FAIL → implement → PASS**
- [ ] **Step 3: Prompt wording** — manual prompt (`:821-833`) and early prompt (`:1027`): replace "Use the CV/JD snippets below" with: `Use the [SUMMARY], [STORY], [STRENGTH], [CV] and [JD] excerpts below. If a [STORY] fits the question, answer with that story's Situation→Action→Result in first person. Never claim experience that is not in the excerpts.` Keep the rest unchanged.
- [ ] **Step 4: `cargo test` all PASS; clippy/fmt clean**
- [ ] **Step 5: Commit** — `feat(interview): profile summary/stories join lexical excerpt selection`

### Task A4: Profile tab UI

**Files:**
- Modify: `src/index.html` (new tab button after "AI" at `:303`; new `<div class="settings-tab-content" id="tab-profile">`)
- Create: `src/js/profile.js` (mixin `profileMethods`)
- Modify: `src/js/app.js` (`Object.assign(App.prototype, profileMethods)`; call `this._profileInit()` where other `_xxxInit` run)
- Modify: `src/styles/main.css` (`.profile-item`, `.profile-draft-bar`)
- Test: `tests/profile.test.js` (DOM stub pattern from `tests/session-lifecycle.test.js`, `vi.mock('../src/js/ipc.js')`)

**Interfaces:**
- Consumes: `list_profile`, `save_profile_item`, `delete_profile_item`, `draft_profile_from_documents` (A1/A2); `this._getInterviewUserId()` from `interview-panel.js:406`.
- Produces (methods): `_profileInit()`, `async _profileLoad()`, `_profileRender(items)`, `async _profileSaveItem(item)` (debounced 600 ms per item id), `async _profileDraft()`, `_profileAcceptDraft()`, `_profileDiscardDraft()`.
- Produces (pure, exported for tests): `mergeDraft(existing, draft) -> ProfileItem[]` — draft `summary` replaces existing summary; draft stories/strengths whose lowercase `title` already exists are dropped.
- DOM ids: `#profile-summary` (textarea), `#profile-items` (list), `#btn-profile-add-story`, `#btn-profile-add-strength`, `#btn-profile-draft`, `#profile-draft-bar` (hidden; shows "N drafted items — Accept / Discard"), `#profile-status` (text).
- Empty state copy in `#profile-items` when no items: `No stories yet — write one, or draft from your CV.`
- Draft button is always enabled (the backend returns `Upload a CV first (Settings → AI).` when no CV); while drafting, button text `Drafting…` and disabled.

- [ ] **Step 1: Failing tests**

```js
it('mergeDraft replaces summary and dedupes stories by title', ...)
it('render shows empty state when no items', ...)
it('typing in a story saves once after debounce with kind/title/content', ...) // vi.useFakeTimers
it('draft error shows message and saves nothing', ...)                        // invoke rejects → #profile-status has text, save_profile_item not called
```

- [ ] **Step 2: `npm test -- --run tests/profile.test.js` → FAIL**
- [ ] **Step 3: Implement HTML + `profile.js` + CSS**; render each story as `<li class="profile-item" data-id>` with `<input class="profile-title">`, `<textarea class="profile-content">`, `<button class="profile-delete">×</button>`.
- [ ] **Step 4: Tests PASS; `node --check src/js/profile.js src/js/app.js`; full `npm test -- --run` green**
- [ ] **Step 5: Commit** — `feat(ui): Profile tab — summary, STAR stories, strengths, draft-from-CV`

---

## Part B — Post-Session Review

### Task B1: Review builder + parser (pure)

**Files:**
- Create: `src-tauri/src/commands/review.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Test: inline

**Interfaces:**
- Produces:
  ```rust
  #[derive(Serialize, Deserialize)] pub struct ReviewQuestion { pub question: String, pub answer_summary: String, pub score: Option<u8>, pub feedback: String, pub stronger_answer: String }
  #[derive(Serialize, Deserialize)] pub struct SessionReview { pub version: u32 /* 1 */, pub generated_at: String, pub model: String, pub roles_known: bool, pub overall: String, pub questions: Vec<ReviewQuestion>, pub practice: Vec<String> }
  pub fn build_review_dialogue(segments: &serde_json::Value, max_chars: usize) -> (String, bool /* roles_known */, bool /* has_candidate */)
  pub fn parse_review(raw: &str, model: &str, roles_known: bool, has_candidate: bool, now: &str) -> Result<SessionReview, String>
  ```
- `build_review_dialogue` accepts either a bare array or `{ "segments": [...] }` (sidecar v1/v2). Line format: `Interviewer: <original>` for `source == "system"`, `Candidate: <original>` for `source == "mic"`; when no segment has `source`, use `Speaker <speaker>: <original>` and `roles_known = false`. Keeps the **last** `max_chars` characters (cut at a line boundary). `has_candidate` = any `mic` line.
- `parse_review` extracts the first `{…}` object (add `extract_json_object_slice` sibling of `extract_json_array_slice` in `interview.rs`, make both `pub(crate)`); `score` outside 1..=5 → `None`; when `has_candidate == false` all scores are forced `None`.

- [ ] **Step 1: Failing tests**

```rust
#[test] fn dialogue_labels_system_and_mic() { /* "Interviewer: …\nCandidate: …", roles_known true, has_candidate true */ }
#[test] fn dialogue_falls_back_to_speaker_labels() { /* segments w/o source → "Speaker 1: …", roles_known false */ }
#[test] fn dialogue_accepts_versioned_sidecar() { /* {"version":2,"segments":[…]} parses same as bare array */ }
#[test] fn dialogue_truncates_keeping_tail() { /* max_chars 40 → last lines only, starts at a line boundary */ }
#[test] fn no_mic_segments_yields_null_scores_hint() { /* has_candidate false; parse_review(…, has_candidate=false) → all score None */ }
#[test] fn parse_review_tolerates_fences_and_bad_scores() { /* score 7 → None, score 4 → Some(4) */ }
```
- [ ] **Step 2: FAIL → implement → PASS; clippy/fmt**
- [ ] **Step 3: Commit** — `feat(review): dialogue builder + tolerant review parser`

### Task B2: `review_session` command + sidecar cache

**Files:**
- Modify: `src-tauri/src/commands/review.rs`, `src-tauri/src/commands/transcript.rs` (`TranscriptEntry.has_review`, delete also removes `.review.json`, expose `pub(crate) fn transcript_dir`, `pub(crate) fn sidecar_path`)
- Modify: `src-tauri/src/lib.rs` (register)
- Test: inline (temp dir via `tempfile`? — check `Cargo.toml` dev-deps; if absent use `std::env::temp_dir()` + unique suffix as other tests in `transcript.rs` do)

**Interfaces:**
- Produces: `review_session(app, filename: String, force: bool) -> Result<SessionReview, String>`; `read_session_review(app, filename) -> Result<Option<SessionReview>, String>`.
- Cache path: `<transcript_dir>/<name>.review.json` where `<name>` is `filename` minus `.md` (same rule as `.segments.json`). `force == false` and cache exists → return cache without LLM.
- Filename validation identical to `read_transcript` (basename, `.md`).
- `max_chars` for dialogue: **16 000**.
- Prompt (exact contract):
  > You are an interview coach reviewing a finished interview transcript. Return ONLY a JSON object: `{"overall":"<3–5 sentences>","questions":[{"question":"<interviewer question, condensed>","answer_summary":"<what the candidate said, ≤40 words>","score":<1–5 integer>,"feedback":"<what worked / what to fix, ≤60 words>","stronger_answer":"<a better first-person answer, ≤90 words>"}],"practice":["<question to rehearse>", …up to 5]}`. Only include questions actually asked. {roles note}
  - `{roles note}` = `Lines are labelled Interviewer/Candidate.` when `roles_known`, else `Speaker roles are unknown — infer who is interviewing from content and say so in "overall".`
  - When `has_candidate == false`, append: `No candidate audio was captured: set every "score" to null and make "feedback" describe how to approach the question.`
- `list_transcripts` adds `has_review: bool` (file exists).

- [ ] **Step 1: Failing tests**

```rust
#[test] fn review_cache_roundtrip() { /* write SessionReview to temp dir via helper write_review(dir, name, &r); read_review(dir, name) == Some(r) */ }
#[test] fn delete_transcript_removes_review_sidecar() { /* create .md, .segments.json, .review.json → delete → none remain */ }
#[test] fn has_review_reflects_sidecar() { /* list over temp dir */ }
```
Factor the file logic into `fn write_review(dir: &Path, name: &str, r: &SessionReview)` / `fn read_review(dir: &Path, name: &str) -> Result<Option<SessionReview>, String>` so tests avoid `AppHandle`.

- [ ] **Step 2: FAIL → implement → PASS; clippy/fmt**
- [ ] **Step 3: Commit** — `feat(review): review_session command with .review.json cache`

### Task B3: Review UI in read-only mode

**Files:**
- Modify: `src/index.html` — inside `#readonly-banner` add `<button id="btn-review-session" class="action-btn">Review interview</button>`; inside `#right-panel` add `<section id="review-panel" hidden>` with `#review-overall`, `#review-questions`, `#review-practice`, `#btn-review-regenerate`, `#btn-review-copy`, `#review-status`
- Create: `src/js/review.js` (mixin `reviewMethods`)
- Modify: `src/js/conversations.js:25-68` (`_openConversationReadOnly` → after segments load call `this._reviewLoadCached(filename)`; sidebar item gets `<span class="conversation-badge" title="Reviewed">★</span>` when `s.has_review`)
- Modify: `src/js/session.js` (`_createNewSession` / exit read-only → `this._reviewHide()`)
- Modify: `src/js/app.js` (assign mixin), `src/styles/main.css` (`.review-card`, `.review-score`, `.conversation-badge`)
- Test: `tests/review.test.js`

**Interfaces:**
- Consumes: `review_session`, `read_session_review`, `list_transcripts.has_review` (B2).
- Produces: `_reviewLoadCached(filename)`, `async _reviewGenerate(filename, force)`, `_reviewRender(review)`, `_reviewHide()`; pure export `formatScore(score) -> string` (`null → "—"`, `n → "●".repeat(n) + "○".repeat(5-n)`).
- Behaviour: first generation asks `confirm('Review this interview with the LLM? One request, ~N KB of transcript.')` where N = `Math.ceil(chars/1024)` of the loaded segments' originals; cached review renders without confirm; `#btn-review-regenerate` always passes `force: true` and confirms.
- Errors go through `this._showToast(msg, 'error')` (persistent pill) and `#review-status`.
- `#review-panel` visible only while `this.readOnlyMode`; suggestions panel is hidden while review panel is visible (both live in `#right-panel`; toggle `hidden`).

- [ ] **Step 1: Failing tests**

```js
it('formatScore renders dots and em-dash for null', ...)
it('opening a reviewed conversation renders cached review without invoking review_session', ...)
it('Review button on unreviewed session confirms then invokes review_session with force:false', ...)
it('null scores render "—" and a "no candidate audio" note when roles_known && no scores', ...)
it('_reviewHide hides panel and clears content', ...)
```

- [ ] **Step 2: FAIL → implement → PASS; `node --check` touched modules; full vitest green**
- [ ] **Step 3: Commit** — `feat(ui): post-session review panel + reviewed badge in sidebar`

---

## Part C — Auto-updater that ships

### Task C1: Point updater at this repo + single-source version bump

**Files:**
- Modify: `src-tauri/tauri.conf.json:55-57` (endpoint), `.github/workflows/release.yml:302,312` (`REPO`, notes URL)
- Create: `scripts/bump-version.cjs`
- Modify: `package.json` (`"bump": "node scripts/bump-version.cjs"`)
- Test: `tests/bump-version.test.js`

**Interfaces:**
- Endpoint: `https://github.com/dainn-dev/assistant/releases/latest/download/latest.json`.
- `node scripts/bump-version.cjs 1.1.0` rewrites `"version"` in `package.json`, `src-tauri/tauri.conf.json`, and `^version = "…"` in `src-tauri/Cargo.toml`, then runs `cargo update -p myjavis --manifest-path src-tauri/Cargo.toml` to sync `Cargo.lock`. Rejects non-`\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?`. Pure export `applyVersion(files: {pkg, conf, cargo}: string, v) -> {pkg, conf, cargo}` for tests.

- [ ] **Step 1: Failing test** — `applyVersion` changes exactly the three version fields, leaves other content byte-identical, throws on `"1.0"`.
- [ ] **Step 2: FAIL → implement → PASS**
- [ ] **Step 3: Edit endpoint + `REPO` + notes URL**; `grep -rn "phuc-nt" . --exclude-dir=node_modules --exclude-dir=target` → no matches.
- [ ] **Step 4: Commit** — `fix(updater): endpoint targets dainn-dev/assistant; add version bump script`

### Task C2: Signing key procedure (manual, documented)

**Files:**
- Create: `docs/release.md`
- Modify: `src-tauri/tauri.conf.json:54` (`pubkey` — only after the user generates a key)
- Modify: `.github/workflows/release.yml` (every `TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ""` → `${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}`)

**Interfaces:**
- The private key for the current `pubkey` is not available in this repo/machine. Rotate: new keypair, new pubkey in config. Old installs (1.0.0 built with the old pubkey) **cannot** verify the new key — they must install the next version manually once; document this.

- [ ] **Step 1: Write `docs/release.md`** with exactly these steps: (1) `npm run tauri signer generate -- -w ~/.tauri/myjavis.key` (prompt for a password; store both in a password manager); (2) GitHub → Settings → Secrets → `TAURI_SIGNING_PRIVATE_KEY` = file contents, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = password; (3) paste the printed public key into `tauri.conf.json` `plugins.updater.pubkey`; (4) release flow: `npm run bump X.Y.Z` → commit → `git tag vX.Y.Z` → `git push --tags` → wait for *Build & Release* → open the **draft** release, check `latest.json` lists `windows-x86_64` with a non-empty signature → **Publish** (the `releases/latest/download` endpoint ignores drafts); (5) local signed build: `TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/myjavis.key)" TAURI_SIGNING_PRIVATE_KEY_PASSWORD=… npm run tauri build`.
- [ ] **Step 2: Workflow password secret edit**
- [ ] **Step 3: STOP — ask the user to run steps 1–3 and paste the new pubkey; then edit `tauri.conf.json`.** Verify with `npm run tauri build` locally using the key: output ends without the "no private key" error and `bundle/nsis/*.exe.sig` exists.
- [ ] **Step 4: Commit** — `chore(release): rotate updater pubkey; document signing + release procedure`

### Task C3: Release workflow that succeeds with Windows alone

**Files:**
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- `build-macos`, `build-macos-intel`, `build-android`: add `continue-on-error: true` at job level.
- `release`: `needs: [build-windows, build-macos, build-macos-intel, build-android]`, `if: ${{ !cancelled() && needs.build-windows.result == 'success' }}`; the three optional `download-artifact` steps get `continue-on-error: true`.
- `latest.json` generation: emit a platform entry **only** when its file and signature are both non-empty (bash: build `PLATFORMS` fragments, join with commas) so a Windows-only release yields `"platforms": {"windows-x86_64": {...}}` and valid JSON.
- Unify `npm install` → `npm ci` in all jobs.
- Add a first job `gates` (ubuntu): `npm ci && npm test -- --run`; `build-windows` `needs: gates`.

- [ ] **Step 1: Edit workflow**; the only reliable validation is the dry run in step 2 (no YAML linter is a project dep).
- [ ] **Step 2: Dry run** — `npm run bump 1.0.1-rc.1`, commit, `git tag v1.0.1-rc.1 && git push origin v1.0.1-rc.1`. Expected: Windows job green, macOS/Android jobs red-but-continue, `release` job creates a **draft** with `latest.json` containing only `windows-x86_64`, `.exe`, `.msi`, `.sig`. Then delete the draft and tag (`gh release delete v1.0.1-rc.1 --yes && git push --delete origin v1.0.1-rc.1`), revert the bump commit.
- [ ] **Step 3: Commit** — `ci(release): Windows-only releases succeed; gate on tests; latest.json lists only built platforms`

### Task C4: Updater UX hardening

**Files:**
- Modify: `src/js/updater.js` (drop the debug `Object.entries(window.__TAURI__)` dump at `:33-39`; errors → `onError` only), `src/js/updater-ui.js` (`onError` → also `this._showToast(\`Update check failed: ${msg}\`, 'error')` **only** for manual checks, not the startup check), `src/js/settings-form.js:568-585` (after `downloadAndInstall` resolves, if `relaunch` missing show `_showToast('Installed — restart MyJavis to finish updating', 'success')`)
- Modify: `src/index.html` `#update-status-text` initial copy → `Checks on startup · click to re-check`
- Test: `tests/updater-ui.test.js` — startup check failure sets status text but does not call `_showToast`; manual check failure calls `_showToast(…, 'error')`.

- [ ] **Step 1: Failing tests → FAIL → implement → PASS**
- [ ] **Step 2: Commit** — `feat(updater): quieter startup check, surfaced manual-check errors`

### Task C5: End-to-end update verification

- [ ] **Step 1:** Install the current `1.0.0` NSIS build from `src-tauri/target/release/bundle/nsis/` on this machine (it carries the **new** pubkey only if rebuilt after C2 — rebuild first).
- [ ] **Step 2:** `npm run bump 1.0.1` → commit → tag → push → publish the draft release.
- [ ] **Step 3:** Launch installed 1.0.0 → Settings → About shows `🆕 Update v1.0.1 available` within ~5 s → Install → app relaunches → About shows `v1.0.1`.
- [ ] **Step 4:** Record outcome in `docs/release.md` under "Verified: <date>".

---

## Execution order & dependencies

```
A1 → A2 → A3 → A4        (Profile; A3 depends on A1 only)
B1 → B2 → B3             (Review; independent of A)
C1 → C2(user) → C3 → C4 → C5
```
Parts are independent; recommended order **C1, C3 (dry run can run in parallel with A) → A → B → C2/C4/C5** so the updater pipe is proven before the first feature release ships through it.

## Out of scope (deliberate)

- Grounding `stronger_answer` in the Profile/CV (one more coupling; revisit after B ships and the review quality is observed).
- Speaker-role assignment for same-room interviews.
- macOS/Android signing secrets — jobs stay optional until certificates exist.
- Scheduler constant tuning (needs metrics from `SessionMetrics` first).
