//! Document ingest: extract → chunk → embed → store in SQLite.

use super::documents::{
    chunk_text_with_overlap, extract_file_text, InterviewFilePart, INGEST_CHUNK_MAX_CHARS,
    INGEST_CHUNK_OVERLAP_CHARS,
};
use crate::commands::util::{http_client, run_blocking};
use crate::copilot::presets;
use crate::db::{self, InterviewDb};
use crate::secrets::{self, SecretSlot};
use crate::services::embeddings;
use crate::settings::SettingsState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

/// Stored rows are scoped by `user_id` — keep it a sane identifier
/// (non-empty, bounded, no control chars that could collide across users).
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
    /// Locked only around DB writes — never across embedding calls, or
    /// early-hint excerpt selection stalls for the whole ingest.
    db: &'a InterviewDb,
    client: &'a reqwest::blocking::Client,
    embeddings_url: &'a str,
    embeddings_key: &'a str,
    user_id: &'a str,
}

fn ingest_one_doc(
    app: &AppHandle,
    ctx: &IngestCtx<'_>,
    doc_type: &str,
    part: InterviewFilePart,
) -> Result<(String, usize), String> {
    emit_progress(app, doc_type, "extracting", 0, 1);
    let text = extract_file_text(&part.filename, &part.bytes)?;
    let doc_id = Uuid::new_v4().to_string();
    {
        let conn = ctx.db.0.lock().map_err(|e| e.to_string())?;
        db::upsert_document(
            &conn,
            &doc_id,
            ctx.user_id,
            doc_type,
            &part.filename,
            Some(&part.bytes),
            &text,
        )?;
    }

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

    // Dedupe chunks BEFORE embedding to avoid storing duplicates.
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
    let mut embedded: Vec<(String, Vec<f32>)> = Vec::with_capacity(total_chunks);
    let max = embeddings::OPENAI_EMBEDDINGS_MAX_INPUTS;
    for batch_start in (0..unique_chunks.len()).step_by(max) {
        let batch_end = (batch_start + max).min(unique_chunks.len());
        let batch: Vec<String> = unique_chunks[batch_start..batch_end]
            .iter()
            .map(|(_, ch)| ch.clone())
            .collect();
        let (vecs, _dim) = embeddings::embed_batch_prefer_openai(
            ctx.client,
            ctx.embeddings_url,
            ctx.embeddings_key,
            &batch,
            None,
        )?;
        emit_progress(app, doc_type, "embedding", batch_end, total_chunks);
        if vecs.len() != batch.len() {
            return Err(format!(
                "Batch embedding returned {} vectors for {} chunks — API response mismatch.",
                vecs.len(),
                batch.len()
            ));
        }
        embedded.extend(batch.into_iter().zip(vecs));
    }
    emit_progress(app, doc_type, "upserting", 0, 1);
    {
        let conn = ctx.db.0.lock().map_err(|e| e.to_string())?;
        db::insert_doc_chunks(&conn, &doc_id, ctx.user_id, doc_type, &embedded)?;
    }
    emit_progress(app, doc_type, "done", total_chunks, total_chunks);
    Ok((doc_id, total_chunks))
}

#[tauri::command]
pub async fn ingest_interview_files(
    app: AppHandle,
    req: IngestInterviewFilesRequest,
) -> Result<IngestInterviewFilesResponse, String> {
    run_blocking(move || {
        let db = app.state::<InterviewDb>();
        let settings = app.state::<SettingsState>();
        ingest_interview_files_blocking(&app, &db, &settings, req)
    })
    .await
}

fn ingest_interview_files_blocking(
    app: &AppHandle,
    db: &InterviewDb,
    settings: &SettingsState,
    mut req: IngestInterviewFilesRequest,
) -> Result<IngestInterviewFilesResponse, String> {
    if req.cv.is_none() && req.jd.is_none() {
        return Err("No files to ingest".to_string());
    }
    req.user_id = sanitize_user_id(&req.user_id)?;

    let (llm_url, app_mode) = {
        let g = settings.0.lock().map_err(|e| e.to_string())?;
        (g.llm_url.clone(), g.app_mode.clone())
    };
    // `cv` / `jd` on the wire are the preset's first / second upload slot.
    let [slot_a, slot_b] = &presets::preset_for_mode(&app_mode).doc_slots;
    let llm_api_key = secrets::get_secret(SecretSlot::Llm)?.unwrap_or_default();
    if llm_api_key.trim().is_empty() {
        return Err("LLM API key not set — add it in Settings → AI.".to_string());
    }
    let embeddings_url = embeddings::embeddings_url_from_llm_url(&llm_url);

    let client = http_client()?;

    let mut cv_doc = None;
    let mut jd_doc = None;
    let mut cv_chunks = 0usize;
    let mut jd_chunks = 0usize;

    let ctx = IngestCtx {
        db,
        client: &client,
        embeddings_url: &embeddings_url,
        embeddings_key: &llm_api_key,
        user_id: &req.user_id,
    };

    if let Some(cv) = req.cv.take() {
        let cv = cv.resolve()?;
        let (id, n) = ingest_one_doc(app, &ctx, slot_a.doc_type, cv)?;
        cv_doc = Some(id);
        cv_chunks = n;
    }
    if let Some(jd) = req.jd.take() {
        let jd = jd.resolve()?;
        let (id, n) = ingest_one_doc(app, &ctx, slot_b.doc_type, jd)?;
        jd_doc = Some(id);
        jd_chunks = n;
    }

    Ok(IngestInterviewFilesResponse {
        ok: true,
        cv_doc_id: cv_doc,
        jd_doc_id: jd_doc,
        cv_chunks,
        jd_chunks,
        message: format!(
            "Indexed {} ({cv_chunks} chunks), {} ({jd_chunks} chunks).",
            slot_a.label, slot_b.label
        ),
    })
}
