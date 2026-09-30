use crate::secrets::{self, SecretSlot};
use serde::{Deserialize, Deserializer, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

/// Translation term: source → target mapping for Soniox
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TranslationTerm {
    pub source: String,
    pub target: String,
}

/// Custom context for Soniox — provides domain-specific hints
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct CustomContext {
    pub domain: Option<String>,
    pub translation_terms: Vec<TranslationTerm>,
}

/// Deserialize `null` as `T::default()` — JS DEFAULT_SETTINGS sends null for
/// fields like app_mode, and strict save validation must still accept them.
fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + Deserialize<'de>,
    D: Deserializer<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// App settings — persisted to JSON
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    /// Soniox API key
    pub soniox_api_key: String,
    /// Source language: "auto" or ISO 639-1 code
    pub source_language: String,
    /// Target language: ISO 639-1 code
    pub target_language: String,
    /// Audio source: "system" | "microphone" | "both"
    pub audio_source: String,
    /// Overlay opacity: 0.0 - 1.0
    pub overlay_opacity: f64,
    /// Font family name (CSS font-family string)
    pub font_family: String,
    /// Font size in px
    pub font_size: u32,
    /// Transcript text color (CSS color string)
    pub font_color: String,
    /// Max transcript lines to display
    pub max_lines: u32,
    /// Whether to show original text alongside translation
    pub show_original: bool,
    /// Translation mode: "soniox" (cloud API) or "local" (MLX models)
    pub translation_mode: String,
    /// Translation type: "one_way" | "two_way"
    pub translation_type: String,
    /// Two-way mode language A (ISO 639-1 code)
    pub language_a: String,
    /// Two-way mode language B (ISO 639-1 code)
    pub language_b: String,
    /// Restrict recognition to language hints only
    pub language_hints_strict: bool,
    /// Max endpoint delay in ms sent to Soniox
    pub endpoint_delay: u32,
    /// Optional custom context for better transcription
    pub custom_context: Option<CustomContext>,
    /// ElevenLabs API key for TTS narration
    pub elevenlabs_api_key: String,
    /// Whether TTS narration is enabled
    pub tts_enabled: bool,
    /// TTS provider: "edge" | "elevenlabs" | "google"
    pub tts_provider: String,
    /// ElevenLabs voice ID
    pub tts_voice_id: String,
    /// TTS speed multiplier (Web Speech)
    pub tts_speed: f64,
    /// Edge TTS voice name
    pub edge_tts_voice: String,
    /// Edge TTS speed percentage
    pub edge_tts_speed: i32,
    /// Auto-read new translations aloud
    pub tts_auto_read: bool,
    /// Google Cloud TTS API key
    pub google_tts_api_key: String,
    /// Google TTS voice name
    pub google_tts_voice: String,
    /// Google TTS speaking rate
    pub google_tts_speed: f64,

    // ─── Interview RAG ───
    /// Pinecone index host
    pub pinecone_host: String,
    /// Expected vector dimension for the Pinecone index
    pub pinecone_vector_dimension: u32,
    /// LLM chat completions endpoint (OpenAI-compatible)
    pub llm_url: String,
    /// LLM model name
    pub llm_model: String,
    /// LLM API key
    pub llm_api_key: String,
    /// Pinecone API key
    pub pinecone_api_key: String,
    /// Interview suggestions language mode: "target" (source_language), "translation", "both"
    pub suggestion_type: String,
    /// App mode: "Interview" | "Meeting" | "" (none)
    /// Frontend sends null; tolerate it as the default.
    #[serde(deserialize_with = "deserialize_null_default")]
    pub app_mode: String,
    /// Proactively stream answer hints while the interviewer is still
    /// speaking (Interview mode, Soniox + system/mic audio only).
    pub early_suggestions: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            soniox_api_key: String::new(),
            source_language: "auto".to_string(),
            target_language: "vi".to_string(),
            audio_source: "system".to_string(),
            overlay_opacity: 0.85,
            font_family: "Inter".to_string(),
            font_size: 16,
            font_color: "#ffffff".to_string(),
            max_lines: 5,
            show_original: true,
            translation_mode: "soniox".to_string(),
            translation_type: "one_way".to_string(),
            language_a: "ja".to_string(),
            language_b: "vi".to_string(),
            language_hints_strict: false,
            endpoint_delay: 3000,
            custom_context: None,
            elevenlabs_api_key: String::new(),
            tts_enabled: false,
            tts_provider: "edge".to_string(),
            tts_voice_id: "21m00Tcm4TlvDq8ikWAM".to_string(),
            tts_speed: 1.2,
            edge_tts_voice: "vi-VN-HoaiMyNeural".to_string(),
            edge_tts_speed: 50,
            tts_auto_read: true,
            google_tts_api_key: String::new(),
            google_tts_voice: "vi-VN-Chirp3-HD-Aoede".to_string(),
            google_tts_speed: 1.0,
            pinecone_host: String::new(),
            pinecone_vector_dimension: 1536,
            llm_url: String::new(),
            llm_model: String::new(),
            llm_api_key: String::new(),
            pinecone_api_key: String::new(),
            suggestion_type: "translation".to_string(),
            app_mode: "Interview".to_string(),
            early_suggestions: false,
        }
    }
}

/// Get the settings file path
/// ~/Library/Application Support/com.personal.translator/settings.json
fn settings_path() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push("com.personal.translator");
    path.push("settings.json");
    path
}

impl Settings {
    /// Mutable access to each API-key field paired with its secrets slot.
    fn secret_fields_mut(&mut self) -> [(SecretSlot, &mut String); 5] {
        [
            (SecretSlot::Soniox, &mut self.soniox_api_key),
            (SecretSlot::ElevenLabs, &mut self.elevenlabs_api_key),
            (SecretSlot::Google, &mut self.google_tts_api_key),
            (SecretSlot::Llm, &mut self.llm_api_key),
            (SecretSlot::Pinecone, &mut self.pinecone_api_key),
        ]
    }

    /// Blank out all API-key fields (called before serializing to settings.json).
    #[allow(dead_code)]
    fn clear_secret_fields(&mut self) {
        for (_, field) in self.secret_fields_mut() {
            field.clear();
        }
    }

    /// Fill API-key fields from the secrets store.
    pub fn hydrate_secrets(&mut self) {
        for (slot, field) in self.secret_fields_mut() {
            if let Ok(Some(value)) = secrets::get_secret(slot) {
                *field = value;
            }
        }
    }

    /// Persist each key field to the secrets store (empty value = delete),
    /// and return a copy of self with the keys blanked for settings.json.
    fn offload_secrets(&self) -> Result<Settings, String> {
        let mut stripped = self.clone();
        for (slot, field) in stripped.secret_fields_mut() {
            let value = std::mem::take(field);
            if value.trim().is_empty() {
                secrets::delete_secret(slot)?;
            } else {
                secrets::set_secret(slot, value.trim())?;
            }
        }
        Ok(stripped)
    }

    /// Load settings from disk, or return defaults.
    /// Legacy installs stored API keys inside settings.json — migrate them to
    /// secrets.json once, then hydrate the returned struct from the store.
    pub fn load() -> Self {
        let path = settings_path();
        let mut settings = if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
                Err(_) => Self::default(),
            }
        } else {
            Self::default()
        };

        // Migrate any keys still embedded in settings.json
        let mut migrated = false;
        for (slot, field) in settings.secret_fields_mut() {
            let value = std::mem::take(field);
            if !value.trim().is_empty() {
                if secrets::set_secret(slot, value.trim()).is_ok() {
                    migrated = true;
                } else {
                    // Secrets store unavailable — keep the key in settings so it isn't lost
                    *field = value;
                }
            }
        }
        if migrated {
            let _ = fs::write(
                &path,
                serde_json::to_string_pretty(&settings).unwrap_or_default(),
            );
        }

        settings.hydrate_secrets();
        settings
    }

    /// Save settings to disk. API keys go to the secrets store, not settings.json.
    pub fn save(&self) -> Result<(), String> {
        let path = settings_path();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config dir: {}", e))?;
        }

        let stripped = self.offload_secrets()?;
        let json = serde_json::to_string_pretty(&stripped)
            .map_err(|e| format!("Failed to serialize: {}", e))?;

        fs::write(&path, json).map_err(|e| format!("Failed to write settings: {}", e))?;

        Ok(())
    }
}

/// Thread-safe settings state managed by Tauri
pub struct SettingsState(pub Mutex<Settings>);

#[cfg(test)]
mod tests {
    use super::*;

    /// The frontend sends every field it manages; nothing may be silently dropped.
    #[test]
    fn deserializes_full_frontend_payload() {
        let json = r#"{
            "soniox_api_key": "sk-soniox",
            "source_language": "auto",
            "target_language": "vi",
            "audio_source": "both",
            "overlay_opacity": 0.5,
            "font_family": "Inter",
            "font_size": 20,
            "font_color": "gold",
            "max_lines": 8,
            "show_original": false,
            "translation_mode": "soniox",
            "translation_type": "two_way",
            "language_a": "en",
            "language_b": "ja",
            "language_hints_strict": true,
            "endpoint_delay": 1500,
            "custom_context": null,
            "elevenlabs_api_key": "el-key",
            "tts_enabled": false,
            "tts_provider": "google",
            "tts_voice_id": "voice1",
            "tts_speed": 1.5,
            "edge_tts_voice": "vi-VN-HoaiMyNeural",
            "edge_tts_speed": 30,
            "tts_auto_read": true,
            "google_tts_api_key": "g-key",
            "google_tts_voice": "vi-VN-Chirp3-HD-Aoede",
            "google_tts_speed": 1.1,
            "pinecone_host": "https://idx.pinecone.io",
            "pinecone_vector_dimension": 1536,
            "llm_url": "https://api.openai.com/v1/chat/completions",
            "llm_model": "gpt-4o-mini",
            "llm_api_key": "llm-key",
            "pinecone_api_key": "pc-key",
            "suggestion_type": "both",
            "app_mode": "Interview",
            "early_suggestions": true
        }"#;
        let s: Settings = serde_json::from_str(json).expect("payload must deserialize");

        assert_eq!(s.translation_type, "two_way");
        assert_eq!(s.language_a, "en");
        assert_eq!(s.language_b, "ja");
        assert!(s.language_hints_strict);
        assert_eq!(s.endpoint_delay, 1500);
        assert_eq!(s.soniox_api_key, "sk-soniox");
        assert_eq!(s.llm_api_key, "llm-key");
        assert_eq!(s.pinecone_api_key, "pc-key");
        assert_eq!(s.app_mode, "Interview");
        assert_eq!(s.suggestion_type, "both");
        assert!(s.early_suggestions);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s: Settings = serde_json::from_str("{}").expect("empty object must deserialize");
        assert_eq!(s.translation_type, "one_way");
        assert_eq!(s.language_a, "ja");
        assert_eq!(s.language_b, "vi");
        assert!(!s.language_hints_strict);
        assert_eq!(s.endpoint_delay, 3000);
        assert!(s.soniox_api_key.is_empty());
    }

    /// Keys must never be serialized into settings.json.
    #[test]
    fn persisted_json_contains_no_api_keys() {
        let s = Settings {
            soniox_api_key: "secret-soniox".into(),
            elevenlabs_api_key: "secret-el".into(),
            google_tts_api_key: "secret-google".into(),
            llm_api_key: "secret-llm".into(),
            pinecone_api_key: "secret-pc".into(),
            ..Default::default()
        };

        let mut clone = s.clone();
        clone.clear_secret_fields();
        let json = serde_json::to_string(&clone).unwrap();

        for key in [
            "secret-soniox",
            "secret-el",
            "secret-google",
            "secret-llm",
            "secret-pc",
        ] {
            assert!(!json.contains(key), "persisted JSON leaked {key}");
        }
    }
}
