//! Copilot mode: live answer suggestions, early hints and post-session
//! review, specialised per preset (interview, meeting) through data in
//! `presets` and prompt builders in `prompts`.

pub mod presets;
pub mod prompts;

/// What a meeting early-hint prompt answers when nothing is directed at the
/// user — the hint is dropped instead of shown.
pub const SILENCE_TOKEN: &str = "NONE";

/// Holds back streamed deltas while they could still spell `SILENCE_TOKEN`,
/// so the token never flashes in the hint card; real text is released as
/// soon as it diverges.
#[derive(Default)]
pub struct SilenceFilter {
    pending: String,
    released: bool,
}

impl SilenceFilter {
    /// Text to emit now, if any.
    pub fn push(&mut self, delta: &str) -> Option<String> {
        if self.released {
            return Some(delta.to_string());
        }
        self.pending.push_str(delta);
        let head = self.pending.trim_start().to_ascii_uppercase();
        let could_be_silence = head.starts_with(SILENCE_TOKEN) || SILENCE_TOKEN.starts_with(&head);
        if could_be_silence {
            return None;
        }
        self.released = true;
        Some(std::mem::take(&mut self.pending))
    }

    /// Whether a finished reply is the silence token (punctuation tolerated).
    pub fn is_silent(full: &str) -> bool {
        full.trim()
            .trim_end_matches(['.', '!'])
            .eq_ignore_ascii_case(SILENCE_TOKEN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(chunks: &[&str]) -> String {
        let mut f = SilenceFilter::default();
        chunks.iter().filter_map(|c| f.push(c)).collect()
    }

    #[test]
    fn the_silence_token_is_never_streamed() {
        assert_eq!(feed(&["NO", "NE"]), "");
        assert_eq!(feed(&[" none", "."]), "");
        assert!(SilenceFilter::is_silent(" None.\n"));
    }

    #[test]
    fn real_hints_are_released_once_they_diverge() {
        assert_eq!(feed(&["- Ask", " about budget"]), "- Ask about budget");
        assert_eq!(feed(&["No", "thing yet"]), "Nothing yet");
        assert!(!SilenceFilter::is_silent("- Ask about budget"));
    }
}
