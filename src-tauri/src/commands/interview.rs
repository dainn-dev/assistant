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
use tauri::{ipc::Channel, AppHandle, Emitter, State};
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
    /// Pre-selected CV/JD excerpts (from `select_context_excerpts`) — the
    /// early path skips embedding+Pinecone and grounds on these directly.
    #[serde(default)]
    pub context_snippets: Option<Vec<String>>,
}

/// Cancellation registry for in-flight suggestion streams. Each request gets
/// a flag; `cancel_suggestion_stream` flips it and the streaming loop checks
/// it between chunks.
pub struct SuggestStreamState {
    pub cancels: std::sync::Mutex<HashMap<u32, std::sync::Arc<std::sync::atomic::AtomicBool>>>,
}

impl Default for SuggestStreamState {
    fn default() -> Self {
        Self {
            cancels: std::sync::Mutex::new(HashMap::new()),
        }
    }
}

/// One document excerpt tagged by its source document kind.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextExcerpt {
    pub doc_type: String,
    pub text: String,
}

/// Events pushed to the frontend over the request's IPC channel.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StreamEvent {
    pub request_id: u32,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
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

pub(crate) fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())
}

pub(crate) fn extract_json_array_slice(text: &str) -> Option<&str> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end <= start {
        return None;
    }
    Some(&text[start..=end])
}

/// First `{..last }` span — for LLM outputs that must be a JSON object.
pub(crate) fn extract_json_object_slice(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
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

    // ── Interview path ────────────────────────────────────────────────────────
    // Fast path: pre-selected local excerpts (select_context_excerpts) mean no
    // embedding call and no Pinecone round-trip — and no Pinecone key needed.
    // Pinecone remains the fallback only when no snippets were provided.
    let local_snips: Vec<String> = req
        .context_snippets
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect();
    let use_local_excerpts = !local_snips.is_empty();

    require_context_source(!use_local_excerpts, &pine_key)?;

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

    let context_snips: Vec<String> = if use_local_excerpts {
        local_snips
    } else {
        if pinecone_host.trim().is_empty() {
            return Err("Pinecone host not configured.".to_string());
        }

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

        let mut snips: Vec<String> = Vec::new();
        for m in &matches {
            if let Some(meta) = &m.metadata {
                if let Some(serde_json::Value::String(s)) = meta.get("content") {
                    if !s.trim().is_empty() {
                        snips.push(s.clone());
                    }
                }
            }
        }
        snips
    };

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
Use the [SUMMARY], [STORY], [STRENGTH], [CV] and [JD] excerpts below. If a [STORY] fits the question, answer with that story's Situation→Action→Result in first person. Never claim experience that is not in the excerpts.\n\
Make the answer technical and concrete (mention design patterns, frameworks, real examples from the excerpts), not generic theory.\n\
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
            "context_source={}, context_snips={}, llm_model={}, prompt_chars={}",
            if use_local_excerpts {
                "local"
            } else {
                "pinecone"
            },
            context_snips.len(),
            llm_model,
            prompt.len()
        ))
    } else {
        None
    };

    Ok(SuggestInterviewAnswersResponse { suggestions, debug })
}

/// Pinecone is only required when the request carries no local excerpts.
fn require_context_source(needs_pinecone: bool, pine_key: &str) -> Result<(), String> {
    if needs_pinecone && pine_key.trim().is_empty() {
        return Err("Pinecone API key not set — add it in Settings → AI.".to_string());
    }
    Ok(())
}

/// Lexical relevance score: normalized token-overlap count between the query
/// and a document chunk. Cheap enough to run on every early-hint request —
/// no embeddings, no network.
fn score_excerpt(query: &str, chunk: &str) -> usize {
    let chunk_lower = chunk.to_lowercase();
    query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3)
        .filter(|t| chunk_lower.contains(*t))
        .count()
}

/// Pick the most query-relevant CV/JD excerpts for the early-hint path —
/// lexical scoring over the locally stored document text, keeping the doc
/// boundary (cv/jd) so the prompt never presents a job requirement as the
/// candidate's own experience.
#[tauri::command]
pub fn select_context_excerpts(
    db: State<'_, InterviewDb>,
    user_id: String,
    query: String,
    max_chars: usize,
) -> Result<Vec<ContextExcerpt>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    select_excerpts_inner(&conn, &user_id, &query, max_chars)
}

/// Excerpt selection over documents + profile items. Summary always leads when
/// present; stories/strengths rank lexically alongside CV/JD chunks. Pure with
/// respect to `conn` so tests can use an in-memory database.
fn select_excerpts_inner(
    conn: &rusqlite::Connection,
    user_id: &str,
    query: &str,
    max_chars: usize,
) -> Result<Vec<ContextExcerpt>, String> {
    let docs = db::get_documents(conn, user_id)?;
    let profile = db::list_profile_items(conn, user_id)?;
    let budget = if max_chars == 0 { 3000 } else { max_chars };

    let mut summary_item: Option<ContextExcerpt> = None;
    let mut scored: Vec<(usize, String, String)> = Vec::new();
    for (doc_type, text) in docs {
        for chunk in
            chunk_text_with_overlap(&text, INGEST_CHUNK_MAX_CHARS, INGEST_CHUNK_OVERLAP_CHARS)
        {
            scored.push((score_excerpt(query, &chunk), doc_type.clone(), chunk));
        }
    }
    for item in profile {
        match item.kind.as_str() {
            "summary" => {
                summary_item = Some(ContextExcerpt {
                    doc_type: "summary".into(),
                    text: item.content,
                });
            }
            "story" | "strength" => {
                let text = if item.title.is_empty() {
                    item.content.clone()
                } else {
                    format!("{}: {}", item.title, item.content)
                };
                let score = score_excerpt(query, &format!("{} {}", item.title, item.content));
                scored.push((score, item.kind, text));
            }
            _ => {}
        }
    }
    // Highest overlap first; stable order keeps deterministic picks on ties.
    scored.sort_by_key(|a| std::cmp::Reverse(a.0));
    let scored = dedupe_excerpts(scored);

    let mut out = Vec::new();
    let mut used = 0usize;
    if let Some(s) = summary_item {
        used += s.text.len();
        out.push(s);
    }
    for (score, doc_type, chunk) in scored {
        if score == 0 && !out.is_empty() {
            break; // ranked hits exhausted — no padding with irrelevant chunks
        }
        if used + chunk.len() > budget && !out.is_empty() {
            continue;
        }
        used += chunk.len();
        out.push(ContextExcerpt {
            doc_type,
            text: chunk,
        });
        if used >= budget {
            break;
        }
    }
    Ok(out)
}

/// Drop chunks whose (case-insensitive) text is contained in an earlier kept
/// chunk — a profile story that repeats a CV passage must not eat the budget.
/// First kept wins, so higher-scored entries always survive.
fn dedupe_excerpts(items: Vec<(usize, String, String)>) -> Vec<(usize, String, String)> {
    let mut kept: Vec<(usize, String, String)> = Vec::new();
    'outer: for (score, kind, text) in items {
        let lower = text.to_lowercase();
        for (_, _, prev) in &kept {
            let pl = prev.to_lowercase();
            if pl.contains(&lower) || lower.contains(&pl) {
                continue 'outer;
            }
        }
        kept.push((score, kind, text));
    }
    kept
}

/// Abort an in-flight suggestion stream. Unknown/stale request ids are
/// no-ops so a late cancel can never kill a newer request.
#[tauri::command]
pub fn cancel_suggestion_stream(request_id: u32, streams: State<'_, SuggestStreamState>) {
    if let Ok(map) = streams.cancels.lock() {
        if let Some(flag) = map.get(&request_id) {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

/// Early-hint path: stream 2–3 plain-text talking points while the
/// interviewer is still speaking. Skips embedding + Pinecone — the frontend
/// passes pre-selected excerpts via `req.context_snippets`.
#[tauri::command]
pub async fn suggest_interview_answers_stream(
    db: State<'_, InterviewDb>,
    settings: State<'_, SettingsState>,
    streams: State<'_, SuggestStreamState>,
    req: SuggestInterviewAnswersRequest,
    request_id: u32,
    channel: Channel<StreamEvent>,
) -> Result<(), String> {
    let (llm_url, llm_model, source_language, target_language, suggestion_type) = {
        let g = settings.0.lock().map_err(|e| e.to_string())?;
        (
            g.llm_url.clone(),
            g.llm_model.clone(),
            g.source_language.clone(),
            g.target_language.clone(),
            g.suggestion_type.clone(),
        )
    };
    let llm_key = secrets::get_secret(SecretSlot::Llm)?.unwrap_or_default();
    if llm_url.trim().is_empty() {
        return Err("LLM URL not configured — fill it in Settings → AI.".to_string());
    }
    if llm_model.trim().is_empty() {
        return Err("LLM model not configured — fill it in Settings → AI.".to_string());
    }
    if llm_key.trim().is_empty() {
        return Err("LLM API key not set — add it in Settings → AI.".to_string());
    }

    let send = |kind: &str, text: Option<String>| {
        let _ = channel.send(StreamEvent {
            request_id,
            kind: kind.to_string(),
            text,
        });
    };

    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let mut map = streams.cancels.lock().map_err(|e| e.to_string())?;
        map.insert(request_id, cancel.clone());
    }

    send("start", None);

    let question = req.transcript_context.clone().unwrap_or_default();
    if question.trim().chars().count() < 8 {
        send("insufficient", None);
        if let Ok(mut map) = streams.cancels.lock() {
            map.remove(&request_id);
        }
        return Ok(());
    }

    // Recent dialogue gives the model conversational context around the
    // partial question — bounded to the last few lines.
    let recent_lines = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        db::recent_messages(&conn, &req.user_id, 4)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(role, content, _)| format!("- [{role}] {content}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let ctx_block = req
        .context_snippets
        .clone()
        .unwrap_or_default()
        .join("\n---\n");

    // Hints render in the language the user reads: suggestion language for
    // 'translation'/'both', interview language for 'target'.
    let hint_lang = if normalized_suggestion_type(&suggestion_type) == "target" {
        &source_language
    } else {
        &target_language
    };

    let prompt = format!(
        "You are coaching a candidate during a live interview. The interviewer's question may be INCOMPLETE — it is still being transcribed. Answer based only on what is clear so far.\n\
Rules:\n\
- Give 2-3 short talking points, each under 20 words, in language code {hint_lang}.\n\
- Ground points in the [SUMMARY]/[STORY]/[CV]/[JD] excerpts when relevant; if a [STORY] fits, hint at its Situation→Action→Result; never invent experience that is not there.\n\
- If the question is too vague to answer, give one line on what to listen for or clarify.\n\
- Plain text only — one point per line starting with \"- \". No JSON, no preamble.\n\n\
CV/JD excerpts (may be empty):\n{ctx_block}\n\n\
Recent dialogue (may be empty):\n{recent_lines}\n\n\
Interviewer question so far (may be incomplete):\n{question}\n",
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let result = llm::complete_suggestions_stream(
        &client,
        &llm_url,
        &llm_key,
        &llm_model,
        &prompt,
        cancel,
        |delta| send("delta", Some(delta)),
    )
    .await;

    if let Ok(mut map) = streams.cancels.lock() {
        map.remove(&request_id);
    }

    match result {
        Ok(full) => send("done", Some(full)),
        Err(e) if e == "__cancelled__" => send("cancelled", None),
        Err(e) => send("error", Some(e)),
    }
    Ok(())
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

    #[test]
    fn pinecone_key_not_required_when_local_excerpts_present() {
        // needs_pinecone=false → any key state is fine
        assert!(require_context_source(false, "").is_ok());
        assert!(require_context_source(false, "pc-key").is_ok());
    }

    #[test]
    fn pinecone_key_required_only_for_fallback_path() {
        assert!(require_context_source(true, "").is_err());
        assert!(require_context_source(true, "   ").is_err());
        assert!(require_context_source(true, "pc-key").is_ok());
    }

    #[test]
    fn score_excerpt_ranks_relevant_chunk_higher() {
        let relevant = "Debugged a production deadlock by ordering lock acquisition";
        let unrelated = "Built a landing page with React and Tailwind";
        assert!(
            score_excerpt("deadlock transaction retry", relevant)
                > score_excerpt("deadlock transaction retry", unrelated)
        );
        assert_eq!(score_excerpt("", relevant), 0);
    }

    fn mem_db() -> rusqlite::Connection {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrate(&c).unwrap();
        c
    }

    fn profile_item(id: &str, kind: &str, title: &str, content: &str) -> db::ProfileItem {
        db::ProfileItem {
            id: id.into(),
            user_id: "u".into(),
            kind: kind.into(),
            title: title.into(),
            content: content.into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn excerpts_put_summary_first() {
        let c = mem_db();
        db::upsert_profile_item(
            &c,
            &profile_item(
                "sum",
                "summary",
                "",
                "Senior backend engineer, Rust + Postgres",
            ),
        )
        .unwrap();
        db::upsert_document(
            &c,
            "d1",
            "u",
            "cv",
            "cv.pdf",
            None,
            "built payment gateway in Go",
        )
        .unwrap();
        let out = select_excerpts_inner(&c, "u", "payment gateway", 3000).unwrap();
        assert_eq!(out[0].doc_type, "summary");
        assert!(out.iter().any(|e| e.doc_type == "cv"));
    }

    #[test]
    fn story_and_cv_duplicates_keep_one() {
        let c = mem_db();
        let dup = "Situation: prod deadlock Task: fix Action: ordered locks Result: zero retries";
        db::upsert_profile_item(&c, &profile_item("s1", "story", "Deadlock fix", dup)).unwrap();
        db::upsert_document(&c, "d1", "u", "cv", "cv.pdf", None, dup).unwrap();
        let out = select_excerpts_inner(&c, "u", "deadlock locks", 3000).unwrap();
        let matches = out
            .iter()
            .filter(|e| e.text.contains("ordered locks"))
            .count();
        assert_eq!(matches, 1, "duplicated text must appear once: {:?}", out);
    }

    #[test]
    fn story_scored_by_title_and_content() {
        let c = mem_db();
        db::upsert_profile_item(
            &c,
            &profile_item(
                "s1",
                "story",
                "Deadlock incident",
                "Situation: db froze Task: restore",
            ),
        )
        .unwrap();
        db::upsert_document(
            &c,
            "d1",
            "u",
            "cv",
            "cv.pdf",
            None,
            "wrote css for landing pages",
        )
        .unwrap();
        let out = select_excerpts_inner(&c, "u", "deadlock", 3000).unwrap();
        assert_eq!(out[0].doc_type, "story");
        assert!(out[0].text.starts_with("Deadlock incident:"));
    }
}
