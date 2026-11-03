//! Helpers shared by command modules: blocking-pool runner, HTTP client,
//! and tolerant JSON slicing of LLM replies.

use std::time::Duration;

/// Run blocking work (reqwest::blocking, SQLite, file parsing) on the blocking
/// pool. Sync `#[tauri::command]`s execute on the main thread and freeze the
/// window for the whole network round-trip; `#[command(async)]` is not enough
/// either — reqwest::blocking panics on a tokio worker thread.
pub(crate) async fn run_blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("background task failed: {e}"))?
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
