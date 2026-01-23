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
        if q.expected_url_substrings.iter().any(|s| s.trim().is_empty()) {
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
        let v = load_e2e_queries_v1(&base.join("e2e_queries_v1.json")).expect("e2e_queries_v1.json");
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
