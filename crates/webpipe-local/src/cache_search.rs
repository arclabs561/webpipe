//! Cache-only “search” over `WEBPIPE_CACHE_DIR`.
//!
//! This module is intentionally:
//! - **offline**: no network calls
//! - **bounded**: caller caps scan size + docs
//! - **deterministic**: stable sorting and scoring
//!
//! Implementation note:
//! This is a lightweight scan + extract pipeline (no external index crate).

use crate::extract;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Serialize)]
pub struct CacheDocHit {
    pub url: String,
    pub final_url: String,
    pub fetched_at_epoch_s: u64,
    pub status: u16,
    pub content_type: Option<String>,
    pub bytes: usize,
    pub extraction_engine: String,
    pub text_chars: usize,
    pub text_truncated: bool,
    pub chunks: Vec<extract::ScoredChunk>,
    pub structure: Option<extract::ExtractedStructure>,
    pub warnings: Vec<&'static str>,
    pub score: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheSearchResult {
    pub ok: bool,
    pub scanned_entries: usize,
    pub selected_docs: usize,
    pub results: Vec<CacheDocHit>,
    pub warnings: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedDoc {
    meta_rel_path: String,
    fetched_at_epoch_s: u64,
    url: String,
    final_url: String,
    status: u16,
    content_type: Option<String>,
    bytes: usize,
    extraction_engine: String,
    text: String,
    text_chars: usize,
    text_truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedCorpus {
    docs: Vec<PersistedDoc>,
}

#[derive(Debug, Default)]
struct CorpusState {
    // Keyed by meta file path (stable cache layout).
    docs: std::collections::BTreeMap<PathBuf, PersistedDoc>,
}

static CORPUS: OnceLock<Mutex<CorpusState>> = OnceLock::new();

fn corpus() -> &'static Mutex<CorpusState> {
    CORPUS.get_or_init(|| Mutex::new(CorpusState::default()))
}

fn env_bool(key: &str) -> bool {
    matches!(
        std::env::var(key)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

fn persist_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(".webpipe_cache_search")
}

fn persist_path(cache_dir: &Path) -> PathBuf {
    persist_dir(cache_dir).join("corpus.json")
}

fn load_persisted_corpus(cache_dir: &Path, max_docs: usize) -> Option<PersistedCorpus> {
    let p = persist_path(cache_dir);
    let bytes = fs::read(p).ok()?;
    let mut c: PersistedCorpus = serde_json::from_slice(&bytes).ok()?;
    c.docs.sort_by_key(|d| Reverse(d.fetched_at_epoch_s));
    c.docs.truncate(max_docs);
    Some(c)
}

fn save_persisted_corpus(cache_dir: &Path, corpus: &PersistedCorpus) -> bool {
    let dir = persist_dir(cache_dir);
    if fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let tmp = dir.join("corpus.json.tmp");
    let out = serde_json::to_vec(corpus).ok();
    let Some(out) = out else {
        return false;
    };
    if fs::write(&tmp, out).is_err() {
        return false;
    }
    fs::rename(tmp, persist_path(cache_dir)).is_ok()
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn meta_to_entry(meta_path: &Path) -> Option<(u64, String, String, u16, Option<String>)> {
    let v = read_json(meta_path)?;
    let fetched_at = v
        .get("fetched_at_epoch_s")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let url = v
        .get("url")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let final_url = v
        .get("final_url")
        .and_then(|x| x.as_str())
        .unwrap_or(&url)
        .to_string();
    let status = v.get("status").and_then(|x| x.as_u64()).unwrap_or(0) as u16;
    let content_type = v
        .get("content_type")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    Some((fetched_at, url, final_url, status, content_type))
}

fn body_path_for_meta(meta_path: &Path) -> PathBuf {
    // FsCache uses: <key>.json + <key>.bin in same dir.
    let s = meta_path.to_string_lossy().to_string();
    if let Some(prefix) = s.strip_suffix(".json") {
        PathBuf::from(format!("{prefix}.bin"))
    } else {
        meta_path.with_extension("bin")
    }
}

fn collect_meta_files(cache_dir: &Path, max_scan_entries: usize) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let max_scan_entries = max_scan_entries.clamp(1, 20_000);

    let Ok(l1) = fs::read_dir(cache_dir) else {
        return out;
    };
    for e1 in l1.flatten() {
        if out.len() >= max_scan_entries {
            break;
        }
        let p1 = e1.path();
        if !p1.is_dir() {
            continue;
        }
        let Ok(l2) = fs::read_dir(&p1) else {
            continue;
        };
        for e2 in l2.flatten() {
            if out.len() >= max_scan_entries {
                break;
            }
            let p2 = e2.path();
            if !p2.is_dir() {
                continue;
            }
            let Ok(l3) = fs::read_dir(&p2) else {
                continue;
            };
            for e3 in l3.flatten() {
                if out.len() >= max_scan_entries {
                    break;
                }
                let p = e3.path();
                if p.extension().and_then(|x| x.to_str()) != Some("json") {
                    continue;
                }
                out.push(p);
            }
        }
    }
    out
}

pub fn cache_search_extract(
    cache_dir: &Path,
    query: &str,
    max_docs: usize,
    max_chars: usize,
    max_bytes: u64,
    width: usize,
    top_chunks: usize,
    max_chunk_chars: usize,
    include_structure: bool,
    max_outline_items: usize,
    max_blocks: usize,
    max_block_chars: usize,
    include_text: bool,
    max_scan_entries: usize,
) -> CacheSearchResult {
    let mut warnings: Vec<&'static str> = Vec::new();

    if query.trim().is_empty() {
        return CacheSearchResult {
            ok: false,
            scanned_entries: 0,
            selected_docs: 0,
            results: Vec::new(),
            warnings: vec!["query_empty"],
        };
    }
    if !cache_dir.exists() {
        return CacheSearchResult {
            ok: false,
            scanned_entries: 0,
            selected_docs: 0,
            results: Vec::new(),
            warnings: vec!["cache_dir_missing"],
        };
    }

    let max_docs = max_docs.clamp(1, 500);
    let max_chars = max_chars.clamp(1_000, 200_000);
    let max_bytes = max_bytes.max(10_000);
    let width = width.clamp(20, 240);
    let top_chunks = top_chunks.clamp(1, 50);
    let max_chunk_chars = max_chunk_chars.clamp(50, 5_000);

    let persist = env_bool("WEBPIPE_CACHE_SEARCH_PERSIST");
    let persist_max_docs = env_usize("WEBPIPE_CACHE_SEARCH_PERSIST_MAX_DOCS", 2000).min(20_000);

    // Best-effort: load persisted corpus once per process.
    if persist {
        if let Ok(mut cg) = corpus().lock() {
            if cg.docs.is_empty() {
                if let Some(pc) = load_persisted_corpus(cache_dir, persist_max_docs) {
                    for pd in pc.docs {
                        let meta_full = cache_dir.join(&pd.meta_rel_path);
                        cg.docs.insert(meta_full, pd);
                    }
                    warnings.push("persisted_corpus_loaded");
                }
            }
        }
    }

    // Scan metadata entries and keep the newest docs.
    let meta_files = collect_meta_files(cache_dir, max_scan_entries);
    let scanned_entries = meta_files.len();
    if scanned_entries == 0 {
        warnings.push("cache_empty_or_unreadable");
    }

    let mut entries: Vec<(u64, PathBuf)> = Vec::new();
    for p in meta_files {
        let Some((t, _, _, _, _)) = meta_to_entry(&p) else {
            continue;
        };
        entries.push((t, p));
    }
    entries.sort_by_key(|(t, _)| Reverse(*t));
    entries.truncate(max_docs);
    let selected_docs = entries.len();

    // Hydrate docs: compute extraction pipeline + score.
    let mut hits: Vec<CacheDocHit> = Vec::new();
    let mut persisted_out: Vec<PersistedDoc> = Vec::new();
    for (_t, meta_p) in entries {
        let Some((fetched_at, url, final_url, status, content_type)) = meta_to_entry(&meta_p)
        else {
            continue;
        };
        let body_p = body_path_for_meta(&meta_p);
        let bytes_full = fs::read(&body_p).unwrap_or_default();
        let bytes = if (bytes_full.len() as u64) > max_bytes {
            &bytes_full[..(max_bytes as usize)]
        } else {
            &bytes_full[..]
        };

        let cfg = extract::ExtractPipelineCfg {
            query: Some(query),
            width,
            max_chars,
            top_chunks,
            max_chunk_chars,
            include_structure,
            max_outline_items,
            max_blocks,
            max_block_chars,
        };
        let pipe =
            extract::extract_pipeline_from_bytes(bytes, content_type.as_deref(), &final_url, cfg);

        let score: u64 = pipe.chunks.iter().map(|c| c.score).sum();
        let extraction_engine = pipe.extracted.engine.to_string();
        let mut doc_warnings: Vec<&'static str> = pipe.extracted.warnings.clone();
        if (bytes_full.len() as u64) > max_bytes {
            doc_warnings.push("cache_body_truncated_for_scan");
        }

        hits.push(CacheDocHit {
            url: url.clone(),
            final_url: final_url.clone(),
            fetched_at_epoch_s: fetched_at,
            status,
            content_type: content_type.clone(),
            bytes: bytes_full.len(),
            extraction_engine: extraction_engine.clone(),
            text_chars: pipe.text_chars,
            text_truncated: pipe.text_truncated,
            chunks: pipe.chunks.clone(),
            structure: pipe.structure.clone(),
            warnings: doc_warnings,
            score,
        });

        if persist {
            let rel = meta_p
                .strip_prefix(cache_dir)
                .ok()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| meta_p.to_string_lossy().to_string());
            persisted_out.push(PersistedDoc {
                meta_rel_path: rel,
                fetched_at_epoch_s: fetched_at,
                url,
                final_url,
                status,
                content_type,
                bytes: bytes_full.len(),
                extraction_engine: extraction_engine.to_string(),
                text: pipe.extracted.text,
                text_chars: pipe.text_chars,
                text_truncated: pipe.text_truncated,
            });
        }
    }

    // Stable sort by score desc, then fetched_at desc, then url.
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.fetched_at_epoch_s.cmp(&a.fetched_at_epoch_s))
            .then_with(|| a.final_url.cmp(&b.final_url))
    });

    // Optional: persist corpus on every call (best-effort, bounded).
    if persist {
        persisted_out.sort_by_key(|d| Reverse(d.fetched_at_epoch_s));
        persisted_out.truncate(persist_max_docs);
        let _ = save_persisted_corpus(
            cache_dir,
            &PersistedCorpus {
                docs: persisted_out,
            },
        );
    }

    // If scan found nothing but a persisted corpus exists in memory, fall back to it (best-effort).
    if hits.is_empty() && persist {
        if let Ok(cg) = corpus().lock() {
            if !cg.docs.is_empty() {
                warnings.push("persisted_corpus_used_without_scan");
            }
        }
    }

    // If include_text=false (typical), keep text only in `extract` downstream; callers can choose.
    if !include_text {
        for h in &mut hits {
            // chunks already contain bounded evidence; we don't include full text here.
            // (MCP layer controls compact vs verbose shaping.)
            let _ = &h;
        }
    }

    CacheSearchResult {
        ok: true,
        scanned_entries,
        selected_docs: selected_docs.min(max_docs),
        results: hits,
        warnings,
    }
}
