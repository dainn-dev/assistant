//! Post-session review — one LLM call over a saved interview transcript,
//! cached next to the transcript as `<name>.review.json`.

use crate::commands::interview::{extract_json_object_slice, http_client};
use crate::secrets::{self, SecretSlot};
use crate::services::llm;
use crate::settings::SettingsState;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};

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
pub fn review_session(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    filename: String,
    force: bool,
) -> Result<SessionReview, String> {
    valid_md_filename(&filename)?;
    let dir = crate::commands::transcript::transcript_dir(&app)?;

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

    let (dialogue, roles_known, has_candidate) =
        build_review_dialogue(&segments, REVIEW_DIALOGUE_MAX_CHARS);
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

    let roles_note = if roles_known {
        "Lines are labelled Interviewer/Candidate."
    } else {
        "Speaker roles are unknown — infer who is interviewing from content and say so in \"overall\"."
    };
    let candidate_note = if has_candidate {
        ""
    } else {
        " No candidate audio was captured: set every \"score\" to null and make \"feedback\" describe how to approach the question."
    };

    let prompt = format!(
        "You are an interview coach reviewing a finished interview transcript. {roles_note}{candidate_note}\n\
        Return ONLY a JSON object:\n\
        {{\"overall\":\"<3–5 sentences>\",\n\
        \"questions\":[{{\"question\":\"<interviewer question, condensed>\",\"answer_summary\":\"<what the candidate said, ≤40 words>\",\"score\":<1–5 integer>,\"feedback\":\"<what worked / what to fix, ≤60 words>\",\"stronger_answer\":\"<a better first-person answer, ≤90 words>\"}}],\n\
        \"practice\":[\"<question to rehearse>\", …up to 5]}}\n\
        Only include questions actually asked. No markdown fences, no extra text.\n\n\
        Transcript:\n{dialogue}"
    );

    let client = http_client()?;
    let raw = llm::complete_suggestions(&client, &llm_url, &llm_key, &llm_model, &prompt)?;
    let now = chrono::Utc::now().to_rfc3339();
    let review = parse_review(&raw, &llm_model, roles_known, has_candidate, &now)?;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionReview {
    pub version: u32,
    pub generated_at: String,
    pub model: String,
    /// False when the transcript predates source tagging — the model then
    /// infers roles from content and should say so in `overall`.
    pub roles_known: bool,
    pub overall: String,
    pub questions: Vec<ReviewQuestion>,
    pub practice: Vec<String>,
}

/// Turn a saved sidecar (bare array or `{version, segments, metrics}`) into a
/// dialogue for the review prompt. Returns (dialogue, roles_known,
/// has_candidate). Keeps the LAST `max_chars` — recent answers matter most.
pub fn build_review_dialogue(
    segments: &serde_json::Value,
    max_chars: usize,
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
            Some("system") => "Interviewer".to_string(),
            Some("mic") => "Candidate".to_string(),
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
        generated_at: now.to_string(),
        model: model.to_string(),
        roles_known,
        overall: v
            .get("overall")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        questions,
        practice,
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
        let (d, roles_known, has_candidate) = build_review_dialogue(&segs, 16_000);
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
        let (d, roles_known, has_candidate) = build_review_dialogue(&segs, 16_000);
        assert!(d.contains("Speaker 1: What is a mutex?"));
        assert!(!roles_known && !has_candidate);
    }

    #[test]
    fn dialogue_accepts_versioned_sidecar() {
        let segs = json!({"version": 2, "segments": [{"source": "system", "original": "Hi"}]});
        let (d, roles_known, _) = build_review_dialogue(&segs, 16_000);
        assert_eq!(d, "Interviewer: Hi");
        assert!(roles_known);
    }

    #[test]
    fn dialogue_truncates_keeping_tail() {
        let segs = json!([
            {"source": "system", "original": "first question here"},
            {"source": "mic", "original": "later answer"},
        ]);
        let (d, _, _) = build_review_dialogue(&segs, 30);
        assert!(!d.contains("first question"));
        assert!(d.contains("Candidate: later answer"));
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
