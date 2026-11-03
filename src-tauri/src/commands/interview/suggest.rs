//! Manual (button / chat) answer suggestions and chat message history.

use super::excerpts::semantic_excerpts;
use crate::commands::util::{extract_json_array_slice, http_client, run_blocking};
use crate::copilot::{presets, prompts};
use crate::db::{self, InterviewDb};
use crate::secrets::{self, SecretSlot};
use crate::services::embeddings;
use crate::services::llm;
use crate::settings::SettingsState;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};

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
    /// early path skips embedding + semantic search and grounds on these directly.
    #[serde(default)]
    pub context_snippets: Option<Vec<String>>,
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

pub(super) fn normalized_suggestion_type(raw: &str) -> &str {
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
pub async fn suggest_interview_answers(
    app: AppHandle,
    req: SuggestInterviewAnswersRequest,
) -> Result<SuggestInterviewAnswersResponse, String> {
    run_blocking(move || {
        let db = app.state::<InterviewDb>();
        let settings = app.state::<SettingsState>();
        suggest_interview_answers_blocking(&db, &settings, req)
    })
    .await
}

fn suggest_interview_answers_blocking(
    db: &InterviewDb,
    settings: &SettingsState,
    req: SuggestInterviewAnswersRequest,
) -> Result<SuggestInterviewAnswersResponse, String> {
    let (llm_url, llm_model, source_language, target_language, suggestion_type, app_mode) = {
        let g = settings.0.lock().map_err(|e| e.to_string())?;
        (
            g.llm_url.clone(),
            g.llm_model.clone(),
            g.source_language.clone(),
            g.target_language.clone(),
            g.suggestion_type.clone(),
            g.app_mode.clone(),
        )
    };
    let preset = presets::preset_for_mode(&app_mode);
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

    // ── Interview path ────────────────────────────────────────────────────────
    // Fast path: pre-selected lexical excerpts (select_context_excerpts) mean
    // no embedding call. On-device semantic search over the stored chunk
    // vectors is the fallback only when no snippets were provided.
    let local_snips: Vec<String> = req
        .context_snippets
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect();
    let use_local_excerpts = !local_snips.is_empty();

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

    let query_text = prompts::suggest_query(
        preset,
        req.transcript_context.as_deref(),
        req.user_draft.as_deref(),
    );

    let context_snips: Vec<String> = if use_local_excerpts {
        local_snips
    } else {
        let emb_url = embeddings::embeddings_url_from_llm_url(&llm_url);
        semantic_excerpts(
            db,
            preset,
            &client,
            &emb_url,
            &llm_key,
            &req.user_id,
            &query_text,
        )?
    };

    let mut recent_lines = String::new();
    for (role, content, _) in recent {
        recent_lines.push_str(&format!("- [{role}] {content}\n"));
    }

    let ctx_block = context_snips.join("\n---\n");
    let st = normalized_suggestion_type(&suggestion_type);
    let prompt = prompts::suggest_prompt(
        preset,
        &prompts::SuggestInputs {
            source_language: &source_language,
            target_language: &target_language,
            suggestion_type: st,
            summary: &summary,
            recent_lines: &recent_lines,
            ctx_block: &ctx_block,
            query_text: &query_text,
        },
    );

    let raw = llm::complete_suggestions(&client, &llm_url, &llm_key, &llm_model, &prompt, 600)?;
    let suggestions = parse_suggestions_from_llm(&raw, st);

    let debug = if req.debug == Some(true) {
        Some(format!(
            "context_source={}, context_snips={}, llm_model={}, prompt_chars={}",
            if use_local_excerpts {
                "local"
            } else {
                "semantic"
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

#[cfg(test)]
mod tests {
    use super::*;

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
