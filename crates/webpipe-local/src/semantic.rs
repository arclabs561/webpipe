//! Lightweight semantic reranking.
//!
//! This is intentionally self-contained (no external embeddings backends).
//! It provides a best-effort “semantic-ish” score based on token overlap,
//! which is often good enough to improve chunk ordering without network calls.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SemanticChunk {
    pub start_char: usize,
    pub end_char: usize,
    pub score: f32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticRerankResult {
    pub ok: bool,
    pub backend: String,
    pub model_id: Option<String>,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub chunks: Vec<SemanticChunk>,
    pub warnings: Vec<&'static str>,
}

fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            cur.push(c);
        } else if !cur.is_empty() {
            if cur.len() >= 2 {
                out.push(cur.clone());
            }
            cur.clear();
        }
    }
    if !cur.is_empty() && cur.len() >= 2 {
        out.push(cur);
    }
    out.sort();
    out.dedup();
    out
}

fn overlap_score(query_toks: &[String], text_toks: &[String]) -> f32 {
    if query_toks.is_empty() || text_toks.is_empty() {
        return 0.0;
    }
    let mut i = 0usize;
    let mut j = 0usize;
    let mut inter = 0u64;
    while i < query_toks.len() && j < text_toks.len() {
        match query_toks[i].cmp(&text_toks[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                inter += 1;
                i += 1;
                j += 1;
            }
        }
    }
    // Normalize by query size so “covering the query” scores highly.
    inter as f32 / (query_toks.len() as f32)
}

pub fn semantic_rerank_chunks(
    query: &str,
    candidates: &[(usize, usize, String)],
    top_k: usize,
) -> SemanticRerankResult {
    let top_k = top_k.max(1);
    let q = query.trim();
    if q.is_empty() || candidates.is_empty() {
        return SemanticRerankResult {
            ok: true,
            backend: "lexical_overlap".to_string(),
            model_id: None,
            cache_hits: 0,
            cache_misses: 0,
            chunks: Vec::new(),
            warnings: vec!["empty_query_or_candidates"],
        };
    }
    let q_toks = tokenize(q);

    let mut scored: Vec<SemanticChunk> = candidates
        .iter()
        .map(|(s, e, t)| {
            let tt = tokenize(t);
            let score = overlap_score(&q_toks, &tt);
            SemanticChunk {
                start_char: *s,
                end_char: *e,
                score,
                text: t.clone(),
            }
        })
        .collect();

    // Stable: score desc, then start/end asc.
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.start_char.cmp(&b.start_char))
            .then_with(|| a.end_char.cmp(&b.end_char))
    });
    scored.truncate(top_k);

    SemanticRerankResult {
        ok: true,
        backend: "lexical_overlap".to_string(),
        model_id: None,
        cache_hits: 0,
        cache_misses: 0,
        chunks: scored,
        warnings: Vec::new(),
    }
}
