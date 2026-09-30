//! Generic OpenAI-compatible chat completion for suggested interview answers.

use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::json;

pub fn complete_suggestions(
    client: &Client,
    url: &str,
    api_key: &str,
    model: &str,
    prompt: &str,
) -> Result<String, String> {
    let body = json!({
        "model": model,
        "temperature": 0.5,
        "max_tokens": 600,
        "messages": [{ "role": "user", "content": prompt }],
    });

    let resp = client
        .post(url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .map_err(|e| format!("LLM request: {e}"))?;

    if !resp.status().is_success() {
        return Err(resp.text().unwrap_or_default());
    }

    #[derive(Deserialize)]
    struct Msg {
        content: String,
    }
    #[derive(Deserialize)]
    struct Choice {
        message: Msg,
    }
    #[derive(Deserialize)]
    struct Root {
        choices: Vec<Choice>,
    }

    let root: Root = resp.json().map_err(|e| e.to_string())?;
    root.choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .ok_or_else(|| "LLM: empty choices".to_string())
}

// ─── Streaming (SSE) ──────────────────────────────────────────

/// Incremental SSE extractor: appends raw bytes to `buf`, returns every
/// complete `data:` payload found, and leaves the partial tail in `buf`.
/// Splits only on `\n` boundaries so a multi-byte UTF-8 char split across
/// network chunks is never sliced mid-sequence.
pub fn sse_feed(buf: &mut Vec<u8>, chunk: &[u8]) -> Vec<String> {
    buf.extend_from_slice(chunk);
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(pos) = buf[cursor..].iter().position(|&b| b == b'\n') {
        let end = cursor + pos;
        let mut line = &buf[cursor..end];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        cursor = end + 1;
        if let Some(payload) = line.strip_prefix(b"data:") {
            let payload = payload.strip_prefix(b" ").unwrap_or(payload);
            out.push(String::from_utf8_lossy(payload).into_owned());
        }
        // Non-data lines (comments, "event:", blank separators) are ignored.
    }
    buf.drain(..cursor);
    out
}

/// Streaming chat completion — OpenAI-compatible SSE. Emits each
/// `choices[0].delta.content` fragment via `on_delta` and returns the full
/// accumulated text. Checks `cancel` between chunks; a set flag aborts with
/// the `"__cancelled__"` sentinel so callers can distinguish user-driven
/// aborts from transport errors.
pub async fn complete_suggestions_stream(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    model: &str,
    prompt: &str,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    mut on_delta: impl FnMut(String),
) -> Result<String, String> {
    use futures_util::StreamExt;
    use std::sync::atomic::Ordering;

    let body = json!({
        "model": model,
        "temperature": 0.5,
        "max_tokens": 400,
        "stream": true,
        "messages": [{ "role": "user", "content": prompt }],
    });

    let resp = client
        .post(url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("LLM request: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let trimmed: String = text.chars().take(200).collect();
        return Err(format!("LLM HTTP {status}: {trimmed}"));
    }

    #[derive(Deserialize)]
    struct Delta {
        content: Option<String>,
    }
    #[derive(Deserialize)]
    struct Choice {
        delta: Delta,
    }
    #[derive(Deserialize)]
    struct Root {
        choices: Vec<Choice>,
    }

    let mut raw = Vec::new();
    let mut full = String::new();
    let mut stream = resp.bytes_stream();

    while let Some(item) = stream.next().await {
        if cancel.load(Ordering::SeqCst) {
            return Err("__cancelled__".to_string());
        }
        let chunk = item.map_err(|e| format!("LLM stream: {e}"))?;
        for payload in sse_feed(&mut raw, &chunk) {
            if payload.trim() == "[DONE]" {
                return Ok(full);
            }
            if let Ok(root) = serde_json::from_str::<Root>(&payload) {
                for c in root.choices {
                    if let Some(t) = c.delta.content {
                        if !t.is_empty() {
                            full.push_str(&t);
                            on_delta(t);
                        }
                    }
                }
            }
            // Malformed payloads are skipped, never surfaced as content.
        }
    }

    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_feed_handles_split_utf8_and_multi_events() {
        let mut buf = Vec::new();
        let a = "data: {\"c\":\"hél".as_bytes();
        let b = "lo\"}\n\ndata: [D".as_bytes();
        let c = "ONE]\n\n".as_bytes();
        assert!(sse_feed(&mut buf, a).is_empty());
        assert_eq!(sse_feed(&mut buf, b), vec!["{\"c\":\"héllo\"}".to_string()]);
        assert_eq!(sse_feed(&mut buf, c), vec!["[DONE]".to_string()]);
    }

    #[test]
    fn sse_feed_tolerates_malformed_and_comment_lines() {
        let mut buf = Vec::new();
        let out = sse_feed(&mut buf, b": junk\r\ndata: ok\n\n");
        assert_eq!(out, vec!["ok".to_string()]);
        // Partial tail without newline stays buffered
        assert!(sse_feed(&mut buf, b"data: part").is_empty());
        assert_eq!(sse_feed(&mut buf, b"ial\n"), vec!["partial".to_string()]);
    }
}
