use crate::db::{self, InterviewDb};
use crate::secrets::{self, SecretSlot};
use crate::services::embeddings;
use crate::services::llm;
use crate::services::pinecone::{pinecone_vector_from_parts, query_top_k, upsert_vectors};
use crate::settings::SettingsState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

const INGEST_CHUNK_MAX_CHARS: usize = 1200;
const INGEST_CHUNK_OVERLAP_CHARS: usize = 150;
/// Upper bound on a single upload — keeps extractor memory bounded and
/// rejects accidental multi-hundred-MB drops.
const INGEST_FILE_MAX_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterviewFilePart {
    pub filename: String,
    #[serde(default)]
    pub bytes: Vec<u8>,
    /// Optional filesystem path. When set, the backend reads the file itself
    /// so multi-MB PDFs/DOCX never cross the IPC bridge as JSON byte arrays.
    #[serde(default)]
    pub path: Option<String>,
}

impl InterviewFilePart {
    /// Materialize `bytes` from `path` if needed, enforcing the size cap.
    fn resolve(mut self) -> Result<Self, String> {
        if let Some(path) = self.path.take() {
            let meta = std::fs::metadata(&path)
                .map_err(|e| format!("Cannot stat '{}': {e}", self.filename))?;
            if meta.len() > INGEST_FILE_MAX_BYTES {
                return Err(format!(
                    "'{}' is too large ({} MB, max {} MB)",
                    self.filename,
                    meta.len() / (1024 * 1024),
                    INGEST_FILE_MAX_BYTES / (1024 * 1024)
                ));
            }
            self.bytes = std::fs::read(&path)
                .map_err(|e| format!("Cannot read '{}': {e}", self.filename))?;
        }
        if self.bytes.len() as u64 > INGEST_FILE_MAX_BYTES {
            return Err(format!(
                "'{}' is too large (max {} MB)",
                self.filename,
                INGEST_FILE_MAX_BYTES / (1024 * 1024)
            ));
        }
        if self.bytes.is_empty() {
            return Err(format!("'{}' is empty", self.filename));
        }
        Ok(self)
    }
}

/// Pinecone namespaces derive from `user_id` — keep it a sane identifier
/// (non-empty, bounded, no control/whitespace chars that could break URLs
/// or collide across users).
fn sanitize_user_id(user_id: &str) -> Result<String, String> {
    let trimmed = user_id.trim();
    if trimmed.is_empty() {
        return Err("user_id is empty".to_string());
    }
    if trimmed.chars().count() > 128 {
        return Err("user_id is too long".to_string());
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("user_id contains invalid characters".to_string());
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestInterviewFilesRequest {
    pub user_id: String,
    pub cv: Option<InterviewFilePart>,
    pub jd: Option<InterviewFilePart>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestInterviewFilesResponse {
    pub ok: bool,
    pub cv_doc_id: Option<String>,
    pub jd_doc_id: Option<String>,
    pub cv_chunks: usize,
    pub jd_chunks: usize,
    pub message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveInterviewMessageRequest {
    pub user_id: String,
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestInterviewAnswersRequest {
    pub user_id: String,
    #[serde(default)]
    pub transcript_context: Option<String>,
    #[serde(default)]
    pub user_draft: Option<String>,
    #[serde(default)]
    pub debug: Option<bool>,
    #[serde(default)]
    pub app_mode: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InterviewSuggestionItem {
    pub id: u32,
    pub target: String,
    pub translation: String,
    pub suggestion_kind: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestInterviewAnswersResponse {
    pub suggestions: Vec<InterviewSuggestionItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug: Option<String>,
}

fn extract_docx(data: &[u8]) -> Result<String, String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;
    use std::io::{Cursor, Read};
    use zip::ZipArchive;

    let cur = Cursor::new(data);
    let mut archive = ZipArchive::new(cur).map_err(|e| format!("docx zip: {e}"))?;
    let mut file = archive
        .by_name("word/document.xml")
        .map_err(|e| format!("docx missing document.xml: {e}"))?;
    let mut xml = String::new();
    file.read_to_string(&mut xml)
        .map_err(|e| format!("docx read: {e}"))?;

    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut out = String::new();
    let mut wt_depth = 0i32;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                if e.name().as_ref() == b"w:t" {
                    wt_depth += 1;
                }
            }
            Ok(Event::End(ref e)) => {
                if e.name().as_ref() == b"w:t" {
                    wt_depth = (wt_depth - 1).max(0);
                }
            }
            Ok(Event::Text(ref t)) => {
                if wt_depth > 0 {
                    let s = t.unescape().map_err(|e| e.to_string())?;
                    out.push_str(&s);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(e.to_string()),
            Ok(_) => {}
        }
        buf.clear();
    }

    Ok(out)
}

fn extract_pdf(data: &[u8]) -> Result<String, String> {
    pdf_extract::extract_text_from_mem(data).map_err(|e| format!("pdf: {e}"))
}

fn extract_file_text(filename: &str, bytes: &[u8]) -> Result<String, String> {
    let lower = filename.to_lowercase();
    let text = if lower.ends_with(".pdf") {
        extract_pdf(bytes)?
    } else if lower.ends_with(".docx") {
        extract_docx(bytes)?
    } else {
        return Err(format!("Unsupported file type: {filename}"));
    };
    Ok(text.trim().chars().take(200_000).collect::<String>())
}

/// Word-boundary chunks up to `max_chars` (byte length of joined words, matching OpenAI token-ish limits for ASCII-heavy CVs).
/// After each chunk except the last, the next chunk starts with a suffix of the previous chunk (up to `overlap` **characters**, word-aligned).
fn chunk_text_with_overlap(text: &str, max_chars: usize, overlap: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    if max_chars == 0 {
        return Vec::new();
    }
    let overlap = overlap.min(max_chars.saturating_sub(1));

    fn overlap_suffix(s: &str, max_overlap_chars: usize) -> &str {
        if max_overlap_chars == 0 || s.is_empty() {
            return "";
        }
        let char_count = s.chars().count();
        if char_count <= max_overlap_chars {
            return s;
        }
        let skip = char_count - max_overlap_chars;
        let start_byte = s
            .char_indices()
            .nth(skip)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        let tail = &s[start_byte..];
        match tail.find(' ') {
            Some(pos) if pos + 1 < tail.len() => &tail[pos + 1..],
            _ => tail,
        }
    }

    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out = Vec::new();
    let mut word_idx = 0usize;
    let mut pending: Option<String> = None;

    while word_idx < words.len() || pending.is_some() {
        let mut cur = pending.take().unwrap_or_default();
        while word_idx < words.len() {
            let w = words[word_idx];
            let add = if cur.is_empty() { w.len() } else { w.len() + 1 };
            if cur.len() + add > max_chars && !cur.is_empty() {
                break;
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(w);
            word_idx += 1;
        }
        if cur.is_empty() && word_idx < words.len() {
            cur.push_str(words[word_idx]);
            word_idx += 1;
        }
        let chunk = cur.trim().to_string();
        if chunk.is_empty() {
            break;
        }
        out.push(chunk.clone());
        if word_idx >= words.len() {
            break;
        }
        let suf = overlap_suffix(&chunk, overlap);
        if !suf.trim().is_empty() {
            pending = Some(suf.to_string());
        }
    }
    out
}

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())
}

fn extract_json_array_slice(text: &str) -> Option<&str> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end <= start {
        return None;
    }
    Some(&text[start..=end])
}

fn line_fallback_suggestion_strings(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim().trim_start_matches(['-', '•']).trim().to_string())
        .filter(|l| !l.is_empty())
        .take(6)
        .collect()
}

#[derive(Deserialize)]
struct SuggestionBothRow {
    target: String,
    translation: String,
}

fn normalized_suggestion_type(raw: &str) -> &str {
    match raw {
        "target" | "translation" | "both" => raw,
        _ => "translation",
    }
}

fn parse_suggestions_from_llm(raw: &str, suggestion_type: &str) -> Vec<InterviewSuggestionItem> {
    let st = normalized_suggestion_type(suggestion_type);
    let Some(slice) = extract_json_array_slice(raw) else {
        return match st {
            "both" => line_fallback_suggestion_strings(raw)
                .into_iter()
                .enumerate()
                .map(|(i, s)| InterviewSuggestionItem {
                    id: i as u32,
                    target: s.clone(),
                    translation: s,
                    suggestion_kind: "answer".to_string(),
                })
                .collect(),
            "target" => line_fallback_suggestion_strings(raw)
                .into_iter()
                .enumerate()
                .map(|(i, s)| InterviewSuggestionItem {
                    id: i as u32,
                    target: s,
                    translation: String::new(),
                    suggestion_kind: "answer".to_string(),
                })
                .collect(),
            _ => line_fallback_suggestion_strings(raw)
                .into_iter()
                .enumerate()
                .map(|(i, s)| InterviewSuggestionItem {
                    id: i as u32,
                    target: String::new(),
                    translation: s,
                    suggestion_kind: "answer".to_string(),
                })
                .collect(),
        };
    };

    match st {
        "both" => serde_json::from_slice::<Vec<SuggestionBothRow>>(slice.as_bytes())
            .map(|rows| {
                rows.into_iter()
                    .enumerate()
                    .map(|(i, r)| InterviewSuggestionItem {
                        id: i as u32,
                        target: r.target,
                        translation: r.translation,
                        suggestion_kind: "answer".to_string(),
                    })
                    .collect()
            })
            .unwrap_or_else(|_| {
                serde_json::from_slice::<Vec<String>>(slice.as_bytes())
                    .unwrap_or_else(|_| line_fallback_suggestion_strings(raw))
                    .into_iter()
                    .enumerate()
                    .map(|(i, s)| InterviewSuggestionItem {
                        id: i as u32,
                        target: s.clone(),
                        translation: s,
                        suggestion_kind: "answer".to_string(),
                    })
                    .collect()
            }),
        "target" => serde_json::from_slice::<Vec<String>>(slice.as_bytes())
            .unwrap_or_else(|_| line_fallback_suggestion_strings(raw))
            .into_iter()
            .enumerate()
            .map(|(i, s)| InterviewSuggestionItem {
                id: i as u32,
                target: s,
                translation: String::new(),
                suggestion_kind: "answer".to_string(),
            })
            .collect(),
        _ => serde_json::from_slice::<Vec<String>>(slice.as_bytes())
            .unwrap_or_else(|_| line_fallback_suggestion_strings(raw))
            .into_iter()
            .enumerate()
            .map(|(i, s)| InterviewSuggestionItem {
                id: i as u32,
                target: String::new(),
                translation: s,
                suggestion_kind: "answer".to_string(),
            })
            .collect(),
    }
}

fn count_speaker_lines(transcript: &str) -> usize {
    transcript.lines().filter(|l| !l.trim().is_empty()).count()
}

#[derive(Deserialize)]
struct MeetingSuggestionBothRow {
    target: String,
    translation: String,
    #[serde(default)]
    suggestion_kind: String,
}

#[derive(Deserialize)]
struct MeetingSuggestionSingleRow {
    text: String,
    #[serde(default)]
    suggestion_kind: String,
}

fn parse_meeting_suggestions_from_llm(
    raw: &str,
    suggestion_type: &str,
) -> Vec<InterviewSuggestionItem> {
    let st = normalized_suggestion_type(suggestion_type);
    let default_kind = "talking_point";

    let Some(slice) = extract_json_array_slice(raw) else {
        return line_fallback_suggestion_strings(raw)
            .into_iter()
            .enumerate()
            .map(|(i, s)| match st {
                "both" => InterviewSuggestionItem {
                    id: i as u32,
                    target: s.clone(),
                    translation: s,
                    suggestion_kind: default_kind.to_string(),
                },
                "target" => InterviewSuggestionItem {
                    id: i as u32,
                    target: s,
                    translation: String::new(),
                    suggestion_kind: default_kind.to_string(),
                },
                _ => InterviewSuggestionItem {
                    id: i as u32,
                    target: String::new(),
                    translation: s,
                    suggestion_kind: default_kind.to_string(),
                },
            })
            .collect();
    };

    match st {
        "both" => serde_json::from_slice::<Vec<MeetingSuggestionBothRow>>(slice.as_bytes())
            .map(|rows| {
                rows.into_iter()
                    .enumerate()
                    .map(|(i, r)| InterviewSuggestionItem {
                        id: i as u32,
                        target: r.target,
                        translation: r.translation,
                        suggestion_kind: if r.suggestion_kind.is_empty() {
                            default_kind.to_string()
                        } else {
                            r.suggestion_kind
                        },
                    })
                    .collect()
            })
            .unwrap_or_else(|_| {
                line_fallback_suggestion_strings(raw)
                    .into_iter()
                    .enumerate()
                    .map(|(i, s)| InterviewSuggestionItem {
                        id: i as u32,
                        target: s.clone(),
                        translation: s,
                        suggestion_kind: default_kind.to_string(),
                    })
                    .collect()
            }),
        _ => serde_json::from_slice::<Vec<MeetingSuggestionSingleRow>>(slice.as_bytes())
            .map(|rows| {
                rows.into_iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let (target, translation) = if st == "target" {
                            (r.text, String::new())
                        } else {
                            (String::new(), r.text)
                        };
                        InterviewSuggestionItem {
                            id: i as u32,
                            target,
                            translation,
                            suggestion_kind: if r.suggestion_kind.is_empty() {
                                default_kind.to_string()
                            } else {
                                r.suggestion_kind
                            },
                        }
                    })
                    .collect()
            })
            .unwrap_or_else(|_| {
                line_fallback_suggestion_strings(raw)
                    .into_iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let (target, translation) = if st == "target" {
                            (s, String::new())
                        } else {
                            (String::new(), s)
                        };
                        InterviewSuggestionItem {
                            id: i as u32,
                            target,
                            translation,
                            suggestion_kind: default_kind.to_string(),
                        }
                    })
                    .collect()
            }),
    }
}

fn emit_progress(app: &AppHandle, doc_type: &str, stage: &str, current: usize, total: usize) {
    let _ = app.emit(
        "ingest:progress",
        serde_json::json!({
            "docType": doc_type,
            "stage": stage,
            "current": current,
            "total": total,
        }),
    );
}

/// Shared dependencies for document ingestion — bundling keeps
/// `ingest_one_doc` readable and under the clippy arg limit.
struct IngestCtx<'a> {
    conn: &'a rusqlite::Connection,
    client: &'a reqwest::blocking::Client,
    pinecone_host: &'a str,
    pine_key: &'a str,
    embeddings_url: &'a str,
    embeddings_key: &'a str,
    user_id: &'a str,
}

fn ingest_one_doc(
    app: &AppHandle,
    ctx: &IngestCtx<'_>,
    doc_type: &str,
    part: InterviewFilePart,
    expected_dim: usize,
) -> Result<(String, usize), String> {
    emit_progress(app, doc_type, "extracting", 0, 1);
    let text = extract_file_text(&part.filename, &part.bytes)?;
    let doc_id = Uuid::new_v4().to_string();
    db::upsert_document(
        ctx.conn,
        &doc_id,
        ctx.user_id,
        doc_type,
        &part.filename,
        Some(&part.bytes),
        &text,
    )?;

    fn normalize_whitespace_key(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for w in s.split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(w);
        }
        out
    }

    let chunks = chunk_text_with_overlap(&text, INGEST_CHUNK_MAX_CHARS, INGEST_CHUNK_OVERLAP_CHARS);

    // Dedupe chunks BEFORE embedding and Pinecone upsert to avoid storing duplicates.
    // Key: trim + normalize whitespace only (no lowercasing) to avoid accidental semantic merges.
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut unique_chunks: Vec<(usize, String)> = Vec::new(); // (original index, chunk text)
    for (i, ch) in chunks.into_iter().enumerate() {
        let key = normalize_whitespace_key(&ch);
        if seen.contains_key(&key) {
            continue;
        }
        seen.insert(key, i);
        unique_chunks.push((i, ch));
    }

    let total_chunks = unique_chunks.len();
    emit_progress(app, doc_type, "embedding", 0, total_chunks);
    let mut all_vectors = Vec::with_capacity(total_chunks);
    let max = embeddings::OPENAI_EMBEDDINGS_MAX_INPUTS;
    for batch_start in (0..unique_chunks.len()).step_by(max) {
        let batch_end = (batch_start + max).min(unique_chunks.len());
        let batch: Vec<String> = unique_chunks[batch_start..batch_end]
            .iter()
            .map(|(_, ch)| ch.clone())
            .collect();
        let (vecs, dim) = embeddings::embed_batch_prefer_openai(
            ctx.client,
            ctx.embeddings_url,
            ctx.embeddings_key,
            &batch,
            Some(expected_dim),
        )?;
        emit_progress(app, doc_type, "embedding", batch_end, total_chunks);
        if dim != expected_dim {
            return Err(format!(
                "Embedding dimension is {dim} but Settings expects {expected_dim}. Set pinecone_vector_dimension to {dim} or use matching embeddings (1536 OpenAI / 768 Gemini)."
            ));
        }
        if vecs.len() != batch.len() {
            return Err(format!(
                "Batch embedding returned {} vectors for {} chunks — API response mismatch.",
                vecs.len(),
                batch.len()
            ));
        }
        for (j, v) in vecs.into_iter().enumerate() {
            let idx = batch_start + j;
            let (orig_i, ch) = &unique_chunks[idx];
            let ref_id = format!("{doc_id}#{orig_i}");
            let pid = format!("{}_{doc_id}_{orig_i}", ctx.user_id);
            all_vectors.push(pinecone_vector_from_parts(
                pid,
                v,
                ctx.user_id,
                "doc_chunk",
                &ref_id,
                Some(doc_type),
                ch,
            ));
        }
    }
    emit_progress(app, doc_type, "upserting", 0, 1);
    upsert_vectors(
        ctx.client,
        ctx.pinecone_host,
        ctx.pine_key,
        Some(ctx.user_id),
        all_vectors,
    )?;
    emit_progress(app, doc_type, "done", total_chunks, total_chunks);
    Ok((doc_id, total_chunks))
}

#[tauri::command]
pub fn ingest_interview_files(
    app: AppHandle,
    db: State<'_, InterviewDb>,
    settings: State<'_, SettingsState>,
    mut req: IngestInterviewFilesRequest,
) -> Result<IngestInterviewFilesResponse, String> {
    if req.cv.is_none() && req.jd.is_none() {
        return Err("No files to ingest".to_string());
    }
    req.user_id = sanitize_user_id(&req.user_id)?;

    let (pinecone_host, expected_dim, llm_url) = {
        let g = settings.0.lock().map_err(|e| e.to_string())?;
        (
            g.pinecone_host.clone(),
            g.pinecone_vector_dimension as usize,
            g.llm_url.clone(),
        )
    };
    let pine_key = secrets::get_secret(SecretSlot::Pinecone)?.unwrap_or_default();
    let llm_api_key = secrets::get_secret(SecretSlot::Llm)?.unwrap_or_default();
    if pinecone_host.trim().is_empty() {
        return Err("Pinecone host is empty — fill it in Settings → AI.".to_string());
    }
    if pine_key.trim().is_empty() {
        return Err("Pinecone API key not set — add it in Settings → AI.".to_string());
    }
    if llm_api_key.trim().is_empty() {
        return Err("LLM API key not set — add it in Settings → AI.".to_string());
    }
    let embeddings_url = embeddings::embeddings_url_from_llm_url(&llm_url);

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let client = http_client()?;

    let mut cv_doc = None;
    let mut jd_doc = None;
    let mut cv_chunks = 0usize;
    let mut jd_chunks = 0usize;

    let ctx = IngestCtx {
        conn: &conn,
        client: &client,
        pinecone_host: &pinecone_host,
        pine_key: &pine_key,
        embeddings_url: &embeddings_url,
        embeddings_key: &llm_api_key,
        user_id: &req.user_id,
    };

    if let Some(cv) = req.cv.take() {
        let cv = cv.resolve()?;
        let (id, n) = ingest_one_doc(&app, &ctx, "cv", cv, expected_dim)?;
        cv_doc = Some(id);
        cv_chunks = n;
    }
    if let Some(jd) = req.jd.take() {
        let jd = jd.resolve()?;
        let (id, n) = ingest_one_doc(&app, &ctx, "jd", jd, expected_dim)?;
        jd_doc = Some(id);
        jd_chunks = n;
    }

    Ok(IngestInterviewFilesResponse {
        ok: true,
        cv_doc_id: cv_doc,
        jd_doc_id: jd_doc,
        cv_chunks,
        jd_chunks,
        message: format!("Indexed CV ({cv_chunks} chunks), JD ({jd_chunks} chunks)."),
    })
}

#[tauri::command]
pub fn save_interview_message(
    db: State<'_, InterviewDb>,
    req: SaveInterviewMessageRequest,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    db::insert_message(&conn, &req.user_id, &req.role, &req.content)?;
    Ok(())
}

#[tauri::command]
pub fn suggest_interview_answers(
    db: State<'_, InterviewDb>,
    settings: State<'_, SettingsState>,
    req: SuggestInterviewAnswersRequest,
) -> Result<SuggestInterviewAnswersResponse, String> {
    let (
        pinecone_host,
        expected_dim,
        llm_url,
        llm_model,
        source_language,
        target_language,
        suggestion_type,
        app_mode_from_settings,
    ) = {
        let g = settings.0.lock().map_err(|e| e.to_string())?;
        (
            g.pinecone_host.clone(),
            g.pinecone_vector_dimension as usize,
            g.llm_url.clone(),
            g.llm_model.clone(),
            g.source_language.clone(),
            g.target_language.clone(),
            g.suggestion_type.clone(),
            g.app_mode.clone(),
        )
    };
    let llm_key = secrets::get_secret(SecretSlot::Llm)?.unwrap_or_default();
    let pine_key = secrets::get_secret(SecretSlot::Pinecone)?.unwrap_or_default();
    if llm_url.trim().is_empty() {
        return Err("LLM URL not configured — fill it in Settings → AI.".to_string());
    }
    if llm_model.trim().is_empty() {
        return Err("LLM model not configured — fill it in Settings → AI.".to_string());
    }
    if llm_key.trim().is_empty() {
        return Err("LLM API key not set — add it in Settings → AI.".to_string());
    }

    // Request app_mode takes priority over persisted settings
    let app_mode = req
        .app_mode
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&app_mode_from_settings);

    if app_mode == "Meeting" {
        // ── Meeting path ──────────────────────────────────────────────────────
        // FR-7 empty-context guard: < 2 non-empty lines → return placeholder, no LLM call
        let transcript = req
            .transcript_context
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_string();
        if count_speaker_lines(&transcript) < 2 {
            return Ok(SuggestInterviewAnswersResponse {
                suggestions: vec![InterviewSuggestionItem {
                    id: 0,
                    target: "Waiting for more conversation context\u{2026}".to_string(),
                    translation: "Waiting for more conversation context\u{2026}".to_string(),
                    suggestion_kind: "talking_point".to_string(),
                }],
                debug: None,
            });
        }

        let st = normalized_suggestion_type(&suggestion_type);
        let return_format = match st {
            "both" => format!(
                "Return ONLY a JSON array. Each element must be an object with exactly three string keys: \
\"target\", \"translation\", and \"suggestion_kind\".\n\
\"target\" is the suggestion in the source language (code: {src}).\n\
\"translation\" is the same suggestion in the translation language (code: {tgt}).\n\
\"suggestion_kind\" must be one of: \"talking_point\", \"clarifying_question\", \"action_item\".\n\
Example: [{{\"target\":\"Let's align on the deadline.\",\"translation\":\"Hãy thống nhất về thời hạn.\",\"suggestion_kind\":\"action_item\"}}]\n\
No markdown fences, no extra text — ONLY the JSON array.",
                src = source_language,
                tgt = target_language,
            ),
            "target" => format!(
                "Return ONLY a JSON array. Each element must be an object with exactly two string keys: \
\"text\" and \"suggestion_kind\".\n\
\"text\" is the suggestion entirely in the source language (code: {src}).\n\
\"suggestion_kind\" must be one of: \"talking_point\", \"clarifying_question\", \"action_item\".\n\
Example: [{{\"text\":\"Can you clarify the scope?\",\"suggestion_kind\":\"clarifying_question\"}}]\n\
No markdown fences, no extra text — ONLY the JSON array.",
                src = source_language,
            ),
            _ => format!(
                "Return ONLY a JSON array. Each element must be an object with exactly two string keys: \
\"text\" and \"suggestion_kind\".\n\
\"text\" is the suggestion entirely in the translation language (code: {tgt}).\n\
\"suggestion_kind\" must be one of: \"talking_point\", \"clarifying_question\", \"action_item\".\n\
Example: [{{\"text\":\"Bạn có thể làm rõ phạm vi không?\",\"suggestion_kind\":\"clarifying_question\"}}]\n\
No markdown fences, no extra text — ONLY the JSON array.",
                tgt = target_language,
            ),
        };

        let prompt = format!(
            "You are an attentive meeting participant. The following is a recent excerpt from a live \
conversation. Based only on what was just said, provide 1\u{2013}2 concise, professional suggestions the \
participant could use. Each suggestion should be one of: a talking point to contribute, a clarifying \
question to ask, or an action item to propose. Keep each suggestion under 25 words.\n\n\
{return_format}\n\n\
Recent conversation excerpt:\n{transcript}\n",
            return_format = return_format,
            transcript = transcript,
        );

        let client = http_client()?;
        let raw = llm::complete_suggestions(&client, &llm_url, &llm_key, &llm_model, &prompt)?;
        let suggestions = parse_meeting_suggestions_from_llm(&raw, st);

        let debug = if req.debug == Some(true) {
            Some(format!(
                "mode=Meeting, llm_model={}, prompt_chars={}",
                llm_model,
                prompt.len()
            ))
        } else {
            None
        };

        return Ok(SuggestInterviewAnswersResponse { suggestions, debug });
    }

    // ── Interview path (unchanged) ────────────────────────────────────────────
    if pine_key.trim().is_empty() {
        return Err("Pinecone API key not set — add it in Settings → AI.".to_string());
    }

    let db_path = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.path()
            .map(PathBuf::from)
            .ok_or_else(|| "SQLite path unavailable".to_string())?
    };

    let uid = req.user_id.clone();
    let (recent, summary_opt) = std::thread::scope(|s| {
        let path_a = db_path.clone();
        let path_b = db_path;
        let uid_a = uid.clone();
        let uid_b = uid;
        let h1 = s.spawn(move || {
            let c = rusqlite::Connection::open(&path_a).map_err(|e| e.to_string())?;
            db::recent_messages(&c, &uid_a, 6)
        });
        let h2 = s.spawn(move || {
            let c = rusqlite::Connection::open(&path_b).map_err(|e| e.to_string())?;
            db::get_summary(&c, &uid_b)
        });
        (h1.join().unwrap(), h2.join().unwrap())
    });
    let recent = recent?;
    let summary = summary_opt?.unwrap_or_default();

    if pinecone_host.trim().is_empty() {
        return Err("Pinecone host not configured.".to_string());
    }

    let client = http_client()?;

    let mut query_bits: Vec<String> = Vec::new();
    if let Some(ref t) = req.transcript_context {
        if !t.trim().is_empty() {
            query_bits.push(format!("Latest interviewer / dialogue line:\n{t}"));
        }
    }
    if let Some(ref d) = req.user_draft {
        if !d.trim().is_empty() {
            query_bits.push(format!("Candidate draft or notes:\n{d}"));
        }
    }
    let query_text = if query_bits.is_empty() {
        "General interview coaching for the next reply.".to_string()
    } else {
        query_bits.join("\n\n")
    };

    let emb_url = embeddings::embeddings_url_from_llm_url(&llm_url);
    let (query_vec, qdim) = embeddings::embed_batch_prefer_openai(
        &client,
        &emb_url,
        &llm_key,
        std::slice::from_ref(&query_text),
        Some(expected_dim),
    )?;
    if qdim != expected_dim {
        return Err(format!(
            "Embedding dimension mismatch: got {qdim}, settings {expected_dim}."
        ));
    }
    let qv = query_vec.into_iter().next().ok_or("no query vector")?;

    let matches = query_top_k(
        &client,
        &pinecone_host,
        &pine_key,
        Some(&req.user_id),
        qv,
        4,
    )?;

    let mut context_snips: Vec<String> = Vec::new();
    for m in &matches {
        if let Some(meta) = &m.metadata {
            if let Some(serde_json::Value::String(s)) = meta.get("content") {
                if !s.trim().is_empty() {
                    context_snips.push(s.clone());
                }
            }
        }
    }

    let mut recent_lines = String::new();
    for (role, content, _) in recent {
        recent_lines.push_str(&format!("- [{role}] {content}\n"));
    }

    let ctx_block = context_snips.join("\n---\n");
    let st = normalized_suggestion_type(&suggestion_type);
    let return_format = match st {
        "both" => format!(
            "Return ONLY a JSON array with exactly 1 object. The object must have exactly two string keys: \"target\" and \"translation\".\n\
The \"target\" value must be the answer in the source/interview language (settings code: {src}).\n\
The \"translation\" value must be the same answer meaning in the translation language (settings code: {tgt}).\n\
Example: [{{\"target\":\"Hello\",\"translation\":\"Xin chào\"}}]\n\
No markdown fences, no extra text — ONLY the JSON array.",
            src = source_language,
            tgt = target_language,
        ),
        "target" => format!(
            "Return ONLY a JSON array with exactly 1 string. The string must be an answer option entirely in the source/interview language (settings code: {src}).\n\
Example: [\"option one\"]\n\
No markdown fences, no extra text — ONLY the JSON array.",
            src = source_language,
        ),
        _ => format!(
            "Return ONLY a JSON array with exactly 1 string. The string must be an answer option entirely in the translation language (settings code: {tgt}).\n\
Example: [\"option one\"]\n\
No markdown fences, no extra text — ONLY the JSON array.",
            tgt = target_language,
        ),
    };

    let prompt = format!(
        "You are the candidate — a senior software engineer. Speak in first person as if you are answering the interviewer directly.\n\
Use the CV/JD snippets below to ground your answer in the candidate's real experience: specific technologies, projects, and patterns they have worked with.\n\
Make the answer technical and concrete (mention design patterns, frameworks, real examples from the CV), not generic theory.\n\
Keep it concise, spoken tone, under ~80 words.\n\n\
{return_format}\n\n\
Session summary (may be empty):\n\
{summary}\n\n\
Recent messages / transcript:\n\
{recent_lines}\n\n\
Candidate CV/JD knowledge (use this to make the answer personal and technical):\n\
{ctx_block}\n\n\
Interviewer's question / task focus:\n\
{query_text}\n",
        return_format = return_format,
        summary = summary,
        recent_lines = recent_lines,
        ctx_block = ctx_block,
        query_text = query_text,
    );

    let raw = llm::complete_suggestions(&client, &llm_url, &llm_key, &llm_model, &prompt)?;
    let suggestions = parse_suggestions_from_llm(&raw, st);

    let debug = if req.debug == Some(true) {
        Some(format!(
            "pinecone_matches={}, llm_model={}, prompt_chars={}",
            matches.len(),
            llm_model,
            prompt.len()
        ))
    } else {
        None
    };

    Ok(SuggestInterviewAnswersResponse { suggestions, debug })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_text_empty_inputs() {
        assert!(chunk_text_with_overlap("", 100, 10).is_empty());
        assert!(chunk_text_with_overlap("hello", 0, 0).is_empty());
    }

    #[test]
    fn chunk_text_short_text_single_chunk() {
        let chunks = chunk_text_with_overlap("one two three", 100, 10);
        assert_eq!(chunks, vec!["one two three"]);
    }

    #[test]
    fn chunk_text_splits_on_word_boundaries() {
        // 5 words of 4 chars => "aaaa bbbb" is 9 bytes; max 9 => two chunks.
        let chunks = chunk_text_with_overlap("aaaa bbbb cccc dddd", 9, 0);
        assert_eq!(chunks, vec!["aaaa bbbb", "cccc dddd"]);
        for c in &chunks {
            assert!(c.len() <= 9);
            assert!(!c.starts_with(' ') && !c.ends_with(' '));
        }
    }

    #[test]
    fn chunk_text_overlap_carries_suffix_forward() {
        let chunks = chunk_text_with_overlap("aaaa bbbb cccc dddd eeee", 9, 4);
        assert!(chunks.len() >= 2);
        // Each chunk after the first starts with a suffix of the previous chunk.
        for pair in chunks.windows(2) {
            let prev_last_word = pair[0].rsplit(' ').next().unwrap();
            assert!(
                pair[1].contains(prev_last_word),
                "chunk {:?} should overlap with {:?}",
                pair[1],
                pair[0]
            );
        }
    }

    #[test]
    fn chunk_text_oversized_word_not_dropped() {
        // A single word longer than max_chars still gets emitted, not lost.
        let chunks = chunk_text_with_overlap("supercalifragilistic ok", 5, 0);
        assert_eq!(chunks.len(), 2);
        assert!(chunks.iter().any(|c| c == "supercalifragilistic"));
        assert!(chunks.iter().any(|c| c == "ok"));
    }

    #[test]
    fn parse_suggestions_json_array_of_strings_translation() {
        let raw = r#"Here you go: ["Chào bạn", "Rất vui được gặp"]"#;
        let items = parse_suggestions_from_llm(raw, "translation");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].translation, "Chào bạn");
        assert_eq!(items[0].target, "");
        assert_eq!(items[0].suggestion_kind, "answer");
    }

    #[test]
    fn parse_suggestions_json_rows_both() {
        let raw = r#"[{"target": "What is your experience?", "translation": "Kinh nghiệm của bạn là gì?"}]"#;
        let items = parse_suggestions_from_llm(raw, "both");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].target, "What is your experience?");
        assert_eq!(items[0].translation, "Kinh nghiệm của bạn là gì?");
    }

    #[test]
    fn parse_suggestions_plain_text_fallback() {
        let raw = "- First answer\n• Second answer\n\nThird answer";
        let items = parse_suggestions_from_llm(raw, "target");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].target, "First answer");
        assert_eq!(items[1].target, "Second answer");
        assert_eq!(items[2].target, "Third answer");
    }

    #[test]
    fn parse_suggestions_both_falls_back_to_string_array() {
        // 'both' prefers {target, translation} rows but accepts plain strings.
        let raw = r#"["answer one", "answer two"]"#;
        let items = parse_suggestions_from_llm(raw, "both");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].target, "answer one");
        assert_eq!(items[0].translation, "answer one");
    }

    #[test]
    fn parse_suggestions_unknown_type_defaults_to_translation() {
        let items = parse_suggestions_from_llm("[\"x\"]", "bogus");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].translation, "x");
        assert_eq!(items[0].target, "");
    }
}
