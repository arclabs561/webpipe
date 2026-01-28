use anyhow::Result;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use webpipe_core::{FetchBackend, FetchCachePolicy, FetchRequest, SearchProvider, SearchQuery};

fn now_epoch_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_lines(path: &Path) -> Result<Vec<String>> {
    let s = fs::read_to_string(path)?;
    Ok(s.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect())
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    Ok(())
}

fn truncate_chars(s: &str, max_chars: usize) -> (String, bool) {
    if max_chars == 0 {
        return ("".to_string(), !s.is_empty());
    }
    let mut out = String::new();
    for (n, ch) in s.chars().enumerate() {
        if n >= max_chars {
            return (out, true);
        }
        out.push(ch);
    }
    (out, false)
}

#[derive(Debug, Clone)]
pub struct DomainPackSpec {
    /// Comma-separated domain ids: arxiv, news, code (and future ids).
    pub domains: Vec<String>,
    /// Max queries per domain (bounded).
    pub per_domain: usize,
    /// Seed for deterministic ordering/selection.
    pub seed: u64,
    /// Output path (json).
    pub out: PathBuf,
    /// Optional notes to embed in the JSON (for provenance).
    pub notes: Option<String>,
}

fn slugify_id(s: &str, max_len: usize) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in s.chars() {
        let c = ch.to_ascii_lowercase();
        let ok = c.is_ascii_alphanumeric();
        if ok {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= max_len {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "q".to_string()
    } else {
        out
    }
}

fn xorshift64star_next(state: &mut u64) -> u64 {
    // Deterministic, tiny RNG (no external deps).
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    x.wrapping_mul(2685821657736338717)
}

fn shuffle_deterministic<T>(items: &mut [T], seed: u64) {
    if items.len() <= 1 {
        return;
    }
    let mut s = if seed == 0 { 0x9e3779b97f4a7c15 } else { seed };
    for i in (1..items.len()).rev() {
        let r = xorshift64star_next(&mut s);
        let j = (r as usize) % (i + 1);
        items.swap(i, j);
    }
}

fn domain_templates(domain: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    // (tags, seed_urls, query_templates)
    let (tags, seed_urls, templates): (Vec<&'static str>, Vec<&'static str>, Vec<&'static str>) =
        match domain {
            "arxiv" | "research" | "academic" => (
                vec!["academic", "arxiv", "research", "live"],
                vec![
                    "https://arxiv.org/list/cs.AI/recent",
                    "https://arxiv.org/list/cs.CL/recent",
                    "https://arxiv.org/list/cs.LG/recent",
                ],
                vec![
                    "recent arxiv papers on tool-using LLM agents",
                    "arxiv 2025 2026 best new methods for retrieval augmented generation",
                    "recent arxiv papers on structured output / constrained decoding for LLMs",
                    "latest arxiv papers on evaluation of LLM reasoning and reliability",
                    "recent arxiv papers on embeddings, reranking, and retrieval benchmarks",
                    "arxiv papers on web agents that browse and cite sources",
                    "recent arxiv papers on information extraction from PDFs and HTML at scale",
                    "arxiv survey paper on MCP protocol or tool calling interfaces",
                    "recent arxiv papers on scalable search + ranking systems for LLMs",
                ],
            ),
            "news" => (
                vec!["news", "live"],
                vec![
                    "https://news.ycombinator.com/",
                    "https://en.wikipedia.org/wiki/Portal:Current_events",
                ],
                vec![
                    "latest AI policy and regulation developments 2025 2026 summary",
                    "recent changes in major LLM APIs: structured output, reasoning controls, tools",
                    "recent open source releases for retrieval and embeddings infrastructure",
                    "breaking news summary with citations for todays top 3 tech stories",
                    "what changed this week in AI tooling and model providers",
                    "recent security incidents involving AI supply chain or dependency confusion",
                    "latest benchmark results for LLMs and how they were measured",
                ],
            ),
            "code" | "github" => (
                vec!["code", "github", "live"],
                // Note: seed URLs are *only* a fallback for code.
                // For code queries, we prefer query-specific search URLs to avoid landing-page chrome.
                vec!["https://github.com/search?q=mcp&type=repositories"],
                vec![
                    "open source MCP server implementations in Rust",
                    "rmcp rust mcp server examples tool definitions and schema validation",
                    "best practices for bounded web scraping and extraction in Rust",
                    "examples of OpenAI-compatible chat completion servers for testing",
                    "how projects implement JSON-mode / structured outputs across providers",
                    "open source reranking and embeddings pipelines (semantic search) examples",
                    "examples of tool calling evaluation harnesses and judge agents",
                    "repos that implement arxiv search and PDF extraction pipelines",
                ],
            ),
            _other => (
                vec!["live", "misc"],
                vec![],
                vec![
                    "recent work and best practices",
                    "what others have done in open source",
                    "examples and references",
                ],
            ),
        };

    (
        tags.into_iter().map(|s| s.to_string()).collect(),
        seed_urls.into_iter().map(|s| s.to_string()).collect(),
        templates.into_iter().map(|s| s.to_string()).collect(),
    )
}

fn url_encode_query_component(s: &str) -> String {
    // Minimal URL encoding for query components.
    // - ASCII alnum and -_.~ are left as-is
    // - space becomes '+'
    // - everything else percent-encoded as UTF-8 bytes
    let mut out = String::new();
    for b in s.as_bytes() {
        let ch = *b as char;
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' || ch == '~' {
            out.push(ch);
        } else if ch == ' ' {
            out.push('+');
        } else {
            out.push('%');
            out.push_str(&format!("{:02X}", b));
        }
    }
    out
}

fn code_seed_urls_for_query(q: &str) -> Vec<String> {
    // For code queries, use query-specific search pages that (usually) contain actual repo lists.
    // This is more “contentful” than Topics/Trending pages which are heavy on UI chrome.
    let qq = url_encode_query_component(q.trim());
    vec![
        format!("https://github.com/search?q={qq}&type=repositories"),
        // Secondary index: GitHub "topics" landing for MCP is still useful for discovery.
        "https://github.com/topics/mcp".to_string(),
    ]
}

pub fn generate_domain_pack(spec: DomainPackSpec) -> Result<PathBuf> {
    let per_domain = spec.per_domain.clamp(1, 50);
    let mut domains: Vec<String> = spec
        .domains
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if domains.is_empty() {
        domains = vec!["arxiv".to_string(), "news".to_string(), "code".to_string()];
    }

    let mut queries_out: Vec<serde_json::Value> = Vec::new();
    for d in domains {
        let (tags, seed_urls, mut templates) = domain_templates(&d);
        shuffle_deterministic(&mut templates, spec.seed ^ (d.len() as u64));
        for (i, q) in templates.into_iter().take(per_domain).enumerate() {
            let slug = slugify_id(&q, 42);
            let query_id = format!("{d}-{i:02}-{slug}");
            let url_paths = if d == "code" || d == "github" {
                code_seed_urls_for_query(&q)
            } else {
                seed_urls.clone()
            };
            queries_out.push(serde_json::json!({
                "query_id": query_id,
                "query": q,
                "tags": tags,
                "url_paths": url_paths,
            }));
        }
    }

    let notes = spec.notes.unwrap_or_else(|| {
        "Domain-generated live query pack (templates; intended for follow-up judging and iteration)."
            .to_string()
    });

    let payload = serde_json::json!({
        "schema_version": 1,
        "kind": "webpipe_e2e_queries",
        "notes": notes,
        "queries": queries_out
    });

    ensure_parent_dir(&spec.out)?;
    fs::write(&spec.out, serde_json::to_string_pretty(&payload)?)?;
    Ok(spec.out)
}

pub struct EvalSearchSpec {
    pub queries: Vec<String>,
    pub providers: Vec<String>, // brave, tavily
    pub max_results: usize,
    pub language: Option<String>,
    pub country: Option<String>,
    pub out: PathBuf,
    pub now_epoch_s: Option<u64>,
}

pub async fn eval_search(spec: EvalSearchSpec) -> Result<PathBuf> {
    let generated_at_epoch_s = spec.now_epoch_s.unwrap_or_else(now_epoch_s);

    // Keep CLI harness aligned with the MCP tool: always 1..=20 results.
    let max_results = spec.max_results.clamp(1, 20);

    let mut runs = Vec::new();
    for q in &spec.queries {
        let qobj = SearchQuery {
            query: q.clone(),
            max_results: Some(max_results),
            language: spec.language.clone(),
            country: spec.country.clone(),
        };

        let mut per_provider = Vec::new();
        for p in &spec.providers {
            let t0 = std::time::Instant::now();
            let res: serde_json::Value = match p.as_str() {
                "brave" => {
                    let client = reqwest::Client::new();
                    match webpipe_local::search::BraveSearchProvider::from_env(client) {
                        Ok(provider) => match provider.search(&qobj).await {
                            Ok(r) => serde_json::json!({
                                "schema_version": 1,
                                "ok": true,
                                "provider": r.provider,
                                "query": qobj.query,
                                "max_results": max_results,
                                "cost_units": r.cost_units,
                                "timings_ms": r.timings_ms,
                                "results": r.results
                            }),
                            Err(e) => serde_json::json!({
                                "schema_version": 1,
                                "ok": false,
                                "provider": "brave",
                                "query": qobj.query,
                                "max_results": max_results,
                                "error": { "code": "search_failed", "message": e.to_string() }
                            }),
                        },
                        Err(e) => serde_json::json!({
                            "schema_version": 1,
                            "ok": false,
                            "provider": "brave",
                            "query": qobj.query,
                            "max_results": max_results,
                            "error": { "code": "not_configured", "message": e.to_string() }
                        }),
                    }
                }
                "tavily" => {
                    let client = reqwest::Client::new();
                    match webpipe_local::search::TavilySearchProvider::from_env(client.clone()) {
                        Ok(provider) => match provider.search(&qobj).await {
                            Ok(r) => serde_json::json!({
                                "schema_version": 1,
                                "ok": true,
                                "provider": r.provider,
                                "query": qobj.query,
                                "max_results": max_results,
                                "cost_units": r.cost_units,
                                "timings_ms": r.timings_ms,
                                "results": r.results
                            }),
                            Err(e) => {
                                let msg = e.to_string();
                                if msg.contains("HTTP 433") {
                                    if let Ok(brave) =
                                        webpipe_local::search::BraveSearchProvider::from_env(client)
                                    {
                                        match brave.search(&qobj).await {
                                            Ok(r) => serde_json::json!({
                                                "schema_version": 1,
                                                "ok": true,
                                                "provider": r.provider,
                                                "query": qobj.query,
                                                "max_results": max_results,
                                                "fallback": {
                                                    "requested_provider": "tavily",
                                                    "reason": "tavily_http_433",
                                                    "tavily_error": msg
                                                },
                                                "cost_units": r.cost_units,
                                                "timings_ms": r.timings_ms,
                                                "results": r.results
                                            }),
                                            Err(e2) => serde_json::json!({
                                                "schema_version": 1,
                                                "ok": false,
                                                "provider": "brave",
                                                "query": qobj.query,
                                                "max_results": max_results,
                                                "fallback": {
                                                    "requested_provider": "tavily",
                                                    "reason": "tavily_http_433",
                                                    "tavily_error": msg
                                                },
                                                "error": { "code": "search_failed", "message": e2.to_string() }
                                            }),
                                        }
                                    } else {
                                        serde_json::json!({
                                            "schema_version": 1,
                                            "ok": false,
                                            "provider": "tavily",
                                            "query": qobj.query,
                                            "max_results": max_results,
                                            "error": { "code": "provider_unavailable", "message": msg }
                                        })
                                    }
                                } else {
                                    serde_json::json!({
                                        "schema_version": 1,
                                        "ok": false,
                                        "provider": "tavily",
                                        "query": qobj.query,
                                        "max_results": max_results,
                                        "error": { "code": "search_failed", "message": msg }
                                    })
                                }
                            }
                        },
                        Err(e) => serde_json::json!({
                            "schema_version": 1,
                            "ok": false,
                            "provider": "tavily",
                            "query": qobj.query,
                            "max_results": max_results,
                            "error": { "code": "not_configured", "message": e.to_string() }
                        }),
                    }
                }
                other => serde_json::json!({
                    "schema_version": 1,
                    "ok": false,
                    "provider": other,
                    "query": qobj.query,
                    "max_results": max_results,
                    "error": { "code": "invalid_params", "message": format!("unknown provider: {other}") }
                }),
            };

            per_provider.push(serde_json::json!({
                "name": p,
                "elapsed_ms": t0.elapsed().as_millis(),
                "result": res,
            }));
        }

        runs.push(serde_json::json!({
            "query": q,
            "providers": per_provider,
        }));
    }

    let payload = serde_json::json!({
        "schema_version": 1,
        "kind": "eval_search",
        "generated_at_epoch_s": generated_at_epoch_s,
        "inputs": {
            "providers": spec.providers,
            "max_results": max_results,
            "language": spec.language,
            "country": spec.country,
            "query_count": spec.queries.len()
        },
        "runs": runs
    });

    ensure_parent_dir(&spec.out)?;
    fs::write(&spec.out, serde_json::to_string_pretty(&payload)? + "\n")?;
    Ok(spec.out)
}

pub struct EvalFetchSpec {
    pub urls: Vec<String>,
    pub fetchers: Vec<String>, // local, firecrawl
    pub timeout_ms: u64,
    pub max_bytes: u64,
    pub max_text_chars: usize,
    pub extract: bool,
    pub extract_width: usize,
    pub out: PathBuf,
    pub now_epoch_s: Option<u64>,
}

pub async fn eval_fetch(spec: EvalFetchSpec) -> Result<PathBuf> {
    let generated_at_epoch_s = spec.now_epoch_s.unwrap_or_else(now_epoch_s);

    let local_fetcher = webpipe_local::LocalFetcher::new(
        std::env::var("WEBPIPE_CACHE_DIR").ok().map(PathBuf::from),
    )?;

    let mut runs = Vec::new();
    for url in &spec.urls {
        let mut per = Vec::new();

        for f in &spec.fetchers {
            match f.as_str() {
                "local" => {
                    let req = FetchRequest {
                        url: url.clone(),
                        timeout_ms: Some(spec.timeout_ms),
                        max_bytes: Some(spec.max_bytes),
                        headers: BTreeMap::new(),
                        cache: FetchCachePolicy::default(),
                    };
                    let t0 = std::time::Instant::now();
                    let resp = local_fetcher.fetch(&req).await;
                    match resp {
                        Ok(r) => {
                            let text = r.text_lossy();
                            let (text_trunc, clipped) = truncate_chars(&text, spec.max_text_chars);

                            let extracted = if spec.extract {
                                let cfg = webpipe_local::extract::ExtractPipelineCfg {
                                    query: None,
                                    width: spec.extract_width,
                                    max_chars: spec.max_text_chars,
                                    top_chunks: 0,
                                    max_chunk_chars: 0,
                                    include_structure: false,
                                    max_outline_items: 0,
                                    max_blocks: 0,
                                    max_block_chars: 0,
                                };
                                let pipe = webpipe_local::extract::extract_pipeline_from_bytes(
                                    &r.bytes,
                                    r.content_type.as_deref(),
                                    &r.final_url,
                                    cfg,
                                );
                                Some(serde_json::json!({
                                    "engine": pipe.extracted.engine,
                                    "width": spec.extract_width,
                                    "truncated": pipe.text_truncated,
                                    "text": pipe.extracted.text,
                                    "warnings": pipe.extracted.warnings
                                }))
                            } else {
                                None
                            };
                            per.push(serde_json::json!({
                                "name": "local",
                                "ok": true,
                                "elapsed_ms": t0.elapsed().as_millis(),
                                "status": r.status,
                                "content_type": r.content_type,
                                "bytes": r.bytes.len(),
                                "truncated": r.truncated || clipped,
                                "source": match r.source { webpipe_core::FetchSource::Cache => "cache", webpipe_core::FetchSource::Network => "network" },
                                "text": text_trunc,
                                "extracted": extracted
                            }));
                        }
                        Err(e) => {
                            per.push(serde_json::json!({"name":"local","ok":false,"elapsed_ms": t0.elapsed().as_millis(),"error": e.to_string()}));
                        }
                    }
                }
                "firecrawl" => {
                    let client = reqwest::Client::new();
                    let t0 = std::time::Instant::now();
                    let fc = match webpipe_local::firecrawl::FirecrawlClient::from_env(client) {
                        Ok(x) => x,
                        Err(e) => {
                            per.push(serde_json::json!({"name":"firecrawl","ok":false,"elapsed_ms": t0.elapsed().as_millis(),"error": e.to_string()}));
                            continue;
                        }
                    };
                    match fc.fetch_markdown(url, spec.timeout_ms, None).await {
                        Ok(r) => {
                            let (text_trunc, clipped) =
                                truncate_chars(&r.markdown, spec.max_text_chars);
                            per.push(serde_json::json!({
                                "name":"firecrawl",
                                "ok": true,
                                "elapsed_ms": t0.elapsed().as_millis(),
                                "status": 200,
                                "content_type": "text/markdown",
                                "bytes": r.markdown.len(),
                                "truncated": clipped,
                                "source": "network",
                                "text": text_trunc
                            }));
                        }
                        Err(e) => {
                            per.push(serde_json::json!({"name":"firecrawl","ok":false,"elapsed_ms": t0.elapsed().as_millis(),"error": e.to_string()}));
                        }
                    }
                }
                other => per.push(
                    serde_json::json!({"name": other, "ok": false, "error": "unknown fetcher"}),
                ),
            }
        }

        // simple similarity between first two successful text outputs (if any)
        let mut metrics = serde_json::json!({});
        if per.len() >= 2 {
            let a = per[0].get("text").and_then(|v| v.as_str()).unwrap_or("");
            let b = per[1].get("text").and_then(|v| v.as_str()).unwrap_or("");
            let sim = webpipe_local::compare::text_jaccard(a, b, 3);
            metrics = serde_json::json!({"text_jaccard_k3": sim});

            if spec.extract {
                let ax = per[0]
                    .get("extracted")
                    .and_then(|v| v.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let bx = per[1].get("text").and_then(|v| v.as_str()).unwrap_or("");
                let simx = webpipe_local::compare::text_jaccard(ax, bx, 3);
                metrics["extracted_vs_other_text_jaccard_k3"] = serde_json::json!(simx);
            }
        }

        runs.push(serde_json::json!({
            "url": url,
            "fetchers": per,
            "metrics": metrics
        }));
    }

    let payload = serde_json::json!({
        "schema_version": 1,
        "kind": "eval_fetch",
        "generated_at_epoch_s": generated_at_epoch_s,
        "inputs": {
            "fetchers": spec.fetchers,
            "timeout_ms": spec.timeout_ms,
            "max_bytes": spec.max_bytes,
            "max_text_chars": spec.max_text_chars,
            "extract": spec.extract,
            "extract_width": spec.extract_width,
            "url_count": spec.urls.len()
        },
        "runs": runs
    });

    ensure_parent_dir(&spec.out)?;
    fs::write(&spec.out, serde_json::to_string_pretty(&payload)? + "\n")?;
    Ok(spec.out)
}

pub fn load_queries(files: &[PathBuf], inline: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for p in files {
        if p.extension().and_then(|e| e.to_str()) == Some("json") {
            // Structured fixture format (v1): see `fixtures/e2e_queries_v1.json`.
            let v = load_e2e_queries_v1(p)?;
            out.extend(v.queries.into_iter().map(|q| q.query));
        } else {
            out.extend(read_lines(p)?);
        }
    }
    out.extend(inline.iter().cloned());
    Ok(out)
}

pub fn load_urls(files: &[PathBuf], inline: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for p in files {
        out.extend(read_lines(p)?);
    }
    out.extend(inline.iter().cloned());
    Ok(out)
}

#[derive(Debug, Deserialize)]
pub struct E2eQueriesV1 {
    pub schema_version: u64,
    pub kind: String,
    pub queries: Vec<E2eQueryV1>,
}

#[derive(Debug, Deserialize)]
pub struct E2eQueryV1 {
    pub query_id: String,
    pub query: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub url_paths: Vec<String>,
}

pub fn load_e2e_queries_v1(path: &Path) -> Result<E2eQueriesV1> {
    let raw = fs::read_to_string(path)?;
    let v: E2eQueriesV1 = serde_json::from_str(&raw)?;
    if v.schema_version != 1 || v.kind != "webpipe_e2e_queries" {
        anyhow::bail!("unexpected e2e queries kind/schema_version");
    }
    for q in &v.queries {
        if q.query_id.trim().is_empty() {
            anyhow::bail!("e2e queries: query_id must be non-empty");
        }
        if q.query.trim().is_empty() {
            anyhow::bail!("e2e queries: query must be non-empty");
        }
        if q.tags.iter().any(|t| t.trim().is_empty()) {
            anyhow::bail!("e2e queries: tags must not contain empty strings");
        }
        if q.url_paths.iter().any(|p| p.trim().is_empty()) {
            anyhow::bail!("e2e queries: url_paths must not contain empty strings");
        }
    }
    Ok(v)
}

#[derive(Debug, Deserialize)]
pub struct E2eQrelsV1 {
    pub schema_version: u64,
    pub kind: String,
    pub qrels: Vec<E2eQrelV1>,
}

#[derive(Debug, Deserialize)]
pub struct E2eQrelV1 {
    pub query_id: String,
    pub expected_url_substrings: Vec<String>,
}

pub fn load_e2e_qrels_v1(path: &Path) -> Result<E2eQrelsV1> {
    let raw = fs::read_to_string(path)?;
    let v: E2eQrelsV1 = serde_json::from_str(&raw)?;
    if v.schema_version != 1 || v.kind != "webpipe_e2e_qrels" {
        anyhow::bail!("unexpected e2e qrels kind/schema_version");
    }
    for q in &v.qrels {
        if q.query_id.trim().is_empty() {
            anyhow::bail!("e2e qrels: query_id must be non-empty");
        }
        if q.expected_url_substrings.is_empty() {
            anyhow::bail!("e2e qrels: expected_url_substrings must be non-empty");
        }
        if q.expected_url_substrings
            .iter()
            .any(|s| s.trim().is_empty())
        {
            anyhow::bail!("e2e qrels: expected_url_substrings must not contain empty strings");
        }
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn truncate_chars_is_unicode_safe() {
        let s = "café 👨\u{200D}👩\u{200D}👧\u{200D}👦";
        let (t, clipped) = truncate_chars(s, 5);
        assert!(t.chars().count() <= 5);
        assert!(clipped);
    }

    #[test]
    fn read_lines_ignores_blanks_and_hash_comments() {
        let mut f = tempfile::NamedTempFile::new().expect("tmp");
        writeln!(
            f,
            r#"
            # comment
            hello

            world
            # another comment
            "#
        )
        .unwrap();
        let got = read_lines(f.path()).expect("read_lines");
        assert_eq!(got, vec!["hello".to_string(), "world".to_string()]);
    }

    #[test]
    fn fixtures_are_parseable() {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let queries = read_lines(&base.join("queries.txt")).expect("queries fixture readable");
        let urls = read_lines(&base.join("urls.txt")).expect("urls fixture readable");
        assert!(!queries.is_empty());
        assert!(!urls.is_empty());
    }

    #[test]
    fn seed_queries_and_qrels_are_parseable_json() {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let q_raw =
            std::fs::read_to_string(base.join("queries_seed.json")).expect("queries_seed.json");
        let qv: serde_json::Value =
            serde_json::from_str(&q_raw).expect("queries_seed.json valid json");
        assert_eq!(qv["schema_version"].as_u64(), Some(1));
        assert_eq!(qv["kind"].as_str(), Some("webpipe_seed_queries"));
        assert!(!qv["queries"].as_array().unwrap().is_empty());

        let r_raw = std::fs::read_to_string(base.join("qrels_seed.json")).expect("qrels_seed.json");
        let rv: serde_json::Value =
            serde_json::from_str(&r_raw).expect("qrels_seed.json valid json");
        assert_eq!(rv["schema_version"].as_u64(), Some(1));
        assert_eq!(rv["kind"].as_str(), Some("webpipe_seed_qrels"));
        assert!(!rv["qrels"].as_array().unwrap().is_empty());
    }

    #[test]
    fn e2e_queries_fixture_is_parseable_json() {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let v =
            load_e2e_queries_v1(&base.join("e2e_queries_v1.json")).expect("e2e_queries_v1.json");
        assert_eq!(v.schema_version, 1);
        assert_eq!(v.kind, "webpipe_e2e_queries");
        assert!(!v.queries.is_empty());
        assert!(v.queries.iter().all(|q| !q.query_id.trim().is_empty()));
        assert!(v.queries.iter().all(|q| !q.query.trim().is_empty()));
    }

    #[test]
    fn e2e_qrels_fixture_is_parseable_json() {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let v = load_e2e_qrels_v1(&base.join("e2e_qrels_v1.json")).expect("e2e_qrels_v1.json");
        assert_eq!(v.schema_version, 1);
        assert_eq!(v.kind, "webpipe_e2e_qrels");
        assert!(!v.qrels.is_empty());
        assert!(v.qrels.iter().all(|q| !q.query_id.trim().is_empty()));
        assert!(v
            .qrels
            .iter()
            .all(|q| !q.expected_url_substrings.is_empty()));
    }
}
