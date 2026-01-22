use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Instant;
use webpipe_core::{Error, Result, SearchProvider, SearchQuery, SearchResponse, SearchResult};

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

fn searxng_endpoint_from_env() -> Option<String> {
    std::env::var("WEBPIPE_SEARXNG_ENDPOINT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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
    endpoint: String,
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
        let endpoint = searxng_endpoint_from_env()
            .ok_or_else(|| Error::NotConfigured("missing WEBPIPE_SEARXNG_ENDPOINT".to_string()))?;
        Ok(Self { client, endpoint })
    }

    fn endpoint_search(&self) -> String {
        // Accept either a base URL (…/), or a full /search endpoint.
        let mut base = self.endpoint.trim().trim_end_matches('/').to_string();
        if !base.ends_with("/search") {
            base.push_str("/search");
        }
        base
    }
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

        let resp = req.send().await.map_err(|e| Error::Search(e.to_string()))?;
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
        let t0 = Instant::now();
        let max_results = q.max_results.unwrap_or(10).min(20);

        let mut req = self
            .client
            .get(self.endpoint_search())
            .query(&[("q", q.query.as_str()), ("format", "json")]);

        // Best-effort hints: SearXNG supports `language` on many instances.
        if let Some(lang) = q.language.as_deref() {
            req = req.query(&[("language", lang)]);
        }

        let resp = req.send().await.map_err(|e| Error::Search(e.to_string()))?;
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
}
