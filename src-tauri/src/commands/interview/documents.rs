//! Uploaded CV/JD (or brief/agenda) files: path/bytes resolution, PDF/DOCX
//! text extraction and overlapping chunking.

use serde::Deserialize;

pub(super) const INGEST_CHUNK_MAX_CHARS: usize = 1200;
pub(super) const INGEST_CHUNK_OVERLAP_CHARS: usize = 150;
/// Upper bound on a single upload — keeps extractor memory bounded and
/// rejects accidental multi-hundred-MB drops.
const INGEST_FILE_MAX_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterviewFilePart {
    pub filename: String,
    #[serde(default)]
    pub bytes: Vec<u8>,
    /// Optional filesystem path. When set, the backend reads the file itself
    /// so multi-MB PDFs/DOCX never cross the IPC bridge as JSON byte arrays.
    #[serde(default)]
    pub path: Option<String>,
}

impl InterviewFilePart {
    /// Materialize `bytes` from `path` if needed, enforcing the size cap.
    pub(super) fn resolve(mut self) -> Result<Self, String> {
        if let Some(path) = self.path.take() {
            let meta = std::fs::metadata(&path)
                .map_err(|e| format!("Cannot stat '{}': {e}", self.filename))?;
            if meta.len() > INGEST_FILE_MAX_BYTES {
                return Err(format!(
                    "'{}' is too large ({} MB, max {} MB)",
                    self.filename,
                    meta.len() / (1024 * 1024),
                    INGEST_FILE_MAX_BYTES / (1024 * 1024)
                ));
            }
            self.bytes = std::fs::read(&path)
                .map_err(|e| format!("Cannot read '{}': {e}", self.filename))?;
        }
        if self.bytes.len() as u64 > INGEST_FILE_MAX_BYTES {
            return Err(format!(
                "'{}' is too large (max {} MB)",
                self.filename,
                INGEST_FILE_MAX_BYTES / (1024 * 1024)
            ));
        }
        if self.bytes.is_empty() {
            return Err(format!("'{}' is empty", self.filename));
        }
        Ok(self)
    }
}

fn extract_docx(data: &[u8]) -> Result<String, String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;
    use std::io::{Cursor, Read};
    use zip::ZipArchive;

    let cur = Cursor::new(data);
    let mut archive = ZipArchive::new(cur).map_err(|e| format!("docx zip: {e}"))?;
    let mut file = archive
        .by_name("word/document.xml")
        .map_err(|e| format!("docx missing document.xml: {e}"))?;
    let mut xml = String::new();
    file.read_to_string(&mut xml)
        .map_err(|e| format!("docx read: {e}"))?;

    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut out = String::new();
    let mut wt_depth = 0i32;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                if e.name().as_ref() == b"w:t" {
                    wt_depth += 1;
                }
            }
            Ok(Event::End(ref e)) => {
                if e.name().as_ref() == b"w:t" {
                    wt_depth = (wt_depth - 1).max(0);
                }
            }
            Ok(Event::Text(ref t)) => {
                if wt_depth > 0 {
                    let s = t.unescape().map_err(|e| e.to_string())?;
                    out.push_str(&s);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(e.to_string()),
            Ok(_) => {}
        }
        buf.clear();
    }

    Ok(out)
}

fn extract_pdf(data: &[u8]) -> Result<String, String> {
    pdf_extract::extract_text_from_mem(data).map_err(|e| format!("pdf: {e}"))
}

pub(super) fn extract_file_text(filename: &str, bytes: &[u8]) -> Result<String, String> {
    let lower = filename.to_lowercase();
    let text = if lower.ends_with(".pdf") {
        extract_pdf(bytes)?
    } else if lower.ends_with(".docx") {
        extract_docx(bytes)?
    } else {
        return Err(format!("Unsupported file type: {filename}"));
    };
    Ok(text.trim().chars().take(200_000).collect::<String>())
}

/// Word-boundary chunks up to `max_chars` (byte length of joined words, matching OpenAI token-ish limits for ASCII-heavy CVs).
/// After each chunk except the last, the next chunk starts with a suffix of the previous chunk (up to `overlap` **characters**, word-aligned).
pub(super) fn chunk_text_with_overlap(text: &str, max_chars: usize, overlap: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    if max_chars == 0 {
        return Vec::new();
    }
    let overlap = overlap.min(max_chars.saturating_sub(1));

    fn overlap_suffix(s: &str, max_overlap_chars: usize) -> &str {
        if max_overlap_chars == 0 || s.is_empty() {
            return "";
        }
        let char_count = s.chars().count();
        if char_count <= max_overlap_chars {
            return s;
        }
        let skip = char_count - max_overlap_chars;
        let start_byte = s
            .char_indices()
            .nth(skip)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        let tail = &s[start_byte..];
        match tail.find(' ') {
            Some(pos) if pos + 1 < tail.len() => &tail[pos + 1..],
            _ => tail,
        }
    }

    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out = Vec::new();
    let mut word_idx = 0usize;
    let mut pending: Option<String> = None;

    while word_idx < words.len() || pending.is_some() {
        let mut cur = pending.take().unwrap_or_default();
        while word_idx < words.len() {
            let w = words[word_idx];
            let add = if cur.is_empty() { w.len() } else { w.len() + 1 };
            if cur.len() + add > max_chars && !cur.is_empty() {
                break;
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(w);
            word_idx += 1;
        }
        if cur.is_empty() && word_idx < words.len() {
            cur.push_str(words[word_idx]);
            word_idx += 1;
        }
        let chunk = cur.trim().to_string();
        if chunk.is_empty() {
            break;
        }
        out.push(chunk.clone());
        if word_idx >= words.len() {
            break;
        }
        let suf = overlap_suffix(&chunk, overlap);
        if !suf.trim().is_empty() {
            pending = Some(suf.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_text_empty_inputs() {
        assert!(chunk_text_with_overlap("", 100, 10).is_empty());
        assert!(chunk_text_with_overlap("hello", 0, 0).is_empty());
    }

    #[test]
    fn chunk_text_short_text_single_chunk() {
        let chunks = chunk_text_with_overlap("one two three", 100, 10);
        assert_eq!(chunks, vec!["one two three"]);
    }

    #[test]
    fn chunk_text_splits_on_word_boundaries() {
        // 5 words of 4 chars => "aaaa bbbb" is 9 bytes; max 9 => two chunks.
        let chunks = chunk_text_with_overlap("aaaa bbbb cccc dddd", 9, 0);
        assert_eq!(chunks, vec!["aaaa bbbb", "cccc dddd"]);
        for c in &chunks {
            assert!(c.len() <= 9);
            assert!(!c.starts_with(' ') && !c.ends_with(' '));
        }
    }

    #[test]
    fn chunk_text_overlap_carries_suffix_forward() {
        let chunks = chunk_text_with_overlap("aaaa bbbb cccc dddd eeee", 9, 4);
        assert!(chunks.len() >= 2);
        // Each chunk after the first starts with a suffix of the previous chunk.
        for pair in chunks.windows(2) {
            let prev_last_word = pair[0].rsplit(' ').next().unwrap();
            assert!(
                pair[1].contains(prev_last_word),
                "chunk {:?} should overlap with {:?}",
                pair[1],
                pair[0]
            );
        }
    }

    #[test]
    fn chunk_text_oversized_word_not_dropped() {
        // A single word longer than max_chars still gets emitted, not lost.
        let chunks = chunk_text_with_overlap("supercalifragilistic ok", 5, 0);
        assert_eq!(chunks.len(), 2);
        assert!(chunks.iter().any(|c| c == "supercalifragilistic"));
        assert!(chunks.iter().any(|c| c == "ok"));
    }
}
