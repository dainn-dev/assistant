//! Post-session review — one LLM call over a saved interview transcript,
//! cached next to the transcript as `<name>.review.json`.

use crate::commands::util::{extract_json_object_slice, http_client, run_blocking};
use crate::copilot::presets::{self, CopilotPreset, ReviewKind};
use crate::copilot::prompts;
use crate::secrets::{self, SecretSlot};
use crate::services::llm;
use crate::settings::SettingsState;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const REVIEW_DIALOGUE_MAX_CHARS: usize = 16_000;

fn review_path(dir: &Path, filename: &str) -> PathBuf {
    dir.join(filename).with_extension("review.json")
}

fn write_review(dir: &Path, filename: &str, review: &SessionReview) -> Result<(), String> {
    let json =
        serde_json::to_string_pretty(review).map_err(|e| format!("serialize review: {e}"))?;
    fs::write(review_path(dir, filename), json).map_err(|e| format!("write review: {e}"))
}

fn read_review(dir: &Path, filename: &str) -> Result<Option<SessionReview>, String> {
    let path = review_path(dir, filename);
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("read review: {e}"))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| format!("parse review: {e}"))
}

fn valid_md_filename(filename: &str) -> Result<(), String> {
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err("Invalid filename".to_string());
    }
    if !filename.ends_with(".md") {
        return Err("Invalid filename".to_string());
    }
    Ok(())
}

/// Cached review for a saved transcript, if any.
#[tauri::command]
pub fn read_session_review(
    app: AppHandle,
    filename: String,
) -> Result<Option<SessionReview>, String> {
    valid_md_filename(&filename)?;
    let dir = crate::commands::transcript::transcript_dir(&app)?;
    read_review(&dir, &filename)
}

/// Generate a post-session review — one LLM call, cached as
/// `<name>.review.json`. `force: true` regenerates regardless of cache.
#[tauri::command]
pub async fn review_session(
    app: AppHandle,
    filename: String,
    force: bool,
) -> Result<SessionReview, String> {
    run_blocking(move || {
        let settings = app.state::<SettingsState>();
        review_session_blocking(&app, &settings, filename, force)
    })
    .await
}

fn review_session_blocking(
    app: &AppHandle,
    settings: &SettingsState,
    filename: String,
    force: bool,
) -> Result<SessionReview, String> {
    valid_md_filename(&filename)?;
    let dir = crate::commands::transcript::transcript_dir(app)?;

    if !force {
        if let Some(cached) = read_review(&dir, &filename)? {
            return Ok(cached);
        }
    }

    let sidecar = dir.join(&filename).with_extension("segments.json");
    let segments: serde_json::Value = if sidecar.exists() {
        let text = fs::read_to_string(&sidecar).map_err(|e| format!("read segments: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("parse segments: {e}"))?
    } else {
        return Err("No structured transcript — this session predates sidecars.".to_string());
    };

    let preset = session_preset(&segments);
    let (dialogue, roles_known, has_candidate) =
        build_review_dialogue(&segments, REVIEW_DIALOGUE_MAX_CHARS, preset);
    if dialogue.trim().is_empty() {
        return Err("Transcript is empty — nothing to review.".to_string());
    }

    let (llm_url, llm_model) = {
        let g = settings.0.lock().map_err(|e| e.to_string())?;
        (g.llm_url.clone(), g.llm_model.clone())
    };
    let llm_key = secrets::get_secret(SecretSlot::Llm)?.unwrap_or_default();
    if llm_url.trim().is_empty() || llm_model.trim().is_empty() || llm_key.trim().is_empty() {
        return Err("Configure the LLM in Settings → AI first.".to_string());
    }

    let prompt = prompts::review_prompt(preset, roles_known, has_candidate, &dialogue);

    let client = http_client()?;
    // Review JSON holds up to ~5 scored questions + practice list — a small
    // max_tokens would cut the object mid-stream and fail JSON parsing.
    let raw = llm::complete_suggestions(&client, &llm_url, &llm_key, &llm_model, &prompt, 4000)?;
    let now = chrono::Utc::now().to_rfc3339();
    let review = match preset.review {
        ReviewKind::Scored => parse_review(&raw, &llm_model, roles_known, has_candidate, &now)?,
        ReviewKind::Minutes => parse_minutes(&raw, &llm_model, roles_known, &now)?,
    };
    write_review(&dir, &filename, &review)?;
    Ok(review)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewQuestion {
    pub question: String,
    pub answer_summary: String,
    /// 1–5 when candidate audio exists; null when the session captured the
    /// interviewer only.
    pub score: Option<u8>,
    pub feedback: String,
    pub stronger_answer: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionItem {
    pub task: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub due: String,
}

fn scored_kind() -> String {
    "scored".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionReview {
    pub version: u32,
    /// "scored" (interview) | "minutes" (meeting). Reviews cached before
    /// presets have no kind and are interview reviews.
    #[serde(default = "scored_kind")]
    pub kind: String,
    pub generated_at: String,
    pub model: String,
    /// False when the transcript predates source tagging — the model then
    /// infers roles from content and should say so in `overall`.
    pub roles_known: bool,
    pub overall: String,
    #[serde(default)]
    pub questions: Vec<ReviewQuestion>,
    #[serde(default)]
    pub practice: Vec<String>,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub action_items: Vec<ActionItem>,
    #[serde(default)]
    pub open_questions: Vec<String>,
}

/// The copilot preset a saved session ran under (sidecar `copilot` field).
/// Sessions saved before presets carry none and were interviews.
pub fn session_preset(segments: &serde_json::Value) -> &'static CopilotPreset {
    segments
        .get("copilot")
        .and_then(|v| v.as_str())
        .map(presets::preset_for_mode)
        .unwrap_or(&presets::INTERVIEW)
}

/// Turn a saved sidecar (bare array or `{version, segments, metrics}`) into a
/// dialogue for the review prompt, labelling speakers with the preset's roles.
/// Returns (dialogue, roles_known, has_self_audio). Keeps the LAST
/// `max_chars` — recent turns matter most.
pub fn build_review_dialogue(
    segments: &serde_json::Value,
    max_chars: usize,
    preset: &CopilotPreset,
) -> (String, bool, bool) {
    let arr = match segments {
        serde_json::Value::Array(a) => a.clone(),
        v @ serde_json::Value::Object(_) => v
            .get("segments")
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    let mut roles_known = false;
    let mut has_candidate = false;
    let mut lines: Vec<String> = Vec::new();
    for seg in &arr {
        let text = seg
            .get("original")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if text.is_empty() {
            continue;
        }
        let source = seg.get("source").and_then(|v| v.as_str());
        let label = match source {
            Some("system") => preset.remote_label.to_string(),
            Some("mic") => preset.self_label.to_string(),
            _ => {
                let sp = seg.get("speaker").and_then(|v| v.as_str()).unwrap_or("?");
                format!("Speaker {sp}")
            }
        };
        if source.is_some() {
            roles_known = true;
        }
        if source == Some("mic") {
            has_candidate = true;
        }
        lines.push(format!("{label}: {text}"));
    }

    // Keep the tail, cut at a line boundary.
    let mut out = String::new();
    for line in lines.iter().rev() {
        if out.len() + line.len() + 1 > max_chars && !out.is_empty() {
            break;
        }
        out = if out.is_empty() {
            line.clone()
        } else {
            format!("{line}\n{out}")
        };
    }
    (out, roles_known, has_candidate)
}

/// Parse the model's review JSON tolerantly: prose fences allowed, out-of-range
/// scores → null, all scores null when the session had no candidate audio.
pub fn parse_review(
    raw: &str,
    model: &str,
    roles_known: bool,
    has_candidate: bool,
    now: &str,
) -> Result<SessionReview, String> {
    let slice = extract_json_object_slice(raw)
        .ok_or_else(|| "Review contained no JSON object".to_string())?;
    let v: serde_json::Value =
        serde_json::from_str(slice).map_err(|e| format!("review JSON: {e}"))?;

    let mut questions = Vec::new();
    for q in v
        .get("questions")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let text = |k: &str| {
            q.get(k)
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let score = match (q.get("score").and_then(|x| x.as_u64()), has_candidate) {
            (Some(n), true) if (1..=5).contains(&n) => Some(n as u8),
            _ => None,
        };
        questions.push(ReviewQuestion {
            question: text("question"),
            answer_summary: text("answer_summary"),
            score,
            feedback: text("feedback"),
            stronger_answer: text("stronger_answer"),
        });
    }

    let practice = v
        .get("practice")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| p.as_str().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .take(5)
        .collect();

    Ok(SessionReview {
        version: 1,
        kind: scored_kind(),
        generated_at: now.to_string(),
        model: model.to_string(),
        roles_known,
        overall: json_str(&v, "overall"),
        questions,
        practice,
        decisions: Vec::new(),
        action_items: Vec::new(),
        open_questions: Vec::new(),
    })
}

fn json_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Non-empty trimmed strings of a JSON array field.
fn json_str_list(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|p| p.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Parse meeting minutes tolerantly: prose fences allowed, blank entries and
/// action items without a task dropped.
pub fn parse_minutes(
    raw: &str,
    model: &str,
    roles_known: bool,
    now: &str,
) -> Result<SessionReview, String> {
    let slice = extract_json_object_slice(raw)
        .ok_or_else(|| "Review contained no JSON object".to_string())?;
    let v: serde_json::Value =
        serde_json::from_str(slice).map_err(|e| format!("review JSON: {e}"))?;
    let action_items = v
        .get("action_items")
        .and_then(|x| x.as_array())
        .map(|items| {
            items
                .iter()
                .map(|item| ActionItem {
                    task: json_str(item, "task"),
                    owner: json_str(item, "owner"),
                    due: json_str(item, "due"),
                })
                .filter(|a| !a.task.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Ok(SessionReview {
        version: 1,
        kind: "minutes".to_string(),
        generated_at: now.to_string(),
        model: model.to_string(),
        roles_known,
        overall: json_str(&v, "overall"),
        questions: Vec::new(),
        practice: Vec::new(),
        decisions: json_str_list(&v, "decisions"),
        action_items,
        open_questions: json_str_list(&v, "open_questions"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dialogue_labels_system_and_mic() {
        let segs = json!([
            {"source": "system", "speaker": "1", "original": "Tell me about deadlock"},
            {"source": "mic", "speaker": "2", "original": "I ordered the locks"},
        ]);
        let (d, roles_known, has_candidate) =
            build_review_dialogue(&segs, 16_000, &presets::INTERVIEW);
        assert!(d.contains("Interviewer: Tell me about deadlock"));
        assert!(d.contains("Candidate: I ordered the locks"));
        assert!(roles_known && has_candidate);
    }

    #[test]
    fn dialogue_falls_back_to_speaker_labels() {
        let segs = json!([
            {"speaker": "1", "original": "What is a mutex?"},
            {"speaker": "2", "original": "A lock"},
        ]);
        let (d, roles_known, has_candidate) =
            build_review_dialogue(&segs, 16_000, &presets::INTERVIEW);
        assert!(d.contains("Speaker 1: What is a mutex?"));
        assert!(!roles_known && !has_candidate);
    }

    #[test]
    fn dialogue_accepts_versioned_sidecar() {
        let segs = json!({"version": 2, "segments": [{"source": "system", "original": "Hi"}]});
        let (d, roles_known, _) = build_review_dialogue(&segs, 16_000, &presets::INTERVIEW);
        assert_eq!(d, "Interviewer: Hi");
        assert!(roles_known);
    }

    #[test]
    fn dialogue_truncates_keeping_tail() {
        let segs = json!([
            {"source": "system", "original": "first question here"},
            {"source": "mic", "original": "later answer"},
        ]);
        let (d, _, _) = build_review_dialogue(&segs, 30, &presets::INTERVIEW);
        assert!(!d.contains("first question"));
        assert!(d.contains("Candidate: later answer"));
    }

    #[test]
    fn session_preset_comes_from_the_sidecar() {
        let meeting = json!({"version": 2, "copilot": "meeting", "segments": []});
        assert_eq!(session_preset(&meeting).id, "meeting");
        assert_eq!(session_preset(&json!([])).id, "interview");
        assert_eq!(session_preset(&json!({"segments": []})).id, "interview");
    }

    #[test]
    fn meeting_dialogue_uses_meeting_role_labels() {
        let segs = json!([
            {"source": "system", "original": "Can you own the rollout?"},
            {"source": "mic", "original": "Yes, by Friday"},
        ]);
        let (d, _, has_self) = build_review_dialogue(&segs, 16_000, &presets::MEETING);
        assert_eq!(d, "Others: Can you own the rollout?\nMe: Yes, by Friday");
        assert!(has_self);
    }

    #[test]
    fn parse_minutes_tolerates_fences_and_missing_fields() {
        let raw = "```json\n{\"overall\":\"Agreed on rollout.\",\"decisions\":[\"Ship v2\",\" \"],\
            \"action_items\":[{\"task\":\"Rollout\",\"owner\":\"Me\",\"due\":\"Friday\"},{\"task\":\"Docs\"},{\"owner\":\"x\"}],\
            \"open_questions\":[\"Budget?\"]}\n```";
        let r = parse_minutes(raw, "m", true, "now").unwrap();
        assert_eq!(r.kind, "minutes");
        assert_eq!(r.decisions, vec!["Ship v2"]);
        assert_eq!(r.action_items.len(), 2, "items without a task are dropped");
        assert_eq!(r.action_items[1].owner, "");
        assert_eq!(r.open_questions, vec!["Budget?"]);
        assert!(r.questions.is_empty());
    }

    #[test]
    fn reviews_cached_before_presets_still_load_as_scored() {
        let cached = r#"{"version":1,"generated_at":"t","model":"m","roles_known":true,
            "overall":"o","questions":[],"practice":[]}"#;
        let r: SessionReview = serde_json::from_str(cached).unwrap();
        assert_eq!(r.kind, "scored");
        assert!(r.action_items.is_empty());
    }

    #[test]
    fn no_mic_segments_yields_null_scores() {
        let raw = r#"{"overall":"o","questions":[{"question":"q","answer_summary":"a","score":4,"feedback":"f","stronger_answer":"s"}]}"#;
        let r = parse_review(raw, "m", true, false, "now").unwrap();
        assert_eq!(r.questions[0].score, None);
    }

    #[test]
    fn parse_review_tolerates_fences_and_bad_scores() {
        let raw = "Sure!\n```json\n{\"overall\":\"o\",\"questions\":[{\"question\":\"q\",\"answer_summary\":\"a\",\"score\":7,\"feedback\":\"f\",\"stronger_answer\":\"s\"},{\"question\":\"q2\",\"answer_summary\":\"a2\",\"score\":4,\"feedback\":\"f2\",\"stronger_answer\":\"s2\"}],\"practice\":[\"p1\",\"p2\"]}\n```";
        let r = parse_review(raw, "m", true, true, "now").unwrap();
        assert_eq!(r.questions[0].score, None);
        assert_eq!(r.questions[1].score, Some(4));
        assert_eq!(r.practice, vec!["p1", "p2"]);
        assert_eq!(r.version, 1);
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("myjavis-review-test-{tag}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn review_cache_roundtrip() {
        let dir = temp_dir("roundtrip");
        let r = parse_review(
            "{\"overall\":\"o\",\"questions\":[],\"practice\":[]}",
            "m",
            true,
            true,
            "now",
        )
        .unwrap();
        write_review(&dir, "a.md", &r).unwrap();
        let back = read_review(&dir, "a.md").unwrap().unwrap();
        assert_eq!(back.overall, "o");
        assert!(review_path(&dir, "a.md").ends_with("a.review.json"));
        assert!(read_review(&dir, "missing.md").unwrap().is_none());
    }
}
