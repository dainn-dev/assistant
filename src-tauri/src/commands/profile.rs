//! Candidate profile CRUD — persistent summary / STAR stories / strengths that
//! join CV/JD excerpts in grounding interview suggestions.

use crate::db::{self, InterviewDb, ProfileItem};
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
