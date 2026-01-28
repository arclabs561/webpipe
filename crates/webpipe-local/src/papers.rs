//! Paper search helpers (bounded) for research workflows.
//!
//! Motivation:
//! - ArXiv Atom search is useful but brittle for natural-language queries.
//! - Academic discovery often benefits from "semantic" aggregators with richer metadata
//!   (Semantic Scholar, OpenAlex), and (optionally) a Google Scholar API proxy.
//!
//! Safety / boundedness:
//! - All functions are explicitly bounded by `limit` and `timeout_ms`.
//! - No secrets are returned; provider keys are read from env only when needed.
//! - Default field sets avoid full-text downloads.
//!
//! Providers:
//! - Semantic Scholar Graph API (public, rate-limited): https://api.semanticscholar.org/
//! - OpenAlex (public): https://api.openalex.org/
//! - SerpAPI Google Scholar (optional, requires key): https://serpapi.com/google-scholar-api

use crate::{Error, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

fn normalize_ws(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn serpapi_key_from_env() -> Option<String> {
    std::env::var("WEBPIPE_SERPAPI_API_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("SERPAPI_API_KEY")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Paper {
    pub title: String,
    pub year: Option<u32>,
    pub authors: Vec<String>,
    pub venue: Option<String>,
    pub url: Option<String>,
    pub pdf_url: Option<String>,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub citation_count: Option<u64>,
    pub abstract_text: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PaperSearchResponse {
    pub ok: bool,
    pub query: String,
    pub backends: Vec<String>,
    pub papers: Vec<Paper>,
    pub warnings: Vec<&'static str>,
    pub timings_ms: BTreeMap<String, u128>,
}

fn clamp_years(years: &[u32]) -> Vec<u32> {
    let mut ys: Vec<u32> = years
        .iter()
        .copied()
        .filter(|y| (1800..=2500).contains(y))
        .collect();
    ys.sort();
    ys.dedup();
    ys
}

fn stable_dedupe_key(p: &Paper) -> String {
    if let Some(doi) = p.doi.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return format!("doi:{}", doi.to_ascii_lowercase());
    }
    if let Some(aid) = p
        .arxiv_id
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return format!("arxiv:{}", aid.to_ascii_lowercase());
    }
    if let Some(url) = p.url.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return format!("url:{}", url.to_ascii_lowercase());
    }
    let mut k = p.title.to_ascii_lowercase();
    k = normalize_ws(&k);
    if let Some(y) = p.year {
        k.push_str(&format!(":{y}"));
    }
    format!("title:{k}")
}

fn sort_papers(mut papers: Vec<Paper>) -> Vec<Paper> {
    // Deterministic "usefulness" sort:
    // - higher citation count first (when present)
    // - newer year first
    // - title lexicographic
    papers.sort_by(|a, b| {
        b.citation_count
            .unwrap_or(0)
            .cmp(&a.citation_count.unwrap_or(0))
            .then_with(|| b.year.unwrap_or(0).cmp(&a.year.unwrap_or(0)))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.source.cmp(&b.source))
    });
    papers
}

async fn semantic_scholar_search(
    http: &reqwest::Client,
    query: &str,
    limit: usize,
    years: &[u32],
    timeout_ms: u64,
    include_abstract: bool,
) -> Result<Vec<Paper>> {
    let limit = limit.clamp(1, 50);
    let q = query.trim();
    if q.is_empty() {
        return Err(Error::InvalidUrl("query must be non-empty".to_string()));
    }
    let mut url = reqwest::Url::parse("https://api.semanticscholar.org/graph/v1/paper/search")
        .map_err(|e| Error::Fetch(e.to_string()))?;
    url.query_pairs_mut()
        .append_pair("query", q)
        .append_pair("limit", &limit.to_string());

    // Field selection is important for payload size and determinism.
    let mut fields = vec![
        "title",
        "year",
        "venue",
        "authors",
        "externalIds",
        "url",
        "openAccessPdf",
        "citationCount",
    ];
    if include_abstract {
        fields.push("abstract");
    }
    url.query_pairs_mut()
        .append_pair("fields", &fields.join(","));

    // Best-effort year filtering: Semantic Scholar doesn't reliably support year filters in the
    // Graph endpoint, so we filter client-side.
    let ys = clamp_years(years);

    #[derive(Debug, Deserialize)]
    struct Resp {
        data: Option<Vec<Item>>,
    }
    #[derive(Debug, Deserialize)]
    struct Item {
        title: Option<String>,
        year: Option<u32>,
        venue: Option<String>,
        url: Option<String>,
        #[serde(rename = "citationCount")]
        citation_count: Option<u64>,
        authors: Option<Vec<Author>>,
        #[serde(rename = "externalIds")]
        external_ids: Option<ExternalIds>,
        #[serde(rename = "openAccessPdf")]
        open_access_pdf: Option<OpenAccessPdf>,
        #[serde(rename = "abstract")]
        abstract_text: Option<String>,
    }
    #[derive(Debug, Deserialize)]
    struct Author {
        name: Option<String>,
    }
    #[derive(Debug, Deserialize)]
    struct ExternalIds {
        #[serde(rename = "DOI")]
        doi: Option<String>,
        #[serde(rename = "ArXiv")]
        arxiv: Option<String>,
    }
    #[derive(Debug, Deserialize)]
    struct OpenAccessPdf {
        url: Option<String>,
    }

    let resp = http
        .get(url)
        .timeout(Duration::from_millis(timeout_ms.max(1000)))
        .send()
        .await
        .map_err(|e| Error::Fetch(e.to_string()))?;
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        return Err(Error::Fetch(format!(
            "semantic_scholar search failed: HTTP {status}"
        )));
    }
    let parsed: Resp = resp.json().await.map_err(|e| Error::Fetch(e.to_string()))?;
    let mut out = Vec::new();
    for it in parsed.data.unwrap_or_default().into_iter() {
        let title = it.title.unwrap_or_default();
        if title.trim().is_empty() {
            continue;
        }
        let year = it.year;
        if !ys.is_empty() && year.is_some_and(|y| !ys.contains(&y)) {
            continue;
        }
        let authors = it
            .authors
            .unwrap_or_default()
            .into_iter()
            .filter_map(|a| a.name)
            .map(|s| normalize_ws(&s))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let (doi, arxiv_id) = it
            .external_ids
            .map(|x| (x.doi, x.arxiv))
            .unwrap_or((None, None));
        out.push(Paper {
            title: normalize_ws(&title),
            year,
            authors,
            venue: it.venue.map(|s| normalize_ws(&s)).filter(|s| !s.is_empty()),
            url: it.url,
            pdf_url: it.open_access_pdf.and_then(|p| p.url),
            doi,
            arxiv_id,
            citation_count: it.citation_count,
            abstract_text: it
                .abstract_text
                .map(|s| normalize_ws(&s))
                .filter(|s| !s.is_empty()),
            source: "semantic_scholar".to_string(),
        });
    }
    Ok(out)
}

async fn openalex_search(
    http: &reqwest::Client,
    query: &str,
    limit: usize,
    years: &[u32],
    timeout_ms: u64,
    include_abstract: bool,
) -> Result<Vec<Paper>> {
    let limit = limit.clamp(1, 200);
    let q = query.trim();
    if q.is_empty() {
        return Err(Error::InvalidUrl("query must be non-empty".to_string()));
    }
    let mut url = reqwest::Url::parse("https://api.openalex.org/works")
        .map_err(|e| Error::Fetch(e.to_string()))?;
    url.query_pairs_mut()
        .append_pair("search", q)
        .append_pair("per-page", &limit.to_string());
    // Keep response stable/lean.
    let mut select = vec![
        "id",
        "display_name",
        "publication_year",
        "primary_location",
        "host_venue",
        "authorships",
        "doi",
        "cited_by_count",
        "open_access",
    ];
    if include_abstract {
        select.push("abstract_inverted_index");
    }
    url.query_pairs_mut()
        .append_pair("select", &select.join(","));

    let ys = clamp_years(years);
    if !ys.is_empty() {
        // OpenAlex supports a filter syntax. Use exact years set if it's small; otherwise skip.
        // Keep it bounded and simple.
        if ys.len() <= 10 {
            let years_csv = ys
                .iter()
                .map(|y| y.to_string())
                .collect::<Vec<_>>()
                .join("|");
            url.query_pairs_mut()
                .append_pair("filter", &format!("publication_year:{years_csv}"));
        }
    }

    #[derive(Debug, Deserialize)]
    struct Resp {
        results: Option<Vec<Item>>,
    }
    #[derive(Debug, Deserialize)]
    struct Item {
        #[serde(rename = "display_name")]
        display_name: Option<String>,
        #[serde(rename = "publication_year")]
        publication_year: Option<u32>,
        doi: Option<String>,
        #[serde(rename = "cited_by_count")]
        cited_by_count: Option<u64>,
        #[serde(rename = "host_venue")]
        host_venue: Option<HostVenue>,
        #[serde(rename = "primary_location")]
        primary_location: Option<Location>,
        authorships: Option<Vec<Authorship>>,
        #[serde(rename = "abstract_inverted_index")]
        abstract_inverted_index: Option<BTreeMap<String, Vec<u32>>>,
    }
    #[derive(Debug, Deserialize)]
    struct HostVenue {
        #[serde(rename = "display_name")]
        display_name: Option<String>,
    }
    #[derive(Debug, Deserialize)]
    struct Location {
        landing_page_url: Option<String>,
        pdf_url: Option<String>,
    }
    #[derive(Debug, Deserialize)]
    struct Authorship {
        author: Option<AuthorObj>,
    }
    #[derive(Debug, Deserialize)]
    struct AuthorObj {
        #[serde(rename = "display_name")]
        display_name: Option<String>,
    }

    let resp = http
        .get(url)
        .timeout(Duration::from_millis(timeout_ms.max(1000)))
        .send()
        .await
        .map_err(|e| Error::Fetch(e.to_string()))?;
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        return Err(Error::Fetch(format!(
            "openalex search failed: HTTP {status}"
        )));
    }
    let parsed: Resp = resp.json().await.map_err(|e| Error::Fetch(e.to_string()))?;
    let mut out = Vec::new();
    for it in parsed.results.unwrap_or_default().into_iter() {
        let title = it.display_name.unwrap_or_default();
        if title.trim().is_empty() {
            continue;
        }
        let year = it.publication_year;
        if !ys.is_empty() && year.is_some_and(|y| !ys.contains(&y)) {
            continue;
        }
        let authors = it
            .authorships
            .unwrap_or_default()
            .into_iter()
            .filter_map(|a| a.author.and_then(|x| x.display_name))
            .map(|s| normalize_ws(&s))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();

        // OpenAlex DOI often comes as a URL like "https://doi.org/10...."
        let doi = it.doi.as_deref().map(|s| s.trim()).and_then(|s| {
            if s.is_empty() {
                None
            } else if let Some(tail) = s.strip_prefix("https://doi.org/") {
                Some(tail.to_string())
            } else {
                Some(s.to_string())
            }
        });

        let venue = it
            .host_venue
            .and_then(|v| v.display_name)
            .map(|s| normalize_ws(&s))
            .filter(|s| !s.is_empty());
        let url = it
            .primary_location
            .as_ref()
            .and_then(|l| l.landing_page_url.clone());
        let pdf_url = it.primary_location.and_then(|l| l.pdf_url);

        let abstract_text = it
            .abstract_inverted_index
            .map(|inv| inverted_index_to_text(&inv))
            .filter(|s| !s.trim().is_empty());

        out.push(Paper {
            title: normalize_ws(&title),
            year,
            authors,
            venue,
            url,
            pdf_url,
            doi,
            arxiv_id: None,
            citation_count: it.cited_by_count,
            abstract_text,
            source: "openalex".to_string(),
        });
    }
    Ok(out)
}

fn inverted_index_to_text(inv: &BTreeMap<String, Vec<u32>>) -> String {
    // OpenAlex "abstract_inverted_index" maps token -> positions.
    // Reconstruct a best-effort abstract string deterministically.
    let mut positions: BTreeMap<u32, String> = BTreeMap::new();
    for (tok, ps) in inv {
        for p in ps {
            // If collisions occur (shouldn't), keep the first token lexicographically.
            positions.entry(*p).or_insert_with(|| tok.clone());
        }
    }
    let mut out = String::new();
    for (_p, tok) in positions {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&tok);
    }
    normalize_ws(&out)
}

async fn serpapi_google_scholar_search(
    http: &reqwest::Client,
    query: &str,
    limit: usize,
    years: &[u32],
    timeout_ms: u64,
) -> Result<Vec<Paper>> {
    let api_key = serpapi_key_from_env().ok_or_else(|| {
        Error::NotConfigured("missing WEBPIPE_SERPAPI_API_KEY (or SERPAPI_API_KEY)".to_string())
    })?;
    let limit = limit.clamp(1, 20); // keep cost bounded; SerpAPI plans vary
    let q = query.trim();
    if q.is_empty() {
        return Err(Error::InvalidUrl("query must be non-empty".to_string()));
    }
    let ys = clamp_years(years);
    let ylo = ys.iter().min().copied();
    let yhi = ys.iter().max().copied();

    let mut url = reqwest::Url::parse("https://serpapi.com/search.json")
        .map_err(|e| Error::Fetch(e.to_string()))?;
    url.query_pairs_mut()
        .append_pair("engine", "google_scholar")
        .append_pair("q", q)
        .append_pair("api_key", &api_key)
        .append_pair("num", &limit.to_string());
    if let Some(y) = ylo {
        url.query_pairs_mut().append_pair("as_ylo", &y.to_string());
    }
    if let Some(y) = yhi {
        url.query_pairs_mut().append_pair("as_yhi", &y.to_string());
    }

    #[derive(Debug, Deserialize)]
    struct Resp {
        #[serde(rename = "organic_results")]
        organic_results: Option<Vec<Item>>,
    }
    #[derive(Debug, Deserialize)]
    struct Item {
        title: Option<String>,
        link: Option<String>,
        #[serde(rename = "publication_info")]
        publication_info: Option<PubInfo>,
        #[serde(rename = "inline_links")]
        inline_links: Option<InlineLinks>,
        snippet: Option<String>,
    }
    #[derive(Debug, Deserialize)]
    struct PubInfo {
        summary: Option<String>,
        authors: Option<Vec<ScholarAuthor>>,
        year: Option<u32>,
    }
    #[derive(Debug, Deserialize)]
    struct ScholarAuthor {
        name: Option<String>,
    }
    #[derive(Debug, Deserialize)]
    struct InlineLinks {
        #[serde(rename = "cited_by")]
        cited_by: Option<CitedBy>,
    }
    #[derive(Debug, Deserialize)]
    struct CitedBy {
        total: Option<u64>,
    }

    let resp = http
        .get(url)
        .timeout(Duration::from_millis(timeout_ms.max(1000)))
        .send()
        .await
        .map_err(|e| Error::Fetch(e.to_string()))?;
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        return Err(Error::Fetch(format!(
            "serpapi scholar failed: HTTP {status}"
        )));
    }
    let parsed: Resp = resp.json().await.map_err(|e| Error::Fetch(e.to_string()))?;
    let mut out = Vec::new();
    for it in parsed.organic_results.unwrap_or_default().into_iter() {
        let title = it.title.unwrap_or_default();
        if title.trim().is_empty() {
            continue;
        }
        let (authors, year, venue) = match it.publication_info {
            Some(pi) => {
                let authors = pi
                    .authors
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|a| a.name)
                    .map(|s| normalize_ws(&s))
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>();
                let year = pi.year;
                // Summary is often "X - Journal, YEAR - publisher"
                let venue = pi
                    .summary
                    .map(|s| normalize_ws(&s))
                    .filter(|s| !s.is_empty());
                (authors, year, venue)
            }
            None => (Vec::new(), None, None),
        };
        if !ys.is_empty() && year.is_some_and(|y| !ys.contains(&y)) {
            continue;
        }
        let citation_count = it
            .inline_links
            .and_then(|l| l.cited_by)
            .and_then(|c| c.total);
        out.push(Paper {
            title: normalize_ws(&title),
            year,
            authors,
            venue,
            url: it.link,
            pdf_url: None,
            doi: None,
            arxiv_id: None,
            citation_count,
            abstract_text: it
                .snippet
                .map(|s| normalize_ws(&s))
                .filter(|s| !s.is_empty()),
            source: "google_scholar_serpapi".to_string(),
        });
    }
    Ok(out)
}

pub async fn paper_search(
    http: reqwest::Client,
    query: String,
    backends: Vec<String>,
    years: Vec<u32>,
    limit: usize,
    timeout_ms: u64,
    include_abstract: bool,
) -> Result<PaperSearchResponse> {
    let t0 = Instant::now();
    let q = query.trim().to_string();
    if q.is_empty() {
        return Err(Error::InvalidUrl("query must be non-empty".to_string()));
    }
    let years = clamp_years(&years);
    let limit = limit.clamp(1, 50);
    let timeout_ms = timeout_ms.max(1000).min(60_000);

    // Normalize backends list. Default: semantic_scholar + openalex.
    let mut bset: BTreeSet<String> = BTreeSet::new();
    if backends.is_empty() {
        bset.insert("semantic_scholar".to_string());
        bset.insert("openalex".to_string());
    } else {
        for b in backends {
            let s = b.trim().to_ascii_lowercase();
            if s.is_empty() {
                continue;
            }
            bset.insert(s);
        }
    }
    let mut backends: Vec<String> = bset.into_iter().collect();

    let mut timings_ms: BTreeMap<String, u128> = BTreeMap::new();
    let mut warnings: Vec<&'static str> = Vec::new();
    let mut all: Vec<Paper> = Vec::new();

    for b in backends.iter() {
        let t = Instant::now();
        let r = match b.as_str() {
            "semantic_scholar" | "semanticscholar" | "s2" => {
                semantic_scholar_search(&http, &q, limit, &years, timeout_ms, include_abstract)
                    .await
            }
            "openalex" => {
                openalex_search(&http, &q, limit, &years, timeout_ms, include_abstract).await
            }
            "google_scholar" | "scholar" | "google_scholar_serpapi" | "serpapi" => {
                // SerpAPI is optional; return a clear NotConfigured error.
                serpapi_google_scholar_search(&http, &q, limit.min(20), &years, timeout_ms).await
            }
            _ => {
                warnings.push("unknown_paper_backend_ignored");
                Ok(Vec::new())
            }
        };
        timings_ms.insert(format!("backend_{b}"), t.elapsed().as_millis());
        match r {
            Ok(mut ps) => {
                for p in ps.iter_mut() {
                    // Normalize source label to the configured backend token (stable).
                    p.source = match b.as_str() {
                        "semantic_scholar" | "semanticscholar" | "s2" => "semantic_scholar",
                        "openalex" => "openalex",
                        "google_scholar" | "scholar" | "google_scholar_serpapi" | "serpapi" => {
                            "google_scholar_serpapi"
                        }
                        other => other,
                    }
                    .to_string();
                }
                all.append(&mut ps);
            }
            Err(e) => {
                // Keep the overall response usable: treat backend failures as warnings and continue.
                // Callers can inspect which backend was configured and add their own retry logic.
                let msg = e.to_string();
                if msg.to_ascii_lowercase().contains("notconfigured")
                    || msg.to_ascii_lowercase().contains("missing")
                {
                    warnings.push("paper_backend_not_configured");
                } else {
                    warnings.push("paper_backend_failed");
                }
            }
        }
    }

    // Stable dedup.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut deduped: Vec<Paper> = Vec::new();
    for p in all.into_iter() {
        let k = stable_dedupe_key(&p);
        if seen.contains(&k) {
            continue;
        }
        seen.insert(k);
        deduped.push(p);
    }
    let papers = sort_papers(deduped);

    timings_ms.insert("total".to_string(), t0.elapsed().as_millis());
    // Normalize list of backends actually requested (original order is already sorted by BTreeSet).
    // Replace aliases with canonical tokens for stable reporting.
    for b in backends.iter_mut() {
        *b = match b.as_str() {
            "semanticscholar" | "s2" => "semantic_scholar".to_string(),
            "scholar" | "google_scholar" | "serpapi" => "google_scholar_serpapi".to_string(),
            other => other.to_string(),
        };
    }
    backends.sort();
    backends.dedup();

    Ok(PaperSearchResponse {
        ok: true,
        query: q,
        backends,
        papers,
        warnings,
        timings_ms,
    })
}
