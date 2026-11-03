//! Text translation through the configured LLM — used by STT providers that
//! only transcribe (Deepgram), unlike Soniox which streams translations.

use crate::commands::util::{extract_json_array_slice, http_client, run_blocking};
use crate::secrets::{self, SecretSlot};
use crate::services::llm;
use crate::settings::{Settings, SettingsState};
use tauri::{AppHandle, Manager};

/// One subtitle segment is short; this bounds cost if the model rambles.
const TRANSLATION_MAX_TOKENS: u32 = 400;

/// Translate one finalized transcript segment using Settings' languages and
/// custom translation terms.
#[tauri::command]
pub async fn translate_segment(app: AppHandle, text: String) -> Result<String, String> {
    run_blocking(move || {
        let settings = app.state::<SettingsState>();
        let snapshot = settings.0.lock().map_err(|e| e.to_string())?.clone();
        translate_blocking(&snapshot, &text)
    })
    .await
}

/// Lines per LLM call when translating a whole file's subtitles.
const BATCH_SIZE: usize = 20;
const BATCH_MAX_TOKENS: u32 = 4000;

/// The LLM key, or an error when the LLM isn't fully configured.
pub(crate) fn llm_key(settings: &Settings) -> Result<String, String> {
    let key = secrets::get_secret(SecretSlot::Llm)?.unwrap_or_default();
    if settings.llm_url.trim().is_empty()
        || settings.llm_model.trim().is_empty()
        || key.trim().is_empty()
    {
        return Err("Translation needs the LLM configured in Settings → AI.".to_string());
    }
    Ok(key)
}

fn translate_blocking(settings: &Settings, text: &str) -> Result<String, String> {
    if text.trim().is_empty() {
        return Ok(String::new());
    }
    let key = llm_key(settings)?;
    let client = http_client()?;
    translate_one(&client, settings, &key, text)
}

fn translate_one(
    client: &reqwest::blocking::Client,
    settings: &Settings,
    key: &str,
    text: &str,
) -> Result<String, String> {
    let raw = llm::complete_suggestions(
        client,
        &settings.llm_url,
        key,
        &settings.llm_model,
        &build_translation_prompt(text, settings),
        TRANSLATION_MAX_TOKENS,
    )?;
    Ok(clean_translation(&raw))
}

/// Translate many subtitle lines, `BATCH_SIZE` per LLM call. A batch whose
/// reply doesn't parse as a same-length JSON array is retried line by line.
/// `on_progress(done, total)` fires after each batch.
pub(crate) fn translate_lines_blocking(
    settings: &Settings,
    lines: &[String],
    mut on_progress: impl FnMut(usize, usize),
) -> Result<Vec<String>, String> {
    let key = llm_key(settings)?;
    let client = http_client()?;
    let mut out = Vec::with_capacity(lines.len());
    for batch in lines.chunks(BATCH_SIZE) {
        let raw = llm::complete_suggestions(
            &client,
            &settings.llm_url,
            &key,
            &settings.llm_model,
            &build_batch_prompt(batch, settings),
            BATCH_MAX_TOKENS,
        )?;
        match parse_batch(&raw, batch.len()) {
            Some(translated) => out.extend(translated),
            None => {
                for line in batch {
                    out.push(translate_one(&client, settings, &key, line)?);
                }
            }
        }
        on_progress(out.len(), lines.len());
    }
    Ok(out)
}

fn build_batch_prompt(lines: &[String], settings: &Settings) -> String {
    let input = serde_json::to_string(lines).unwrap_or_else(|_| "[]".to_string());
    format!(
        "You translate subtitles of a recording. {direction} Each array item is one \
         subtitle line; keep the meaning of each line in its own item.{terms}\n\
         Reply with ONLY a JSON array of exactly {n} strings, in the same order — \
         no markdown fences, no extra text.\n\nLines:\n{input}",
        direction = direction_sentence(settings),
        terms = terms_block(settings),
        n = lines.len(),
    )
}

fn parse_batch(raw: &str, expected: usize) -> Option<Vec<String>> {
    let slice = extract_json_array_slice(raw)?;
    let items: Vec<String> = serde_json::from_str(slice).ok()?;
    (items.len() == expected).then(|| items.iter().map(|s| clean_translation(s)).collect())
}

/// Prompt for a single subtitle segment. Language codes are ISO 639-1;
/// LLMs handle them without a name table.
fn build_translation_prompt(text: &str, settings: &Settings) -> String {
    format!(
        "You translate live speech-to-text subtitles. {direction} \
         The text may be a fragment of a longer sentence or contain recognition errors; \
         translate naturally and keep it concise.{terms}\n\
         Reply with ONLY the translation — no quotes, notes or explanations.\n\n\
         Text:\n{text}",
        direction = direction_sentence(settings),
        terms = terms_block(settings),
    )
}

fn direction_sentence(settings: &Settings) -> String {
    if settings.translation_type == "two_way" {
        format!(
            "The text is in {a} or {b}. Translate it into the other language \
             ({a} → {b}, {b} → {a}).",
            a = settings.language_a,
            b = settings.language_b,
        )
    } else {
        let from = if settings.source_language == "auto" {
            String::new()
        } else {
            format!(" from {}", settings.source_language)
        };
        format!(
            "Translate the text{from} into {}.",
            settings.target_language
        )
    }
}

/// Custom translation terms (Settings → Context) as a prompt section.
fn terms_block(settings: &Settings) -> String {
    let terms: Vec<String> = settings
        .custom_context
        .as_ref()
        .map(|c| {
            c.translation_terms
                .iter()
                .filter(|t| !t.source.trim().is_empty() && !t.target.trim().is_empty())
                .map(|t| format!("{} = {}", t.source.trim(), t.target.trim()))
                .collect()
        })
        .unwrap_or_default();
    if terms.is_empty() {
        String::new()
    } else {
        format!(
            "\nAlways translate these terms exactly as given:\n{}\n",
            terms.join("\n")
        )
    }
}

fn clean_translation(raw: &str) -> String {
    let t = raw.trim();
    t.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(t)
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{CustomContext, Settings, TranslationTerm};

    fn settings(translation_type: &str) -> Settings {
        Settings {
            translation_type: translation_type.into(),
            source_language: "ja".into(),
            target_language: "vi".into(),
            language_a: "en".into(),
            language_b: "ja".into(),
            ..Default::default()
        }
    }

    #[test]
    fn one_way_prompt_targets_the_target_language() {
        let p = build_translation_prompt("こんにちは", &settings("one_way"));
        assert!(p.contains("into vi"), "{p}");
        assert!(p.contains("こんにちは"));
        assert!(p.contains("ONLY the translation"));
    }

    #[test]
    fn two_way_prompt_names_both_languages() {
        let p = build_translation_prompt("hello", &settings("two_way"));
        assert!(p.contains("en"), "{p}");
        assert!(p.contains("ja"), "{p}");
        assert!(p.contains("other language"), "{p}");
    }

    #[test]
    fn prompt_includes_custom_translation_terms() {
        let mut s = settings("one_way");
        s.custom_context = Some(CustomContext {
            domain: None,
            translation_terms: vec![TranslationTerm {
                source: "Christ".into(),
                target: "Kitô".into(),
            }],
        });
        let p = build_translation_prompt("Christ", &s);
        assert!(p.contains("Christ = Kitô"), "{p}");
    }

    #[test]
    fn batch_prompt_lists_lines_as_json_and_states_the_count() {
        let lines = vec!["one".to_string(), "two \"quoted\"".to_string()];
        let p = build_batch_prompt(&lines, &settings("one_way"));
        assert!(p.contains(r#"["one","two \"quoted\""]"#), "{p}");
        assert!(p.contains("exactly 2"), "{p}");
        assert!(p.contains("into vi"), "{p}");
    }

    #[test]
    fn parse_batch_accepts_only_a_matching_string_array() {
        assert_eq!(
            parse_batch("```json\n[\"một\", \"hai\"]\n```", 2),
            Some(vec!["một".to_string(), "hai".to_string()])
        );
        assert_eq!(parse_batch("[\"một\"]", 2), None);
        assert_eq!(parse_batch("[1, 2]", 2), None);
        assert_eq!(parse_batch("no json here", 1), None);
    }

    #[test]
    fn clean_translation_strips_quotes_and_whitespace() {
        assert_eq!(clean_translation("  \"Xin chào\"\n"), "Xin chào");
        assert_eq!(clean_translation("Xin chào"), "Xin chào");
    }
}
