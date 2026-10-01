//! Candidate profile CRUD — persistent summary / STAR stories / strengths that
//! join CV/JD excerpts in grounding interview suggestions.

use crate::commands::interview::extract_json_array_slice;
use crate::db::{self, InterviewDb, ProfileItem};
use crate::secrets::{self, SecretSlot};
use crate::services::llm;
use crate::settings::SettingsState;
use tauri::State;

#[tauri::command]
pub fn list_profile(
    db: State<'_, InterviewDb>,
    user_id: String,
) -> Result<Vec<ProfileItem>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    db::list_profile_items(&conn, &user_id)
}

#[tauri::command]
pub fn save_profile_item(db: State<'_, InterviewDb>, item: ProfileItem) -> Result<(), String> {
    if item.id.trim().is_empty() || item.user_id.trim().is_empty() {
        return Err("profile item id/user_id required".into());
    }
    if !matches!(item.kind.as_str(), "summary" | "story" | "strength") {
        return Err(format!("unknown profile kind: {}", item.kind));
    }
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    db::upsert_profile_item(&conn, &item)
}

#[tauri::command]
pub fn delete_profile_item(
    db: State<'_, InterviewDb>,
    user_id: String,
    id: String,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    db::delete_profile_item(&conn, &user_id, &id)
}

const CV_PROMPT_BUDGET: usize = 12_000;
const JD_PROMPT_BUDGET: usize = 4_000;

/// Parse the LLM's profile draft. Returns unsaved items (fresh ids, caller's
/// `user_id`/`now`); unknown kinds are dropped rather than rejected so one
/// hallucinated row doesn't sink the whole draft.
fn parse_profile_draft(raw: &str, user_id: &str, now: &str) -> Result<Vec<ProfileItem>, String> {
    let slice = extract_json_array_slice(raw)
        .ok_or_else(|| "Profile draft contained no JSON array".to_string())?;
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(slice).map_err(|e| format!("profile draft JSON: {e}"))?;

    let mut seq = 0u32;
    let mut out = Vec::new();
    for row in rows {
        let kind = row.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if !matches!(kind, "summary" | "story" | "strength") {
            continue;
        }
        let content = row
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if content.is_empty() {
            continue;
        }
        let title = row
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        seq += 1;
        out.push(ProfileItem {
            id: format!("draft-{now}-{seq}"),
            user_id: user_id.to_string(),
            kind: kind.to_string(),
            title,
            content,
            updated_at: now.to_string(),
        });
    }
    if out.is_empty() {
        return Err("Profile draft produced no usable items".into());
    }
    Ok(out)
}

/// One LLM call: turn the uploaded CV (+ JD if present) into a profile draft —
/// a summary, STAR stories, and strengths the user reviews before saving.
#[tauri::command]
pub fn draft_profile_from_documents(
    db: State<'_, InterviewDb>,
    settings: State<'_, SettingsState>,
    user_id: String,
) -> Result<Vec<ProfileItem>, String> {
    let (llm_url, llm_model) = {
        let g = settings.0.lock().map_err(|e| e.to_string())?;
        (g.llm_url.clone(), g.llm_model.clone())
    };
    let llm_key = secrets::get_secret(SecretSlot::Llm)?.unwrap_or_default();
    if llm_url.trim().is_empty() || llm_model.trim().is_empty() || llm_key.trim().is_empty() {
        return Err("Configure the LLM in Settings → AI first.".to_string());
    }

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let docs = db::get_documents(&conn, &user_id)?;
    let mut cv = String::new();
    let mut jd = String::new();
    for (doc_type, text) in docs {
        match doc_type.as_str() {
            "cv" => cv = text,
            "jd" => jd = text,
            _ => {}
        }
    }
    if cv.trim().is_empty() {
        return Err("Upload a CV first (Settings → AI).".to_string());
    }
    let cv: String = cv.chars().take(CV_PROMPT_BUDGET).collect();
    let jd: String = jd.chars().take(JD_PROMPT_BUDGET).collect();
    let jd_block = if jd.trim().is_empty() {
        String::new()
    } else {
        format!("\n\nTarget job description (prefer stories relevant to it):\n{jd}")
    };

    let prompt = format!(
        "You are preparing an interview-answer profile for a candidate. Return ONLY a JSON array.\n\
        Items: {{\"kind\":\"summary\",\"title\":\"\",\"content\":\"<3–4 sentence first-person summary>\"}} — exactly once;\n\
        5–8 {{\"kind\":\"story\",\"title\":\"<≤8 words>\",\"content\":\"Situation: … Task: … Action: … Result: …\"}};\n\
        3–5 {{\"kind\":\"strength\",\"title\":\"<skill>\",\"content\":\"<one sentence evidence from the CV>\"}}.\n\
        Use only facts present in the CV; never invent experience. No markdown fences, no extra text.{jd_block}\n\n\
        Candidate CV:\n{cv}"
    );

    let client = crate::commands::interview::http_client()?;
    let raw = llm::complete_suggestions(&client, &llm_url, &llm_key, &llm_model, &prompt)?;
    let now = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
    parse_profile_draft(&raw, &user_id, &now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_parse_accepts_fenced_array() {
        let raw = "```json\n[{\"kind\":\"summary\",\"title\":\"\",\"content\":\"Senior backend engineer…\"},\n\
            {\"kind\":\"story\",\"title\":\"Fixed deadlock\",\"content\":\"Situation: x Task: y Action: z Result: w\"}]\n```";
        let items = parse_profile_draft(raw, "u", "now").unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, "summary");
        assert_eq!(items[1].title, "Fixed deadlock");
        assert!(items.iter().all(|i| i.user_id == "u" && !i.id.is_empty()));
    }

    #[test]
    fn draft_parse_rejects_non_json() {
        let err =
            parse_profile_draft("Here is your profile: a great engineer", "u", "now").unwrap_err();
        assert!(err.contains("JSON"), "unexpected error: {err}");
    }

    #[test]
    fn draft_parse_drops_unknown_kind() {
        let raw = "[{\"kind\":\"hobby\",\"title\":\"x\",\"content\":\"y\"},\
            {\"kind\":\"strength\",\"title\":\"Rust\",\"content\":\"5y systems work\"}]";
        let items = parse_profile_draft(raw, "u", "now").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, "strength");
    }
}
