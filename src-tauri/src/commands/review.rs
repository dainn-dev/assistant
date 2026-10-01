//! Post-session review — one LLM call over a saved interview transcript,
//! cached next to the transcript as `<name>.review.json`.

use crate::commands::interview::extract_json_object_slice;
use serde::{Deserialize, Serialize};

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
                let sp = seg
                    .get("speaker")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
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
    let slice =
        extract_json_object_slice(raw).ok_or_else(|| "Review contained no JSON object".to_string())?;
    let v: serde_json::Value =
        serde_json::from_str(slice).map_err(|e| format!("review JSON: {e}"))?;

    let mut questions = Vec::new();
    for q in v.get("questions").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
        let text = |k: &str| q.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
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
        overall: v.get("overall").and_then(|x| x.as_str()).unwrap_or("").trim().to_string(),
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
}
