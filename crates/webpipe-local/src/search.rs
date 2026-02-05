use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Instant;
use webpipe_core::{Error, Result, SearchProvider, SearchQuery, SearchResponse, SearchResult};

fn timeout_ms_from_query(q: &SearchQuery) -> u64 {
    // Provider requests can hang indefinitely without an explicit timeout.
    // Keep a conservative cap even if callers pass something huge.
    q.timeout_ms.unwrap_or(20_000).clamp(1_000, 60_000)
}

fn brave_api_key_from_env() -> Option<String> {
    std::env::var("WEBPIPE_BRAVE_API_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("BRAVE_SEARCH_API_KEY")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
}

fn tavily_api_key_from_env() -> Option<String> {
    std::env::var("WEBPIPE_TAVILY_API_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("TAVILY_API_KEY")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
}

fn brave_endpoint_from_env() -> Option<String> {
    std::env::var("WEBPIPE_BRAVE_ENDPOINT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn tavily_endpoint_from_env() -> Option<String> {
    std::env::var("WEBPIPE_TAVILY_ENDPOINT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn searxng_endpoints_from_env() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    // Allow a comma/whitespace-separated list of endpoints for simple load spreading.
    if let Ok(v) = std::env::var("WEBPIPE_SEARXNG_ENDPOINTS") {
        for raw in v.split(|c: char| c == ',' || c.is_whitespace()) {
            let s = raw.trim();
            if s.is_empty() {
                continue;
            }
            let s = s.to_string();
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }

    // Back-compat: single endpoint.
    if let Ok(v) = std::env::var("WEBPIPE_SEARXNG_ENDPOINT") {
        let s = v.trim().to_string();
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    }

    out
}

#[derive(Debug, Clone)]
pub struct BraveSearchProvider {
    client: reqwest::Client,
    api_key: String,
}

#[derive(Debug, Clone)]
pub struct TavilySearchProvider {
    client: reqwest::Client,
    api_key: String,
}

#[derive(Debug, Clone)]
pub struct SearxngSearchProvider {
    client: reqwest::Client,
    endpoints: Vec<String>,
}

impl TavilySearchProvider {
    pub fn from_env(client: reqwest::Client) -> Result<Self> {
        let api_key = tavily_api_key_from_env().ok_or_else(|| {
            Error::NotConfigured("missing WEBPIPE_TAVILY_API_KEY (or TAVILY_API_KEY)".to_string())
        })?;
        Ok(Self { client, api_key })
    }

    fn endpoint() -> String {
        tavily_endpoint_from_env().unwrap_or_else(|| "https://api.tavily.com/search".to_string())
    }
}

impl BraveSearchProvider {
    pub fn from_env(client: reqwest::Client) -> Result<Self> {
        let api_key = brave_api_key_from_env().ok_or_else(|| {
            Error::NotConfigured(
                "missing WEBPIPE_BRAVE_API_KEY (or BRAVE_SEARCH_API_KEY)".to_string(),
            )
        })?;
        Ok(Self { client, api_key })
    }

    fn endpoint() -> String {
        // Docs: https://api.search.brave.com/res/v1/web/search
        brave_endpoint_from_env()
            .unwrap_or_else(|| "https://api.search.brave.com/res/v1/web/search".to_string())
    }
}

impl SearxngSearchProvider {
    pub fn from_env(client: reqwest::Client) -> Result<Self> {
        let endpoints = searxng_endpoints_from_env();
        if endpoints.is_empty() {
            return Err(Error::NotConfigured(
                "missing WEBPIPE_SEARXNG_ENDPOINT (or WEBPIPE_SEARXNG_ENDPOINTS)".to_string(),
            ));
        }
        Ok(Self { client, endpoints })
    }

    fn endpoint_search_for(base_endpoint: &str) -> String {
        // Accept either a base URL (…/), or a full /search endpoint.
        let mut base = base_endpoint.trim().trim_end_matches('/').to_string();
        if !base.ends_with("/search") {
            base.push_str("/search");
        }
        base
    }

    fn stable_hash64(query: &SearchQuery) -> u64 {
        // Stable across runs (unlike HashMap's RandomState).
        // FNV-1a over a few “routing” fields.
        let mut h: u64 = 1469598103934665603;
        for b in query.query.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(1099511628211);
        }
        if let Some(lang) = query.language.as_deref() {
            for b in lang.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(1099511628211);
            }
        }
        if let Some(country) = query.country.as_deref() {
            for b in country.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(1099511628211);
            }
        }
        h
    }

    fn pick_endpoint_index(&self, q: &SearchQuery) -> usize {
        if self.endpoints.is_empty() {
            return 0;
        }
        (Self::stable_hash64(q) as usize) % self.endpoints.len()
    }

    // endpoint_search removed: we now route via `searxng_search_at_endpoint`.
}

pub async fn searxng_search_at_endpoint(
    client: &reqwest::Client,
    base_endpoint: &str,
    q: &SearchQuery,
) -> Result<SearchResponse> {
    let t0 = Instant::now();
    let max_results = q.max_results.unwrap_or(10).min(20);
    let timeout_ms = timeout_ms_from_query(q);

    let endpoint_search = SearxngSearchProvider::endpoint_search_for(base_endpoint);
    let mut req = client
        .get(endpoint_search)
        .query(&[("q", q.query.as_str()), ("format", "json")]);

    // Best-effort hints: SearXNG supports `language` on many instances.
    if let Some(lang) = q.language.as_deref() {
        req = req.query(&[("language", lang)]);
    }

    let resp = req
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .send()
        .await
        .map_err(|e| Error::Search(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Search(format!("searxng search HTTP {status}")));
    }

    let parsed: SearxngSearchResponse = resp
        .json()
        .await
        .map_err(|e| Error::Search(e.to_string()))?;

    let mut out = Vec::new();
    if let Some(rs) = parsed.results {
        for r in rs.into_iter().take(max_results) {
            let Some(url) = r.url else { continue };
            out.push(SearchResult {
                url,
                title: r.title,
                snippet: r.content,
                source: "searxng".to_string(),
            });
        }
    }

    let mut timings_ms = BTreeMap::new();
    timings_ms.insert("search".to_string(), t0.elapsed().as_millis());

    Ok(SearchResponse {
        results: out,
        provider: "searxng".to_string(),
        // Best-effort: treat SearXNG as “free” in our cost accounting unless the user
        // wants to map it to a budget externally.
        cost_units: 0,
        timings_ms,
    })
}

#[derive(Debug, Deserialize)]
struct BraveWebSearchResponse {
    web: Option<BraveWeb>,
}

#[derive(Debug, Deserialize)]
struct BraveWeb {
    results: Option<Vec<BraveWebResult>>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResult {
    url: String,
    title: Option<String>,
    #[serde(rename = "description")]
    description: Option<String>,
}

#[async_trait::async_trait]
impl SearchProvider for BraveSearchProvider {
    fn name(&self) -> &'static str {
        "brave"
    }

    async fn search(&self, q: &SearchQuery) -> Result<SearchResponse> {
        let t0 = Instant::now();
        let timeout_ms = timeout_ms_from_query(q);

        let mut req = self
            .client
            .get(Self::endpoint())
            .header("X-Subscription-Token", &self.api_key)
            .query(&[("q", q.query.as_str())]);

        if let Some(n) = q.max_results {
            // Brave uses `count` for result count.
            req = req.query(&[("count", n.to_string())]);
        }
        if let Some(lang) = q.language.as_deref() {
            // Best-effort: Brave docs include optional knobs; treat these as hints.
            req = req.query(&[("search_lang", lang)]);
        }
        if let Some(country) = q.country.as_deref() {
            req = req.query(&[("country", country)]);
        }

        let resp = req
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .send()
            .await
            .map_err(|e| Error::Search(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Search(format!("brave search HTTP {status}")));
        }

        let parsed: BraveWebSearchResponse = resp
            .json()
            .await
            .map_err(|e| Error::Search(e.to_string()))?;
        let mut out = Vec::new();
        if let Some(web) = parsed.web {
            if let Some(results) = web.results {
                for r in results {
                    out.push(SearchResult {
                        url: r.url,
                        title: r.title,
                        snippet: r.description,
                        source: "brave".to_string(),
                    });
                }
            }
        }

        let mut timings_ms = BTreeMap::new();
        timings_ms.insert("search".to_string(), t0.elapsed().as_millis());

        Ok(SearchResponse {
            results: out,
            provider: "brave".to_string(),
            cost_units: 1,
            timings_ms,
        })
    }
}

#[derive(Debug, Deserialize)]
struct TavilySearchResponse {
    results: Vec<TavilyResult>,
    usage: Option<TavilyUsage>,
}

#[derive(Debug, Deserialize)]
struct TavilyUsage {
    credits: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TavilyResult {
    url: String,
    title: Option<String>,
    content: Option<String>,
}

#[async_trait::async_trait]
impl SearchProvider for TavilySearchProvider {
    fn name(&self) -> &'static str {
        "tavily"
    }

    async fn search(&self, q: &SearchQuery) -> Result<SearchResponse> {
        let t0 = Instant::now();
        let max_results = q.max_results.unwrap_or(5).min(20);
        let timeout_ms = timeout_ms_from_query(q);

        let body = serde_json::json!({
            "query": q.query,
            "max_results": max_results,
            // Keep it comparable to Brave: don't ask for answer/raw_content.
            "include_answer": false,
            "include_raw_content": false,
            "search_depth": "basic",
            "include_usage": true,
            // Best-effort hints; Tavily accepts country only for some topics, but it is safe to send.
            "country": q.country,
        });

        let resp = self
            .client
            .post(Self::endpoint())
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.api_key),
            )
            .json(&body)
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .send()
            .await
            .map_err(|e| Error::Search(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Search(format!("tavily search HTTP {status}")));
        }

        let parsed: TavilySearchResponse = resp
            .json()
            .await
            .map_err(|e| Error::Search(e.to_string()))?;

        let mut out = Vec::new();
        for r in parsed.results {
            out.push(SearchResult {
                url: r.url,
                title: r.title,
                snippet: r.content,
                source: "tavily".to_string(),
            });
        }

        let mut timings_ms = BTreeMap::new();
        timings_ms.insert("search".to_string(), t0.elapsed().as_millis());

        Ok(SearchResponse {
            results: out,
            provider: "tavily".to_string(),
            cost_units: parsed.usage.and_then(|u| u.credits).unwrap_or(1),
            timings_ms,
        })
    }
}

#[derive(Debug, Deserialize)]
struct SearxngSearchResponse {
    results: Option<Vec<SearxngResult>>,
}

#[derive(Debug, Deserialize)]
struct SearxngResult {
    url: Option<String>,
    title: Option<String>,
    // SearXNG uses `content` for snippets in JSON format.
    content: Option<String>,
}

#[async_trait::async_trait]
impl SearchProvider for SearxngSearchProvider {
    fn name(&self) -> &'static str {
        "searxng"
    }

    async fn search(&self, q: &SearchQuery) -> Result<SearchResponse> {
        // Deterministic sharding when multiple endpoints are configured.
        let idx = self.pick_endpoint_index(q);
        let base_endpoint = self.endpoints.get(idx).map(|s| s.as_str()).unwrap_or("");
        searxng_search_at_endpoint(&self.client, base_endpoint, q).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvGuard {
        k: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(k: &'static str, v: &str) -> Self {
            let prev = std::env::var(k).ok();
            std::env::set_var(k, v);
            Self { k, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(v) = self.prev.take() {
                std::env::set_var(self.k, v);
            } else {
                std::env::remove_var(self.k);
            }
        }
    }

    #[test]
    fn empty_api_keys_are_treated_as_missing() {
        let _g1 = EnvGuard::set("WEBPIPE_BRAVE_API_KEY", "");
        let _g2 = EnvGuard::set("WEBPIPE_TAVILY_API_KEY", "   ");
        // These should behave the same as "unset".
        assert!(brave_api_key_from_env().is_none());
        assert!(tavily_api_key_from_env().is_none());
    }

    #[test]
    fn parses_minimal_brave_shape() {
        let js = r#"
        {
          "web": {
            "results": [
              {"url":"https://example.com","title":"Example","description":"Hello"}
            ]
          }
        }
        "#;
        let parsed: BraveWebSearchResponse = serde_json::from_str(js).unwrap();
        let web = parsed.web.unwrap();
        let rs = web.results.unwrap();
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].url, "https://example.com");
        assert_eq!(rs[0].title.as_deref(), Some("Example"));
        assert_eq!(rs[0].description.as_deref(), Some("Hello"));
    }

    #[test]
    fn parses_minimal_tavily_shape() {
        let js = r#"
        {
          "results": [
            {"url":"https://example.com","title":"Example","content":"Hello"}
          ],
          "usage": { "credits": 1 }
        }
        "#;
        let parsed: TavilySearchResponse = serde_json::from_str(js).unwrap();
        assert_eq!(parsed.results.len(), 1);
        assert_eq!(parsed.results[0].url, "https://example.com");
        assert_eq!(parsed.results[0].title.as_deref(), Some("Example"));
        assert_eq!(parsed.results[0].content.as_deref(), Some("Hello"));
        assert_eq!(parsed.usage.unwrap().credits, Some(1));
    }

    #[test]
    fn parses_minimal_searxng_shape() {
        let js = r#"
        {
          "results": [
            {"url":"https://example.com","title":"Example","content":"Hello"}
          ]
        }
        "#;
        let parsed: SearxngSearchResponse = serde_json::from_str(js).unwrap();
        assert_eq!(parsed.results.unwrap().len(), 1);
    }

    #[test]
    fn searxng_endpoints_from_env_accepts_list_and_dedups() {
        let _g1 = EnvGuard::set("WEBPIPE_SEARXNG_ENDPOINTS", "http://a, http://b http://a");
        let _g2 = EnvGuard::set("WEBPIPE_SEARXNG_ENDPOINT", "http://b");
        let eps = searxng_endpoints_from_env();
        assert_eq!(eps, vec!["http://a".to_string(), "http://b".to_string()]);
    }

    #[test]
    fn searxng_endpoint_sharding_is_deterministic_for_same_query() {
        let p = SearxngSearchProvider {
            client: reqwest::Client::new(),
            endpoints: vec!["http://a".to_string(), "http://b".to_string()],
        };
        let q = SearchQuery {
            query: "hello world".to_string(),
            max_results: None,
            language: Some("en".to_string()),
            country: Some("us".to_string()),
            timeout_ms: None,
        };
        let i1 = p.pick_endpoint_index(&q);
        let i2 = p.pick_endpoint_index(&q);
        assert_eq!(i1, i2);
        assert!(i1 < 2);
    }
}
