//! Prompt builders for copilot LLM calls. Interview text was moved here
//! verbatim from the command modules; `golden/` fixtures lock it.

use super::presets::CopilotPreset;
use super::SILENCE_TOKEN;

/// Inputs for the manual (button / chat) suggestion prompt.
pub struct SuggestInputs<'a> {
    pub source_language: &'a str,
    pub target_language: &'a str,
    /// Normalized suggestion type: "target" | "translation" | "both".
    pub suggestion_type: &'a str,
    pub summary: &'a str,
    pub recent_lines: &'a str,
    pub ctx_block: &'a str,
    pub query_text: &'a str,
}

/// Inputs for the streamed early-hint prompt.
pub struct EarlyInputs<'a> {
    pub hint_lang: &'a str,
    pub ctx_block: &'a str,
    pub recent_lines: &'a str,
    pub question: &'a str,
}

fn is_meeting(preset: &CopilotPreset) -> bool {
    preset.id == "meeting"
}

/// Excerpt labels of the preset's upload slots: (`[A] and [B]`, `[A]/[B]`).
fn slot_labels(preset: &CopilotPreset) -> (String, String) {
    let [a, b] = &preset.doc_slots;
    (
        format!("[{}] and [{}]", a.label, b.label),
        format!("[{}]/[{}]", a.label, b.label),
    )
}

/// The "what to answer" focus for a manual suggestion.
pub fn suggest_query(
    preset: &CopilotPreset,
    transcript_context: Option<&str>,
    user_draft: Option<&str>,
) -> String {
    let (line_label, draft_label, fallback) = if is_meeting(preset) {
        (
            "Latest dialogue line",
            "My draft or notes",
            "General meeting support for my next contribution.",
        )
    } else {
        (
            "Latest interviewer / dialogue line",
            "Candidate draft or notes",
            "General interview coaching for the next reply.",
        )
    };
    let mut query_bits: Vec<String> = Vec::new();
    if let Some(t) = transcript_context.filter(|t| !t.trim().is_empty()) {
        query_bits.push(format!("{line_label}:\n{t}"));
    }
    if let Some(d) = user_draft.filter(|d| !d.trim().is_empty()) {
        query_bits.push(format!("{draft_label}:\n{d}"));
    }
    if query_bits.is_empty() {
        fallback.to_string()
    } else {
        query_bits.join("\n\n")
    }
}

fn meeting_suggest_prompt(
    preset: &CopilotPreset,
    inputs: &SuggestInputs,
    return_format: &str,
) -> String {
    let (labels_and, _) = slot_labels(preset);
    format!(
        "You are me, a participant in a live meeting. Suggest what I could say next, in first person: \
         answer a question or request directed at me, or ask one useful clarifying question.\n\
         Use the {labels_and} excerpts below when relevant. Never invent facts, numbers or commitments \
         that are not in the excerpts or the dialogue.\n\
         Keep it concise, spoken tone, under ~60 words.\n\n\
         {return_format}\n\n\
         Session summary (may be empty):\n{summary}\n\n\
         Recent messages / transcript:\n{recent_lines}\n\n\
         Meeting brief / agenda knowledge:\n{ctx_block}\n\n\
         What to respond to:\n{query_text}\n",
        summary = inputs.summary,
        recent_lines = inputs.recent_lines,
        ctx_block = inputs.ctx_block,
        query_text = inputs.query_text,
    )
}

pub fn suggest_prompt(preset: &CopilotPreset, inputs: &SuggestInputs) -> String {
    let source_language = inputs.source_language;
    let target_language = inputs.target_language;
    let st = inputs.suggestion_type;
    let summary = inputs.summary;
    let recent_lines = inputs.recent_lines;
    let ctx_block = inputs.ctx_block;
    let query_text = inputs.query_text;
    let return_format = match st {
        "both" => format!(
            "Return ONLY a JSON array with exactly 1 object. The object must have exactly two string keys: \"target\" and \"translation\".\n\
The \"target\" value must be the answer in the source/interview language (settings code: {src}).\n\
The \"translation\" value must be the same answer meaning in the translation language (settings code: {tgt}).\n\
Example: [{{\"target\":\"Hello\",\"translation\":\"Xin chào\"}}]\n\
No markdown fences, no extra text — ONLY the JSON array.",
            src = source_language,
            tgt = target_language,
        ),
        "target" => format!(
            "Return ONLY a JSON array with exactly 1 string. The string must be an answer option entirely in the source/interview language (settings code: {src}).\n\
Example: [\"option one\"]\n\
No markdown fences, no extra text — ONLY the JSON array.",
            src = source_language,
        ),
        _ => format!(
            "Return ONLY a JSON array with exactly 1 string. The string must be an answer option entirely in the translation language (settings code: {tgt}).\n\
Example: [\"option one\"]\n\
No markdown fences, no extra text — ONLY the JSON array.",
            tgt = target_language,
        ),
    };
    if is_meeting(preset) {
        // Same JSON contract; only the language wording is interview-specific.
        let return_format = return_format.replace("source/interview language", "source language");
        return meeting_suggest_prompt(preset, inputs, &return_format);
    }

    let prompt = format!(
        "You are the candidate — a senior software engineer. Speak in first person as if you are answering the interviewer directly.\n\
Use the [SUMMARY], [STORY], [STRENGTH], [CV] and [JD] excerpts below. If a [STORY] fits the question, answer with that story's Situation→Action→Result in first person. Never claim experience that is not in the excerpts.\n\
Make the answer technical and concrete (mention design patterns, frameworks, real examples from the excerpts), not generic theory.\n\
Keep it concise, spoken tone, under ~80 words.\n\n\
{return_format}\n\n\
Session summary (may be empty):\n\
{summary}\n\n\
Recent messages / transcript:\n\
{recent_lines}\n\n\
Candidate CV/JD knowledge (use this to make the answer personal and technical):\n\
{ctx_block}\n\n\
Interviewer's question / task focus:\n\
{query_text}\n",
        return_format = return_format,
        summary = summary,
        recent_lines = recent_lines,
        ctx_block = ctx_block,
        query_text = query_text,
    );
    prompt
}

fn meeting_early_prompt(preset: &CopilotPreset, inputs: &EarlyInputs) -> String {
    let (_, labels_slash) = slot_labels(preset);
    format!(
        "You are assisting me during a live meeting. The latest remark may be INCOMPLETE — it is still \
         being transcribed. Answer based only on what is clear so far.\n\
         Rules:\n\
         - If the remark is a question or request directed at me, give 2-3 short talking points, each \
         under 20 words, in language code {hint_lang}.\n\
         - Ground points in the {labels_slash} excerpts when relevant; never invent facts or commitments.\n\
         - If nothing in it is directed at me or needs my input, reply with exactly {SILENCE_TOKEN}.\n\
         - Plain text only — one point per line starting with \"- \". No JSON, no preamble.\n\n\
         Brief/agenda excerpts (may be empty):\n{ctx_block}\n\n\
         Recent dialogue (may be empty):\n{recent_lines}\n\n\
         Latest remark so far (may be incomplete):\n{question}\n",
        hint_lang = inputs.hint_lang,
        ctx_block = inputs.ctx_block,
        recent_lines = inputs.recent_lines,
        question = inputs.question,
    )
}

pub fn early_prompt(preset: &CopilotPreset, inputs: &EarlyInputs) -> String {
    if is_meeting(preset) {
        return meeting_early_prompt(preset, inputs);
    }
    let hint_lang = inputs.hint_lang;
    let ctx_block = inputs.ctx_block;
    let recent_lines = inputs.recent_lines;
    let question = inputs.question;
    let prompt = format!(
        "You are coaching a candidate during a live interview. The interviewer's question may be INCOMPLETE — it is still being transcribed. Answer based only on what is clear so far.\n\
Rules:\n\
- Give 2-3 short talking points, each under 20 words, in language code {hint_lang}.\n\
- Ground points in the [SUMMARY]/[STORY]/[CV]/[JD] excerpts when relevant; if a [STORY] fits, hint at its Situation→Action→Result; never invent experience that is not there.\n\
- If the question is too vague to answer, give one line on what to listen for or clarify.\n\
- Plain text only — one point per line starting with \"- \". No JSON, no preamble.\n\n\
CV/JD excerpts (may be empty):\n{ctx_block}\n\n\
Recent dialogue (may be empty):\n{recent_lines}\n\n\
Interviewer question so far (may be incomplete):\n{question}\n",
    );
    prompt
}

fn meeting_review_prompt(
    preset: &CopilotPreset,
    roles_known: bool,
    has_self: bool,
    dialogue: &str,
) -> String {
    let roles_note = if roles_known {
        format!(
            "Lines are labelled {}/{}.",
            preset.remote_label, preset.self_label
        )
    } else {
        "Speaker roles are unknown — infer them from content.".to_string()
    };
    let self_note = if has_self {
        ""
    } else {
        " My microphone was not captured, so my own lines are missing — only assign action items to me when others said so."
    };
    format!(
        "You are writing minutes for a finished meeting transcript. {roles_note}{self_note}\n\
         Return ONLY a JSON object:\n\
         {{\"overall\":\"<3–5 sentence summary>\",\n\
         \"decisions\":[\"<decision made>\"],\n\
         \"action_items\":[{{\"task\":\"<what>\",\"owner\":\"<who, or empty>\",\"due\":\"<when, or empty>\"}}],\n\
         \"open_questions\":[\"<unresolved question>\"]}}\n\
         Only include items actually discussed; use empty arrays when there are none. \
         No markdown fences, no extra text.\n\n\
         Transcript:\n{dialogue}"
    )
}

pub fn review_prompt(
    preset: &CopilotPreset,
    roles_known: bool,
    has_candidate: bool,
    dialogue: &str,
) -> String {
    if is_meeting(preset) {
        return meeting_review_prompt(preset, roles_known, has_candidate, dialogue);
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
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot::presets::INTERVIEW;
    use std::path::PathBuf;

    /// Compare against `golden/<name>.txt`; `UPDATE_GOLDEN=1` rewrites it.
    fn check_golden(name: &str, actual: &str) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/copilot/golden")
            .join(format!("{name}.txt"));
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, actual).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("missing golden {}: {e}", path.display()))
            .replace("\r\n", "\n"); // git autocrlf on Windows checkouts
        assert_eq!(actual, expected, "prompt drifted from golden/{name}.txt");
    }

    fn suggest(st: &str, query: &str) -> String {
        suggest_prompt(
            &INTERVIEW,
            &SuggestInputs {
                source_language: "ja",
                target_language: "vi",
                suggestion_type: st,
                summary: "Earlier: discussed caching.",
                recent_lines: "- [user] How do you scale it?\n",
                ctx_block: "[CV] Built a Kafka pipeline\n---\n[JD] Needs Go",
                query_text: query,
            },
        )
    }

    #[test]
    fn interview_suggest_prompts_match_golden() {
        let q = suggest_query(
            &INTERVIEW,
            Some("Tell me about a hard bug"),
            Some("deadlock story"),
        );
        check_golden("interview_query_full", &q);
        check_golden(
            "interview_query_empty",
            &suggest_query(&INTERVIEW, Some("  "), None),
        );
        check_golden("interview_suggest_both", &suggest("both", &q));
        check_golden("interview_suggest_target", &suggest("target", &q));
        check_golden("interview_suggest_translation", &suggest("translation", &q));
    }

    #[test]
    fn interview_early_prompt_matches_golden() {
        let p = early_prompt(
            &INTERVIEW,
            &EarlyInputs {
                hint_lang: "vi",
                ctx_block: "[STORY] Deadlock fix: ordered locks",
                recent_lines: "- [interviewer] Tell me about",
                question: "Tell me about a time you",
            },
        );
        check_golden("interview_early", &p);
    }

    #[test]
    fn meeting_prompts_speak_as_a_participant() {
        use crate::copilot::presets::MEETING;
        use crate::copilot::SILENCE_TOKEN;

        let q = suggest_query(
            &MEETING,
            Some("Can you own the rollout?"),
            Some("yes, Friday"),
        );
        assert!(
            q.starts_with("Latest dialogue line:\nCan you own the rollout?"),
            "{q}"
        );
        assert!(suggest_query(&MEETING, None, None).contains("meeting"));

        let suggest = suggest_prompt(
            &MEETING,
            &SuggestInputs {
                source_language: "en",
                target_language: "vi",
                suggestion_type: "both",
                summary: "",
                recent_lines: "- [system] Can you own the rollout?\n",
                ctx_block: "[AGENDA] Rollout plan",
                query_text: &q,
            },
        );
        assert!(
            suggest.contains("[BRIEF]") && suggest.contains("[AGENDA]"),
            "{suggest}"
        );
        let early = early_prompt(
            &MEETING,
            &EarlyInputs {
                hint_lang: "vi",
                ctx_block: "",
                recent_lines: "",
                question: "Can you",
            },
        );
        assert!(
            early.contains(&format!("exactly {SILENCE_TOKEN}")),
            "{early}"
        );
        let review = review_prompt(&MEETING, true, false, "Others: Who owns it?");
        assert!(review.contains("\"action_items\"") && review.contains("\"decisions\""));
        assert!(review.contains("Others/Me"), "{review}");

        for p in [&suggest, &early, &review] {
            let lower = p.to_lowercase();
            assert!(
                !lower.contains("candidate") && !lower.contains("interview"),
                "{p}"
            );
        }
        check_golden("meeting_suggest_both", &suggest);
        check_golden("meeting_early", &early);
        check_golden("meeting_review", &review);
    }

    #[test]
    fn interview_review_prompts_match_golden() {
        let dialogue = "Interviewer: Why Rust?\nCandidate: Memory safety.";
        check_golden(
            "interview_review_known",
            &review_prompt(&INTERVIEW, true, true, dialogue),
        );
        check_golden(
            "interview_review_unknown",
            &review_prompt(&INTERVIEW, false, false, dialogue),
        );
    }
}
