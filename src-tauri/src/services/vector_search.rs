//! On-device vector search over ingested CV/JD chunks. A user has at most a
//! few dozen chunks, so brute-force cosine is instant and needs no index.

/// Serialize an embedding as little-endian f32 bytes for a SQLite BLOB.
pub fn encode_f32(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Inverse of [`encode_f32`]; `None` when the blob isn't a whole number of f32s.
pub fn decode_f32(bytes: &[u8]) -> Option<Vec<f32>> {
    let (words, rest) = bytes.as_chunks::<4>();
    if !rest.is_empty() {
        return None;
    }
    Some(words.iter().map(|w| f32::from_le_bytes(*w)).collect())
}

fn cosine(a: &[f32], b: &[f32]) -> Option<f32> {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    (na > 0.0 && nb > 0.0).then(|| dot / (na * nb))
}

/// The `k` items most similar to `query`, best first. Vectors whose
/// dimension differs from the query (embedded with another model before a
/// settings change) or that have zero norm are skipped, not errors.
pub fn top_k_cosine<T>(query: &[f32], items: Vec<(T, Vec<f32>)>, k: usize) -> Vec<T> {
    let mut scored: Vec<(f32, T)> = items
        .into_iter()
        .filter(|(_, v)| v.len() == query.len())
        .filter_map(|(item, v)| cosine(query, &v).map(|s| (s, item)))
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().take(k).map(|(_, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trips() {
        let v = vec![0.5_f32, -1.25, 3.0];
        assert_eq!(decode_f32(&encode_f32(&v)), Some(v));
    }

    #[test]
    fn decode_rejects_truncated_blob() {
        assert_eq!(decode_f32(&[0, 0, 128]), None);
    }

    #[test]
    fn top_k_orders_by_cosine_similarity() {
        let query = [1.0, 0.0];
        let items = vec![
            ("far", vec![0.0, 1.0]),
            ("near", vec![0.9, 0.1]),
            ("exact", vec![2.0, 0.0]),
        ];
        let ranked: Vec<&str> = top_k_cosine(&query, items, 2);
        assert_eq!(ranked, vec!["exact", "near"]);
    }

    #[test]
    fn top_k_skips_dimension_mismatches_and_zero_vectors() {
        let query = [1.0, 0.0];
        let items = vec![
            ("wrong-dim", vec![1.0, 0.0, 0.0]),
            ("zero", vec![0.0, 0.0]),
            ("ok", vec![1.0, 1.0]),
        ];
        assert_eq!(top_k_cosine(&query, items, 5), vec!["ok"]);
    }

    #[test]
    fn top_k_with_empty_input_is_empty() {
        let none: Vec<(&str, Vec<f32>)> = Vec::new();
        assert!(top_k_cosine(&[1.0], none, 3).is_empty());
    }
}
