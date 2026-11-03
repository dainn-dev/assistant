//! Context excerpts for prompts: lexical selection over stored documents
//! and profile items, plus on-device semantic search over chunk vectors.

use super::documents::{
    chunk_text_with_overlap, INGEST_CHUNK_MAX_CHARS, INGEST_CHUNK_OVERLAP_CHARS,
};
use crate::copilot::presets;
use crate::db::{self, InterviewDb};
use crate::services::embeddings;
use crate::services::vector_search;
use crate::settings::SettingsState;
use serde::Serialize;
use tauri::State;

/// One document excerpt tagged by its source document kind.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextExcerpt {
    pub doc_type: String,
    pub text: String,
}

const SEMANTIC_EXCERPTS_K: usize = 4;

/// Non-empty chunk contents most similar to `query_vec`, labelled `[CV]` /
/// `[JD]` like the lexical excerpts so the prompt keeps the doc boundary.
fn rank_chunk_excerpts(query_vec: &[f32], chunks: Vec<db::DocChunk>, k: usize) -> Vec<String> {
    let items = chunks
        .into_iter()
        .filter(|c| !c.content.trim().is_empty())
        .map(|c| ((c.doc_type, c.content), c.embedding))
        .collect();
    vector_search::top_k_cosine(query_vec, items, k)
        .into_iter()
        .map(|(doc_type, content)| format!("[{}] {content}", doc_type.to_uppercase()))
        .collect()
}

/// On-device semantic search over the user's stored chunk vectors. Skips the
/// embedding call entirely when nothing has been ingested yet.
pub(super) fn semantic_excerpts(
    db: &InterviewDb,
    preset: &presets::CopilotPreset,
    client: &reqwest::blocking::Client,
    embeddings_url: &str,
    embeddings_key: &str,
    user_id: &str,
    query: &str,
) -> Result<Vec<String>, String> {
    let chunks: Vec<db::DocChunk> = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        db::list_doc_chunks(&conn, user_id)?
    }
    .into_iter()
    .filter(|c| preset.owns_doc_type(&c.doc_type))
    .collect();
    if chunks.is_empty() {
        return Ok(Vec::new());
    }
    let (vecs, _dim) = embeddings::embed_batch_prefer_openai(
        client,
        embeddings_url,
        embeddings_key,
        &[query.to_string()],
        None,
    )?;
    let query_vec = vecs.into_iter().next().ok_or("no query vector")?;
    Ok(rank_chunk_excerpts(&query_vec, chunks, SEMANTIC_EXCERPTS_K))
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
    settings: State<'_, SettingsState>,
    user_id: String,
    query: String,
    max_chars: usize,
) -> Result<Vec<ContextExcerpt>, String> {
    let app_mode = settings
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .app_mode
        .clone();
    let preset = presets::preset_for_mode(&app_mode);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    select_excerpts_inner(&conn, preset, &user_id, &query, max_chars)
}

/// Excerpt selection over the preset's documents (+ profile items when the
/// preset has a profile). Summary always leads when present; stories and
/// strengths rank lexically alongside document chunks. Pure with respect to
/// `conn` so tests can use an in-memory database.
fn select_excerpts_inner(
    conn: &rusqlite::Connection,
    preset: &presets::CopilotPreset,
    user_id: &str,
    query: &str,
    max_chars: usize,
) -> Result<Vec<ContextExcerpt>, String> {
    let docs: Vec<(String, String)> = db::get_documents(conn, user_id)?
        .into_iter()
        .filter(|(doc_type, _)| preset.owns_doc_type(doc_type))
        .collect();
    let profile = if preset.has_profile {
        db::list_profile_items(conn, user_id)?
    } else {
        Vec::new()
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_chunk(doc_type: &str, content: &str, v: &[f32]) -> db::DocChunk {
        db::DocChunk {
            doc_type: doc_type.into(),
            content: content.into(),
            embedding: v.to_vec(),
        }
    }

    #[test]
    fn rank_chunk_excerpts_labels_by_doc_type_best_first() {
        let chunks = vec![
            doc_chunk("jd", "needs Kafka", &[0.0, 1.0]),
            doc_chunk("cv", "built Kafka pipeline", &[1.0, 0.1]),
            doc_chunk("cv", "   ", &[1.0, 0.0]),
        ];
        let out = rank_chunk_excerpts(&[1.0, 0.0], chunks, 4);
        assert_eq!(
            out,
            vec![
                "[CV] built Kafka pipeline".to_string(),
                "[JD] needs Kafka".to_string()
            ]
        );
    }

    #[test]
    fn rank_chunk_excerpts_respects_k() {
        let chunks = vec![
            doc_chunk("cv", "a", &[1.0, 0.0]),
            doc_chunk("cv", "b", &[0.9, 0.1]),
        ];
        assert_eq!(rank_chunk_excerpts(&[1.0, 0.0], chunks, 1), vec!["[CV] a"]);
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
        let out =
            select_excerpts_inner(&c, &presets::INTERVIEW, "u", "payment gateway", 3000).unwrap();
        assert_eq!(out[0].doc_type, "summary");
        assert!(out.iter().any(|e| e.doc_type == "cv"));
    }

    #[test]
    fn story_and_cv_duplicates_keep_one() {
        let c = mem_db();
        let dup = "Situation: prod deadlock Task: fix Action: ordered locks Result: zero retries";
        db::upsert_profile_item(&c, &profile_item("s1", "story", "Deadlock fix", dup)).unwrap();
        db::upsert_document(&c, "d1", "u", "cv", "cv.pdf", None, dup).unwrap();
        let out =
            select_excerpts_inner(&c, &presets::INTERVIEW, "u", "deadlock locks", 3000).unwrap();
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
        let out = select_excerpts_inner(&c, &presets::INTERVIEW, "u", "deadlock", 3000).unwrap();
        assert_eq!(out[0].doc_type, "story");
        assert!(out[0].text.starts_with("Deadlock incident:"));
    }

    #[test]
    fn meeting_excerpts_exclude_interview_docs_and_profile() {
        let c = mem_db();
        db::upsert_document(
            &c,
            "d1",
            "u",
            "cv",
            "cv.pdf",
            None,
            "deadlock fix in payments",
        )
        .unwrap();
        db::upsert_document(&c, "d2", "u", "agenda", "a.md", None, "deadlock postmortem").unwrap();
        db::upsert_profile_item(
            &c,
            &profile_item("s1", "story", "Deadlock", "deadlock story"),
        )
        .unwrap();
        let out = select_excerpts_inner(&c, &presets::MEETING, "u", "deadlock", 3000).unwrap();
        let types: Vec<&str> = out.iter().map(|e| e.doc_type.as_str()).collect();
        assert_eq!(types, vec!["agenda"]);
    }
}
