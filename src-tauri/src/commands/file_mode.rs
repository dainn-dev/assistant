//! File mode: media file → Deepgram transcript → LLM translation → two .srt
//! files written next to the input.

use crate::commands::interview::run_blocking;
use crate::commands::translate;
use crate::secrets::{self, SecretSlot};
use crate::services::deepgram;
use crate::services::media_decode::{decode_file_to_pcm16k, encode_wav, TARGET_RATE};
use crate::services::subtitles::{to_srt, SubtitleCue};
use crate::settings::{Settings, SettingsState};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};

/// Upload + server-side transcription of a long recording can take minutes.
const DEEPGRAM_FILE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Serialize)]
pub struct FileProgress {
    /// "decoding" | "transcribing" | "translating" | "writing" | "done"
    pub stage: &'static str,
    pub current: usize,
    pub total: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTranscriptResult {
    pub original_srt: String,
    pub translated_srt: String,
    pub cues: usize,
    pub audio_seconds: f64,
}

#[tauri::command]
pub async fn transcribe_media_file(
    app: AppHandle,
    path: String,
    channel: Channel<FileProgress>,
) -> Result<FileTranscriptResult, String> {
    run_blocking(move || {
        let settings = {
            let state = app.state::<SettingsState>();
            let guard = state.0.lock().map_err(|e| e.to_string())?;
            guard.clone()
        };
        transcribe_file_blocking(&settings, Path::new(&path), &channel)
    })
    .await
}

fn transcribe_file_blocking(
    settings: &Settings,
    path: &Path,
    channel: &Channel<FileProgress>,
) -> Result<FileTranscriptResult, String> {
    let progress = |stage: &'static str, current: usize, total: usize| {
        let _ = channel.send(FileProgress {
            stage,
            current,
            total,
        });
    };
    if !path.is_file() {
        return Err(format!("File not found: {}", path.display()));
    }
    let key = secrets::get_secret(SecretSlot::Deepgram)?.unwrap_or_default();
    if key.trim().is_empty() {
        return Err(
            "File mode transcribes with Deepgram — add a Deepgram API key in Settings → Translation."
                .to_string(),
        );
    }
    // Fail before the slow upload if translation can't run afterwards.
    translate::llm_key(settings)?;

    progress("decoding", 0, 1);
    let pcm = decode_file_to_pcm16k(path)?;
    let audio_seconds = pcm.len() as f64 / TARGET_RATE as f64;
    let wav = encode_wav(&pcm, TARGET_RATE, 1);
    drop(pcm);

    progress("transcribing", 0, 1);
    let client = reqwest::blocking::Client::builder()
        .timeout(DEEPGRAM_FILE_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let query = deepgram::prerecorded_query(
        &settings.deepgram_model,
        &settings.translation_type,
        &settings.source_language,
    );
    let cues = deepgram::transcribe_prerecorded(&client, &key, &query, wav)?;
    if cues.is_empty() {
        return Err("No speech was found in the file.".to_string());
    }

    let lines: Vec<String> = cues.iter().map(|c| c.text.clone()).collect();
    progress("translating", 0, lines.len());
    let translated = translate::translate_lines_blocking(settings, &lines, |done, total| {
        progress("translating", done, total)
    })?;
    let translated_cues: Vec<SubtitleCue> = cues
        .iter()
        .zip(translated)
        .map(|(c, text)| SubtitleCue { text, ..c.clone() })
        .collect();

    progress("writing", 0, 1);
    let (original_path, translated_path) = srt_output_paths(path, settings);
    std::fs::write(&original_path, to_srt(&cues))
        .map_err(|e| format!("Write {}: {e}", original_path.display()))?;
    std::fs::write(&translated_path, to_srt(&translated_cues))
        .map_err(|e| format!("Write {}: {e}", translated_path.display()))?;
    progress("done", 1, 1);

    Ok(FileTranscriptResult {
        original_srt: original_path.display().to_string(),
        translated_srt: translated_path.display().to_string(),
        cues: cues.len(),
        audio_seconds,
    })
}

/// `<stem>.<source>.srt` + `<stem>.<target>.srt` next to the input; generic
/// suffixes when the language isn't fixed (auto-detect, two-way).
fn srt_output_paths(input: &Path, settings: &Settings) -> (PathBuf, PathBuf) {
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "subtitles".to_string());
    let one_way = settings.translation_type != "two_way";
    let source = if one_way && settings.source_language != "auto" {
        settings.source_language.as_str()
    } else {
        "original"
    };
    let target = if one_way && settings.target_language != source {
        settings.target_language.as_str()
    } else {
        "translated"
    };
    let dir = input.parent().unwrap_or_else(|| Path::new("."));
    (
        dir.join(format!("{stem}.{source}.srt")),
        dir.join(format!("{stem}.{target}.srt")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use std::path::PathBuf;

    fn settings(translation_type: &str, source: &str, target: &str) -> Settings {
        Settings {
            translation_type: translation_type.into(),
            source_language: source.into(),
            target_language: target.into(),
            ..Default::default()
        }
    }

    #[test]
    fn one_way_outputs_are_named_by_language() {
        let (orig, tr) =
            srt_output_paths(Path::new("/v/talk.mp4"), &settings("one_way", "ja", "vi"));
        assert_eq!(orig, PathBuf::from("/v/talk.ja.srt"));
        assert_eq!(tr, PathBuf::from("/v/talk.vi.srt"));
    }

    #[test]
    fn auto_source_and_two_way_use_generic_suffixes() {
        let (orig, tr) =
            srt_output_paths(Path::new("/v/a.mp3"), &settings("one_way", "auto", "en"));
        assert_eq!(orig, PathBuf::from("/v/a.original.srt"));
        assert_eq!(tr, PathBuf::from("/v/a.en.srt"));
        let (orig, tr) = srt_output_paths(Path::new("/v/a.mp3"), &settings("two_way", "ja", "vi"));
        assert_eq!(orig, PathBuf::from("/v/a.original.srt"));
        assert_eq!(tr, PathBuf::from("/v/a.translated.srt"));
    }

    #[test]
    fn identical_languages_never_overwrite_the_original() {
        let (orig, tr) = srt_output_paths(Path::new("/v/a.wav"), &settings("one_way", "en", "en"));
        assert_ne!(orig, tr);
    }
}
