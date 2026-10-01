use chrono::Local;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// Get the transcript directory path
pub(crate) fn transcript_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?
        .join("transcripts");

    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create transcript dir: {}", e))?;
    Ok(dir)
}

/// Save a transcript session to a file.
/// Also writes a `<name>.segments.json` sidecar with the structured segment
/// list so the read-only view doesn't have to re-parse markdown.
///
/// `filename` is optional: passing a name returned by an earlier call
/// overwrites that file (used for per-session autosave so repeated stops
/// keep writing to the same artifact). A missing/None filename creates a
/// new timestamped file; same-second collisions get a `-N` suffix.
/// Called when user clicks "Clear", stops recording, or closes app
#[tauri::command]
pub fn save_transcript(
    app: AppHandle,
    content: String,
    segments: Option<serde_json::Value>,
    filename: Option<String>,
) -> Result<String, String> {
    let dir = transcript_dir(&app)?;
    let filename = match filename {
        Some(name) => {
            // Same sanitization as read/delete: basename only, .md only
            if name.contains('/')
                || name.contains('\\')
                || name.contains("..")
                || !name.ends_with(".md")
            {
                return Err("Invalid filename".to_string());
            }
            name
        }
        None => {
            let base = Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
            let mut candidate = format!("{}.md", base);
            let mut n = 1;
            while dir.join(&candidate).exists() {
                candidate = format!("{}-{}.md", base, n);
                n += 1;
            }
            candidate
        }
    };
    let filepath = dir.join(&filename);

    fs::write(&filepath, content).map_err(|e| format!("Failed to save transcript: {}", e))?;
    tracing::info!("transcript saved to {}", filepath.display());

    if let Some(segments) = segments {
        if let Ok(json) = serde_json::to_string_pretty(&segments) {
            // Best-effort — the .md is the canonical artifact
            let _ = fs::write(filepath.with_extension("segments.json"), json);
        }
    }

    Ok(filepath.to_string_lossy().to_string())
}

/// Open the transcript directory in the system file manager
/// macOS: Finder, Windows: Explorer
#[tauri::command]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn open_transcript_dir(app: AppHandle) -> Result<(), String> {
    let dir = transcript_dir(&app)?;

    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";

    std::process::Command::new(cmd)
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("Failed to open transcript dir: {}", e))?;
    Ok(())
}

#[tauri::command]
#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn open_transcript_dir(_app: AppHandle) -> Result<(), String> {
    Err("open_transcript_dir is not supported on mobile.".to_string())
}

#[derive(Serialize)]
pub struct TranscriptEntry {
    filename: String,
    path: String,
    created_at: String,
    size_bytes: u64,
    /// True when a `<name>.review.json` sidecar exists (post-session review).
    has_review: bool,
}

/// List all saved transcript sessions, newest first
#[tauri::command]
pub fn list_transcripts(app: AppHandle) -> Result<Vec<TranscriptEntry>, String> {
    let dir = transcript_dir(&app)?;

    let mut entries: Vec<TranscriptEntry> = fs::read_dir(&dir)
        .map_err(|e| format!("Failed to read transcript dir: {}", e))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let filename = entry.file_name().to_string_lossy().to_string();
            if !filename.ends_with(".md") {
                return None;
            }
            let path = entry.path().to_string_lossy().to_string();
            let size_bytes = entry.metadata().ok()?.len();
            // Parse created_at from filename: YYYY-MM-DD_HH-MM-SS.md
            let _created_at = filename
                .strip_suffix(".md")
                .unwrap_or(&filename)
                .replace('_', " ")
                .replace('-', ":")
                // Fix date separator: first two colons are date separators
                // Transform "2026:03:27 10:21:05" → "2026-03-27 10:21:05"
                .to_string();
            // More accurate: split on space, fix date part
            let created_at = {
                let base = filename.strip_suffix(".md").unwrap_or(&filename);
                // base = "2026-03-27_10-21-05"
                let parts: Vec<&str> = base.splitn(2, '_').collect();
                if parts.len() == 2 {
                    let time_part = parts[1].replace('-', ":");
                    format!("{} {}", parts[0], time_part)
                } else {
                    base.to_string()
                }
            };
            let has_review = entry.path().with_extension("review.json").exists();
            Some(TranscriptEntry {
                filename,
                path,
                created_at,
                size_bytes,
                has_review,
            })
        })
        .collect();

    // Sort by filename descending (newest first — filenames are timestamps)
    entries.sort_by(|a, b| b.filename.cmp(&a.filename));

    Ok(entries)
}

/// Read the structured segments sidecar for a saved transcript, if present.
/// Returns null for transcripts saved before sidecars existed — the frontend
/// falls back to markdown parsing in that case.
#[tauri::command]
pub fn read_transcript_segments(
    app: AppHandle,
    filename: String,
) -> Result<Option<serde_json::Value>, String> {
    // Sanitize: no path traversal, and require transcript files only
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err("Invalid filename".to_string());
    }
    if !filename.ends_with(".md") {
        return Err("Invalid filename".to_string());
    }

    let dir = transcript_dir(&app)?;
    let sidecar = dir.join(&filename).with_extension("segments.json");
    if !sidecar.exists() {
        return Ok(None);
    }

    let text = fs::read_to_string(&sidecar)
        .map_err(|e| format!("Failed to read transcript segments: {}", e))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| format!("Failed to parse transcript segments: {}", e))
}

/// Read the content of a saved transcript file
#[tauri::command]
pub fn read_transcript(app: AppHandle, filename: String) -> Result<String, String> {
    // Sanitize: no path traversal
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err("Invalid filename".to_string());
    }
    let dir = transcript_dir(&app)?;
    let filepath = dir.join(&filename);
    fs::read_to_string(&filepath).map_err(|e| format!("Failed to read transcript: {}", e))
}

/// Delete a saved transcript file
#[tauri::command]
pub fn delete_transcript(app: AppHandle, filename: String) -> Result<(), String> {
    // Sanitize: no path traversal, and require transcript files only
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err("Invalid filename".to_string());
    }
    if !filename.ends_with(".md") {
        return Err("Invalid filename".to_string());
    }

    let dir = transcript_dir(&app)?;
    let filepath = dir.join(&filename);

    // Remove the segments + review sidecars too (best-effort)
    let _ = fs::remove_file(filepath.with_extension("segments.json"));
    let _ = fs::remove_file(filepath.with_extension("review.json"));

    match fs::remove_file(&filepath) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Failed to delete transcript: {}", e)),
    }
}
