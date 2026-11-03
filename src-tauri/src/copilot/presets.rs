//! Copilot presets — everything that differs between interview and meeting
//! copilots, as data. Prompt text lives in `prompts`, keyed by `id`.

/// One upload slot: stored `doc_type` and the excerpt label shown to the LLM.
#[derive(Debug, PartialEq, Eq)]
pub struct DocSlot {
    pub doc_type: &'static str,
    pub label: &'static str,
}

/// Shape of the post-session review JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewKind {
    /// Per-question scores + stronger answers (interview).
    Scored,
    /// Summary, decisions, action items, open questions (meeting).
    Minutes,
}

#[derive(Debug)]
pub struct CopilotPreset {
    pub id: &'static str,
    /// Speaker on the system-audio stream (the other side of the call).
    pub remote_label: &'static str,
    /// Speaker on the microphone stream (the user).
    pub self_label: &'static str,
    /// Upload slots, in UI order.
    pub doc_slots: [DocSlot; 2],
    pub has_profile: bool,
    pub review: ReviewKind,
    /// Early hints may answer `SILENCE_TOKEN` when nothing is directed at
    /// the user (meetings); interview hints always answer.
    pub hints_may_be_silent: bool,
}

pub static INTERVIEW: CopilotPreset = CopilotPreset {
    id: "interview",
    remote_label: "Interviewer",
    self_label: "Candidate",
    doc_slots: [
        DocSlot {
            doc_type: "cv",
            label: "CV",
        },
        DocSlot {
            doc_type: "jd",
            label: "JD",
        },
    ],
    has_profile: true,
    review: ReviewKind::Scored,
    hints_may_be_silent: false,
};

pub static MEETING: CopilotPreset = CopilotPreset {
    id: "meeting",
    remote_label: "Others",
    self_label: "Me",
    doc_slots: [
        DocSlot {
            doc_type: "brief",
            label: "BRIEF",
        },
        DocSlot {
            doc_type: "agenda",
            label: "AGENDA",
        },
    ],
    has_profile: false,
    review: ReviewKind::Minutes,
    hints_may_be_silent: true,
};

impl CopilotPreset {
    /// Whether ingested documents of `doc_type` belong to this preset —
    /// keeps a meeting brief out of interview answers and vice versa.
    pub fn owns_doc_type(&self, doc_type: &str) -> bool {
        self.doc_slots.iter().any(|s| s.doc_type == doc_type)
    }
}

/// Preset for a stored `app_mode` (case-insensitive). Unknown or empty
/// values fall back to interview — the only copilot that existed before
/// presets, so old settings and sessions keep their behaviour.
pub fn preset_for_mode(app_mode: &str) -> &'static CopilotPreset {
    match app_mode.trim().to_ascii_lowercase().as_str() {
        "meeting" => &MEETING,
        _ => &INTERVIEW,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_and_unknown_modes_resolve_to_interview() {
        assert_eq!(preset_for_mode("Interview").id, "interview");
        assert_eq!(preset_for_mode("").id, "interview");
        assert_eq!(preset_for_mode("bogus").id, "interview");
    }

    #[test]
    fn meeting_mode_resolves_case_insensitively() {
        assert_eq!(preset_for_mode("Meeting").id, "meeting");
        assert_eq!(preset_for_mode(" meeting ").id, "meeting");
    }

    #[test]
    fn presets_own_only_their_document_types() {
        assert!(INTERVIEW.owns_doc_type("cv"));
        assert!(!INTERVIEW.owns_doc_type("brief"));
        assert!(MEETING.owns_doc_type("agenda"));
        assert!(!MEETING.owns_doc_type("jd"));
        assert!(!MEETING.has_profile);
        assert_eq!(MEETING.review, ReviewKind::Minutes);
    }
}
