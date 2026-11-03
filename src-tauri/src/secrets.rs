//! API keys stored in a dedicated secrets.json file in the app config directory.
//! Using file-based storage for cross-platform reliability (keyring v3 has
//! empty-blob read issues on Windows Credential Manager).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug)]
pub enum SecretSlot {
    Soniox,
    ElevenLabs,
    Google,
    Llm,
    Deepgram,
}

impl SecretSlot {
    fn key(self) -> &'static str {
        match self {
            SecretSlot::Soniox => "soniox_api_key",
            SecretSlot::ElevenLabs => "elevenlabs_api_key",
            SecretSlot::Google => "google_tts_api_key",
            SecretSlot::Llm => "llm_api_key",
            SecretSlot::Deepgram => "deepgram_api_key",
        }
    }

    pub fn parse(s: &str) -> Option<SecretSlot> {
        match s.to_lowercase().as_str() {
            "soniox" => Some(SecretSlot::Soniox),
            "elevenlabs" => Some(SecretSlot::ElevenLabs),
            "google" => Some(SecretSlot::Google),
            "llm" => Some(SecretSlot::Llm),
            "deepgram" => Some(SecretSlot::Deepgram),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct SecretsFile {
    #[serde(flatten)]
    keys: HashMap<String, String>,
}

fn secrets_path() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push("com.personal.translator");
    path.push("secrets.json");
    path
}

/// Load the store. A read error is returned (never treated as "empty") so a
/// following save can't wipe keys it failed to read. Unparseable content is
/// moved aside to `secrets.json.corrupt-<unix-ts>` for manual recovery, then
/// the store starts empty — the user re-enters keys instead of being stuck.
fn load_from(path: &Path) -> Result<SecretsFile, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SecretsFile::default()),
        Err(e) => return Err(format!("read secrets: {e}")),
    };
    match serde_json::from_str(&text) {
        Ok(store) => Ok(store),
        Err(parse_err) => {
            let ts = chrono::Utc::now().timestamp();
            let backup = path.with_extension(format!("json.corrupt-{ts}"));
            fs::rename(path, &backup).map_err(|e| format!("quarantine corrupt secrets: {e}"))?;
            tracing::warn!(
                "secrets.json unparseable ({parse_err}); moved to {}",
                backup.display()
            );
            Ok(SecretsFile::default())
        }
    }
}

/// Write via a temp file + rename so a crash mid-write never leaves a
/// truncated secrets.json behind.
fn save_to(path: &Path, store: &SecretsFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(store).map_err(|e| format!("serialize: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| format!("write secrets: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("replace secrets: {e}")
    })
}

pub fn set_secret(slot: SecretSlot, value: &str) -> Result<(), String> {
    let path = secrets_path();
    let mut store = load_from(&path)?;
    store.keys.insert(slot.key().to_string(), value.to_string());
    save_to(&path, &store)
}

pub fn get_secret(slot: SecretSlot) -> Result<Option<String>, String> {
    let store = load_from(&secrets_path())?;
    Ok(store
        .keys
        .get(slot.key())
        .filter(|v| !v.is_empty())
        .cloned())
}

pub fn delete_secret(slot: SecretSlot) -> Result<(), String> {
    let path = secrets_path();
    let mut store = load_from(&path)?;
    store.keys.remove(slot.key());
    save_to(&path, &store)
}

pub fn has_secret(slot: SecretSlot) -> Result<bool, String> {
    Ok(get_secret(slot)?.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_secrets_path() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("myjavis-secrets-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir.join("secrets.json")
    }

    fn store_with(key: &str, value: &str) -> SecretsFile {
        let mut store = SecretsFile::default();
        store.keys.insert(key.to_string(), value.to_string());
        store
    }

    #[test]
    fn missing_file_loads_as_empty_store() {
        let path = temp_secrets_path();
        let store = load_from(&path).unwrap();
        assert!(store.keys.is_empty());
    }

    #[test]
    fn saved_store_round_trips() {
        let path = temp_secrets_path();
        save_to(&path, &store_with("llm_api_key", "k1")).unwrap();
        let store = load_from(&path).unwrap();
        assert_eq!(
            store.keys.get("llm_api_key").map(String::as_str),
            Some("k1")
        );
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let path = temp_secrets_path();
        save_to(&path, &store_with("llm_api_key", "k1")).unwrap();
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name() != "secrets.json")
            .collect();
        assert!(leftovers.is_empty(), "unexpected files: {leftovers:?}");
    }

    #[test]
    fn corrupted_file_is_quarantined_not_overwritten() {
        let path = temp_secrets_path();
        let corrupted = "{\"soniox_api_key\": \"keep-me\", trunc";
        fs::write(&path, corrupted).unwrap();

        let store = load_from(&path).unwrap();
        assert!(store.keys.is_empty());
        // A later save must not destroy the only copy of the old keys.
        save_to(&path, &store_with("llm_api_key", "new")).unwrap();

        let quarantined: Vec<String> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".corrupt-"))
            .map(|e| fs::read_to_string(e.path()).unwrap())
            .collect();
        assert_eq!(quarantined, vec![corrupted.to_string()]);
    }
}
