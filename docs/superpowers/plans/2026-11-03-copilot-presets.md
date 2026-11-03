# Copilot Presets — Plan cho mục 8 (định vị Interview mode)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Date:** 2026-11-03 · **Base:** `main` @ `34baff2` (v1.1.0)
**Status:** Phase 0 đã chốt (2026-11-03): Q1 = B (preset), Q2 = Meeting, Q3 = đổi tên UI thành "Copilot" + dropdown preset, Q4 = hoãn (không trả lời → theo đề xuất). Phase 1 + 2 xong; Phase 3: đổi tên UI + tách `interview.rs` xong, còn cập nhật README.

**Lệch so với plan khi làm:**
- Prompt draft profile **không** đi qua preset (Meeting không có Profile).
- Thêm cờ `hints_may_be_silent` + `SilenceFilter`: gợi ý sớm của Meeting trả `NONE` khi không ai hỏi mình → backend phát sự kiện `skip`, token không bao giờ hiện lên.
- Đổi preset sẽ bỏ chọn file đang chọn (`pickedUploadsSurvive`) để CV không bị ingest thành Brief.

**Goal:** Biến Interview mode từ một tính năng gắn cứng thành **một preset** của chế độ "Copilot" chung. Interview giữ nguyên hành vi; thêm preset thứ hai (Meeting) dùng lại toàn bộ pipeline hiện có.

**Architecture:** Mỗi preset là dữ liệu, không phải nhánh code: nhãn vai người nói, nhãn tài liệu, 4 prompt (gợi ý thủ công, gợi ý sớm, review, draft profile), schema review và vài cờ tính năng. Rust giữ prompt (một nguồn sự thật, test được); JS chỉ giữ nhãn UI và cờ tính năng.

**Tech Stack:** Tauri v2 (Rust: rusqlite, reqwest, serde), vanilla JS mixin trên `App.prototype`, Vitest.

---

## Bối cảnh: Interview đang gắn cứng ở đâu

Phần **đã chung chung** (không cần đổi):

| Thành phần | File | Ghi chú |
|---|---|---|
| Tách 2 luồng nhận dạng system/mic | `session.js` `_startSplitCapture` | Vai = nguồn âm thanh, không phụ thuộc phỏng vấn |
| Excerpt cục bộ (lexical + semantic) | `interview.rs` `select_context_excerpts`, `semantic_excerpts` | Chạy trên bảng `documents` / `doc_chunks` / `profile_items` |
| Gợi ý stream + huỷ | `suggest_interview_answers_stream`, `early-suggestions.js` | Scheduler, metrics không biết gì về phỏng vấn |
| Review sau phiên | `review.rs` | Đọc sidecar `.segments.json` (source `system`/`mic`) |

Phần **gắn cứng với phỏng vấn** (đây là toàn bộ phạm vi refactor):

| Chỗ | File:dòng (v1.1.0) |
|---|---|
| Prompt gợi ý thủ công: "You are the candidate — a senior software engineer" | `interview.rs` ~786 |
| Prompt gợi ý sớm: "coaching a candidate during a live interview" | `interview.rs` ~1085 |
| Prompt review + schema `questions[].score` | `review.rs` ~111–136, `parse_review` |
| Nhãn vai `Interviewer` / `Candidate` khi dựng dialogue | `review.rs` ~196 |
| Prompt draft profile (summary / STAR story / strength) | `profile.rs` ~147 |
| Nhãn tài liệu `cv` / `jd`, nhãn excerpt `[CV]`/`[JD]` | `interview.rs`, `interview-panel.js` uploads |
| ~15 phép so sánh `currentTemplate === 'Interview'` | `interview-panel.js`, `profile.js`, `settings-form.js` |
| `app_mode` chỉ chấp nhận `'Interview'` | `settings-form.js:390`, `index.html` `#select-app-mode` |

---

## Phase 0 — Chốt quyết định (0.5 ngày, cần bạn)

Ba lựa chọn:

| | A. Giữ là tính năng phụ | **B. Preset trong cùng app (đề xuất)** | C. Tách thành app riêng |
|---|---|---|---|
| Công sức | ~0 | ~5–7 ngày cho Phase 1–2 | Vài tuần + 2 pipeline release |
| Rủi ro | Interview tiếp tục phình trong `interview.rs` (1355 dòng) | Hồi quy prompt Interview → chặn bằng golden test | Nhân đôi capture/updater/signing |
| Giá trị | Không đổi | Cùng pipeline phục vụ họp, bán hàng… | Định vị rõ, nhưng mất lợi thế "1 app" |

**Đề xuất B**, vì 80% cỗ máy đã chung chung; chi phí chủ yếu là di chuyển prompt và thay các phép so sánh chuỗi bằng cờ tính năng.

Cần bạn chốt:

- [ ] **Q1.** Chọn B? (nếu A → dừng plan ở đây, chỉ làm task tách file ở Phase 3)
- [ ] **Q2.** Preset thứ hai: **Meeting** (đề xuất — phạm vi rộng nhất, dùng được ngay với chế độ System+Mic) hay **Sales**?
- [ ] **Q3.** Đổi tên UI "Interview" → "Copilot" với dropdown preset (Phase 3), hay giữ "Interview" làm tên chế độ và chỉ thêm preset?
- [ ] **Q4.** Preset tuỳ chỉnh (người dùng tự sửa prompt) có nằm trong phạm vi không? Đề xuất: **để sau**, chỉ làm khi Meeting đã được dùng thật.

**Tiêu chí thành công** (app không có telemetry, nên đo trên máy bạn + phản hồi người dùng):

- Interview sau refactor cho ra **prompt giống hệt byte** (golden test) → không hồi quy.
- Một buổi họp thật dùng preset Meeting cho ra review có ≥1 quyết định / action item đúng, và gợi ý sớm không bắn vô ích nhiều hơn Interview (so `metrics` trong sidecar: tỉ lệ hint bị huỷ).

---

## Phase 1 — Trừu tượng hoá preset, không đổi hành vi (~2 ngày)

Mục tiêu: Interview chạy y hệt v1.1.0, nhưng mọi chỗ gắn cứng đi qua preset.

### 1.1 Rust: `CopilotPreset`

Files: tạo `src-tauri/src/copilot/mod.rs`, `src-tauri/src/copilot/presets.rs`; sửa `interview.rs`, `review.rs`, `profile.rs`.

- [ ] **Golden test trước (RED → giữ GREEN suốt Phase 1):** chụp 4 prompt hiện tại của Interview với input cố định (`build_suggest_prompt`, `build_early_prompt`, `build_review_prompt`, `build_profile_prompt` — tách hàm dựng prompt ra khỏi các command nếu chưa tách) và assert bằng chuỗi literal.
- [ ] Định nghĩa:
  ```rust
  pub struct CopilotPreset {
      pub id: &'static str,                 // "interview"
      pub remote_label: &'static str,       // "Interviewer" — nguồn system
      pub self_label: &'static str,         // "Candidate"   — nguồn mic
      pub doc_slots: [DocSlot; 2],          // [cv, jd]: doc_type + nhãn excerpt
      pub has_profile: bool,
      pub review: ReviewKind,               // Scored (interview) | Minutes (meeting)
  }
  pub fn preset(id: &str) -> &'static CopilotPreset  // id lạ → interview
  ```
  Prompt là hàm `fn(&PromptInputs, &CopilotPreset) -> String` theo preset, không phải template chuỗi — giữ được kiểm tra của compiler và golden test.
- [ ] Các command đọc preset từ `Settings.app_mode` (một nguồn sự thật; không thêm field vào request). `"Interview"` (giá trị đang lưu ở máy người dùng) map sang `interview`.
- [ ] Di chuyển nguyên văn 4 prompt vào preset `interview`. Golden test phải vẫn xanh.
- [ ] `build_review_dialogue` nhận nhãn vai từ preset thay vì hằng số.

### 1.2 JS: cờ tính năng thay cho so sánh chuỗi

Files: tạo `src/js/copilot-presets.js`; sửa `interview-panel.js`, `profile.js`, `settings-form.js`.

- [ ] Test trước: `copilotPreset('Interview')` trả về `{ id: 'interview', title, uploads: [{docType:'cv',label:'CV'},…], hasProfile: true }`; `copilotPreset('')` → `null`.
- [ ] Thêm `this._copilot()` (preset hiện tại hoặc `null`). Thay ~15 chỗ `currentTemplate === 'Interview'` bằng `this._copilot()` / `this._copilot()?.hasProfile`. `_isSuggestionsMode()` → `!!this._copilot()`.
- [ ] Nhãn nút upload, tiêu đề panel gợi ý lấy từ preset.

**Acceptance Phase 1:** `cargo test` + `npm test` xanh; golden test chứng minh prompt Interview không đổi; chạy thử 1 phiên Interview thấy y hệt v1.1.0. Có thể release riêng (patch).

---

## Phase 2 — Preset Meeting (~2–3 ngày)

### 2.1 Định nghĩa preset

| | Interview | Meeting |
|---|---|---|
| Nhãn vai (system / mic) | Interviewer / Candidate | Others / Me |
| Tài liệu (2 slot) | CV / JD | Brief / Agenda |
| Gợi ý thủ công | Câu trả lời ngôi thứ nhất ≤80 từ | "Nên nói gì tiếp": trả lời câu hỏi đang hướng tới mình, hoặc 1 câu hỏi làm rõ |
| Gợi ý sớm | 2–3 talking point cho câu hỏi đang nghe | 2–3 point khi bị hỏi; im lặng nếu không ai hỏi mình (giảm hint vô ích) |
| Profile | Có (summary / STAR / strength) | Không (`has_profile: false`) |
| Review | Chấm điểm từng câu hỏi | Biên bản: tóm tắt, quyết định, action item (người, hạn), câu hỏi còn mở |

### 2.2 Tasks

- [ ] Test trước cho prompt Meeting (chứa nhãn vai Others/Me, nhãn `[BRIEF]`/`[AGENDA]`, không nhắc "candidate").
- [ ] `ReviewKind::Minutes`: schema JSON mới + `parse_minutes` (tolerant như `parse_review`), test với output LLM mẫu có fence/thừa chữ.
- [ ] Cache review: thêm field `kind` vào `<name>.review.json`; file cũ không có `kind` → coi là `scored` (test).
- [ ] Frontend `review.js`: render theo `kind` (Minutes = các danh sách, không có điểm).
- [ ] `index.html` `#select-app-mode`: thêm option `Meeting`; `settings-form.js:390` chấp nhận các id preset hợp lệ thay vì chỉ `'Interview'`.
- [ ] Ẩn tab Profile khi `!hasProfile`.

**Acceptance Phase 2:** 1 buổi họp thật (System+Mic) → panel gợi ý dùng được, review ra biên bản có action item; Interview không đổi (golden test).

---

## Phase 3 — Đặt tên & dọn dẹp (~1 ngày, tuỳ Q3)

- [ ] (Nếu Q3 = đổi tên) UI: "Session Mode" → "Copilot" + dropdown preset; class `interview-active` → `copilot-active` (CSS + `mobile.css`).
- [x] Tách `interview.rs` (1355 dòng) — thực tế: thư mục `commands/interview/{documents,ingest,suggest,excerpts,stream}.rs` (giữ đường dẫn `commands::interview::…`) + `commands/util.rs` cho `run_blocking`/`http_client`/`extract_json_*`. Ban đầu dự kiến đặt trong `copilot/` — giải quyết luôn mục nợ kỹ thuật "file quá 800 dòng". Tên command Tauri **giữ nguyên** (frontend không phải đổi `invoke`).
- [ ] Cập nhật `CLAUDE.md` (mục Interview/RAG) và README (tính năng).

Không cần migration dữ liệu: bảng không đổi; `doc_type` mới (`brief`, `agenda`) chỉ là giá trị mới trong cột cũ.

---

## Phase 4 — Preset tuỳ chỉnh (hoãn, chỉ khi Q4 = có)

Ghi lại để không làm sớm:

- Cho sửa **phần persona / chỉ dẫn** của prompt, **không** cho sửa phần định dạng trả về (JSON / "- " bullet) — parser phụ thuộc vào đó.
- Lưu trong `settings.json` (`custom_presets: Vec<…>`), giới hạn độ dài, có nút "reset về mặc định".

---

## Rủi ro & cách chặn

| Rủi ro | Chặn bằng |
|---|---|
| Prompt Interview bị đổi ngầm khi refactor | Golden test viết **trước** khi di chuyển (Phase 1.1) |
| `.review.json` cũ không đọc được | `kind` thiếu → `scored`; test với file mẫu cũ |
| Gợi ý sớm bắn quá nhiều trong họp (nhiều người nói, ít câu hỏi) | Prompt Meeting cho phép trả rỗng; so tỉ lệ hint bị huỷ trong `metrics` sidecar giữa 2 preset trước khi chỉnh ngưỡng scheduler |
| Phạm vi trôi sang Sales/Custom | Q2/Q4 chốt ở Phase 0; mỗi preset mới là một plan riêng |

## Ước lượng tổng

Phase 0: 0.5 ngày · Phase 1: ~2 ngày · Phase 2: 2–3 ngày · Phase 3: ~1 ngày → **khoảng 6 ngày** cho B với Meeting, chưa gồm Phase 4.
