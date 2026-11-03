//! Deepgram pre-recorded transcription (file mode). Live streaming runs in
//! the webview (src/js/deepgram.js).

use crate::services::subtitles::SubtitleCue;
use reqwest::blocking::Client;
use serde::Deserialize;

const PRERECORDED_URL: &str = "https://api.deepgram.com/v1/listen";

/// Same language rule as live mode: `multi` (nova-3 code-switching) when the
/// source isn't pinned to one language.
pub fn prerecorded_query(model: &str, translation_type: &str, source_language: &str) -> String {
    let multi =
        translation_type == "two_way" || source_language.is_empty() || source_language == "auto";
    let language = if multi { "multi" } else { source_language };
    format!(
        "model={}&language={language}&utterances=true&smart_format=true&punctuate=true",
        if model.trim().is_empty() {
            "nova-3"
        } else {
            model.trim()
        }
    )
}

#[derive(Deserialize)]
struct PrerecordedResponse {
    results: PrerecordedResults,
}

#[derive(Deserialize)]
struct PrerecordedResults {
    utterances: Option<Vec<Utterance>>,
}

#[derive(Deserialize)]
struct Utterance {
    start: f64,
    end: f64,
    transcript: String,
}

/// Utterances → cues, dropping blank ones.
pub fn parse_utterances(body: &str) -> Result<Vec<SubtitleCue>, String> {
    let parsed: PrerecordedResponse =
        serde_json::from_str(body).map_err(|e| format!("Deepgram response: {e}"))?;
    let utterances = parsed
        .results
        .utterances
        .ok_or("Deepgram response has no utterances")?;
    Ok(utterances
        .into_iter()
        .filter(|u| !u.transcript.trim().is_empty())
        .map(|u| SubtitleCue {
            start: u.start,
            end: u.end,
            text: u.transcript.trim().to_string(),
        })
        .collect())
}

/// Upload a WAV and return its utterances as subtitle cues.
pub fn transcribe_prerecorded(
    client: &Client,
    api_key: &str,
    query: &str,
    wav: Vec<u8>,
) -> Result<Vec<SubtitleCue>, String> {
    let resp = client
        .post(format!("{PRERECORDED_URL}?{query}"))
        .header("Authorization", format!("Token {api_key}"))
        .header("Content-Type", "audio/wav")
        .body(wav)
        .send()
        .map_err(|e| format!("Deepgram request: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .map_err(|e| format!("Deepgram response body: {e}"))?;
    if !status.is_success() {
        return Err(format!("Deepgram error {status}: {body}"));
    }
    parse_utterances(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_uses_multi_for_auto_and_two_way() {
        let q = prerecorded_query("nova-3", "one_way", "auto");
        assert!(q.contains("language=multi"), "{q}");
        assert!(q.contains("utterances=true"), "{q}");
        assert!(prerecorded_query("nova-3", "two_way", "ja").contains("language=multi"));
        assert!(prerecorded_query("nova-2", "one_way", "vi").contains("language=vi"));
        assert!(prerecorded_query("nova-2", "one_way", "vi").contains("model=nova-2"));
    }

    #[test]
    fn parses_utterances_into_cues() {
        let json = r#"{"results":{"utterances":[
            {"start":0.5,"end":2.0,"transcript":"Hello there."},
            {"start":2.5,"end":3.0,"transcript":"   "},
            {"start":3.1,"end":4.2,"transcript":"Bye."}
        ]}}"#;
        let cues = parse_utterances(json).unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "Hello there.");
        assert_eq!(cues[1].start, 3.1);
    }

    #[test]
    fn missing_utterances_is_an_error() {
        assert!(parse_utterances(r#"{"results":{}}"#).is_err());
        assert!(parse_utterances("not json").is_err());
    }
}
