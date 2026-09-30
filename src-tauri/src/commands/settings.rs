use crate::settings::{Settings, SettingsState};
use std::collections::BTreeSet;
use tauri::State;

/// Keys the frontend is allowed to send — derived at runtime from the
/// serialized field names of `Settings`, so it stays in sync automatically.
fn allowed_settings_fields() -> BTreeSet<String> {
    serde_json::to_value(Settings::default())
        .ok()
        .and_then(|v| v.as_object().map(|o| o.keys().cloned().collect()))
        .unwrap_or_default()
}

/// List payload keys that don't map to a `Settings` field.
fn unknown_fields(payload: &serde_json::Value) -> Vec<String> {
    let allowed = allowed_settings_fields();
    match payload.as_object() {
        Some(map) => map
            .keys()
            .filter(|k| !allowed.contains(*k))
            .cloned()
            .collect(),
        None => Vec::new(),
    }
}

/// Get current settings (with API keys hydrated from the secrets store)
#[tauri::command]
pub fn get_settings(state: State<'_, SettingsState>) -> Result<Settings, String> {
    let settings = state.0.lock().map_err(|e| format!("Lock error: {}", e))?;
    let mut out = settings.clone();
    out.hydrate_secrets();
    Ok(out)
}

/// Save settings. Unknown fields are rejected loudly — the previous lenient
/// deserialization silently dropped frontend keys (e.g. translation_type).
/// Load stays lenient so a hand-edited settings.json never wipes user prefs.
#[tauri::command]
pub fn save_settings(
    new_settings: serde_json::Value,
    state: State<'_, SettingsState>,
) -> Result<(), String> {
    let unknown = unknown_fields(&new_settings);
    if !unknown.is_empty() {
        return Err(format!(
            "Unknown settings field(s): {}. Update the Rust Settings struct or the frontend payload.",
            unknown.join(", ")
        ));
    }

    let new_settings: Settings = serde_json::from_value(new_settings)
        .map_err(|e| format!("Invalid settings payload: {}", e))?;

    let mut settings = state.0.lock().map_err(|e| format!("Lock error: {}", e))?;

    // Save to disk
    new_settings.save()?;

    // Update in-memory state
    *settings = new_settings;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_fields() {
        let payload = serde_json::json!({
            "source_language": "en",
            "target_language": "vi",
            "endpoint_delays": 1500,
            "brand_new_flag": true
        });
        let unknown = unknown_fields(&payload);
        assert_eq!(unknown, vec!["brand_new_flag", "endpoint_delays"]);
    }

    #[test]
    fn accepts_all_known_frontend_keys() {
        // Every key the frontend may send must be accepted.
        let payload = serde_json::json!({
            "soniox_api_key": "", "source_language": "auto", "target_language": "vi",
            "audio_source": "system", "overlay_opacity": 0.85, "font_family": "Inter",
            "font_size": 16, "max_lines": 5, "show_original": true,
            "translation_mode": "soniox", "translation_type": "one_way",
            "language_a": "ja", "language_b": "vi", "language_hints_strict": false,
            "endpoint_delay": 3000, "custom_context": null,
            "elevenlabs_api_key": "", "tts_enabled": false, "tts_provider": "edge",
            "tts_voice_id": "", "tts_speed": 1.2, "edge_tts_voice": "",
            "edge_tts_speed": 50, "tts_auto_read": true,
            "google_tts_api_key": "", "google_tts_voice": "", "google_tts_speed": 1.0,
            "pinecone_host": "", "pinecone_vector_dimension": 1536,
            "pinecone_api_key": "", "llm_url": "", "llm_model": "", "llm_api_key": "",
            "suggestion_type": "translation", "app_mode": null
        });
        assert!(unknown_fields(&payload).is_empty());
        serde_json::from_value::<Settings>(payload).expect("full frontend payload must parse");
    }
}
