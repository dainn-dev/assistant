//! Early-hint streaming suggestions and their cancellation.

use super::suggest::{normalized_suggestion_type, SuggestInterviewAnswersRequest};
use crate::copilot::{presets, prompts, SilenceFilter};
use crate::db::{self, InterviewDb};
use crate::secrets::{self, SecretSlot};
use crate::services::llm;
use crate::settings::SettingsState;
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use tauri::{ipc::Channel, State};

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

/// Events pushed to the frontend over the request's IPC channel.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StreamEvent {
    pub request_id: u32,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
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
/// interviewer is still speaking. Skips embedding + semantic search — the frontend
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

    let prompt = prompts::early_prompt(
        preset,
        &prompts::EarlyInputs {
            hint_lang,
            ctx_block: &ctx_block,
            recent_lines: &recent_lines,
            question: &question,
        },
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    // Meeting hints may answer SILENCE_TOKEN ("nothing for you"); hold it back
    // and report "skip" instead of flashing it in the hint card.
    let may_be_silent = preset.hints_may_be_silent;
    let mut silence = SilenceFilter::default();
    let result = llm::complete_suggestions_stream(
        &client,
        &llm_url,
        &llm_key,
        &llm_model,
        &prompt,
        cancel,
        |delta| {
            if !may_be_silent {
                send("delta", Some(delta));
            } else if let Some(text) = silence.push(&delta) {
                send("delta", Some(text));
            }
        },
    )
    .await;

    if let Ok(mut map) = streams.cancels.lock() {
        map.remove(&request_id);
    }

    match result {
        Ok(full) if may_be_silent && SilenceFilter::is_silent(&full) => send("skip", None),
        Ok(full) => send("done", Some(full)),
        Err(e) if e == "__cancelled__" => send("cancelled", None),
        Err(e) => send("error", Some(e)),
    }
    Ok(())
}
