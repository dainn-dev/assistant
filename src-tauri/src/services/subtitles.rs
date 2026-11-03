//! SubRip (.srt) subtitle output for file mode.

use serde::Serialize;

/// One subtitle line; times are seconds from the start of the media.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubtitleCue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// `HH:MM:SS,mmm`; negative input clamps to zero.
pub fn srt_timestamp(seconds: f64) -> String {
    let total_ms = (seconds.max(0.0) * 1000.0).round() as u64;
    let (h, rem) = (total_ms / 3_600_000, total_ms % 3_600_000);
    let (m, rem) = (rem / 60_000, rem % 60_000);
    let (s, ms) = (rem / 1000, rem % 1000);
    format!("{h:02}:{m:02}:{s:02},{ms:03}")
}

/// Render cues as an .srt document, skipping blank ones.
pub fn to_srt(cues: &[SubtitleCue]) -> String {
    cues.iter()
        .filter(|c| !c.text.trim().is_empty())
        .enumerate()
        .map(|(i, c)| {
            format!(
                "{}\n{} --> {}\n{}\n",
                i + 1,
                srt_timestamp(c.start),
                srt_timestamp(c.end),
                c.text.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(start: f64, end: f64, text: &str) -> SubtitleCue {
        SubtitleCue {
            start,
            end,
            text: text.into(),
        }
    }

    #[test]
    fn timestamps_use_srt_format() {
        assert_eq!(srt_timestamp(0.0), "00:00:00,000");
        assert_eq!(srt_timestamp(3725.5), "01:02:05,500");
        assert_eq!(srt_timestamp(-1.0), "00:00:00,000");
    }

    #[test]
    fn cues_are_numbered_from_one_and_blank_line_separated() {
        let srt = to_srt(&[cue(0.0, 1.25, "Hello"), cue(2.0, 3.0, "World")]);
        assert_eq!(
            srt,
            "1\n00:00:00,000 --> 00:00:01,250\nHello\n\n2\n00:00:02,000 --> 00:00:03,000\nWorld\n"
        );
    }

    #[test]
    fn empty_cues_are_skipped_and_numbering_stays_contiguous() {
        let srt = to_srt(&[cue(0.0, 1.0, "  "), cue(1.0, 2.0, "Kept")]);
        assert!(srt.starts_with("1\n00:00:01,000"), "{srt}");
    }
}
