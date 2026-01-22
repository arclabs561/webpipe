use serde::Deserialize;
use std::time::Instant;
use webpipe_core::{Error, Result};

fn firecrawl_api_key_from_env() -> Option<String> {
    std::env::var("WEBPIPE_FIRECRAWL_API_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("FIRECRAWL_API_KEY")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
}

#[derive(Debug, Clone)]
pub struct FirecrawlClient {
    client: reqwest::Client,
    api_key: String,
}

impl FirecrawlClient {
    pub fn from_env(client: reqwest::Client) -> Result<Self> {
        let api_key = firecrawl_api_key_from_env().ok_or_else(|| {
            Error::NotConfigured(
                "missing WEBPIPE_FIRECRAWL_API_KEY (or FIRECRAWL_API_KEY)".to_string(),
            )
        })?;
        Ok(Self { client, api_key })
    }

    fn endpoint_v2() -> String {
        // For tests / enterprise proxies, allow overriding the endpoint.
        //
        // Default: upstream Firecrawl v2 scrape endpoint.
        std::env::var("WEBPIPE_FIRECRAWL_ENDPOINT_V2")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "https://api.firecrawl.dev/v2/scrape".to_string())
    }

    pub async fn fetch_markdown(
        &self,
        url: &str,
        timeout_ms: u64,
        max_age_ms: Option<u64>,
    ) -> Result<FirecrawlScrapeResult> {
        let t0 = Instant::now();

        let body = serde_json::json!({
            "url": url,
            "formats": ["markdown"],
            "onlyMainContent": true,
            "timeout": timeout_ms,
            "maxAge": max_age_ms.unwrap_or(172800000u64)
        });

        let resp = self
            .client
            .post(Self::endpoint_v2())
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.api_key),
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Fetch(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Fetch(format!("firecrawl fetch HTTP {status}")));
        }

        let parsed: FirecrawlScrapeResponse =
            resp.json().await.map_err(|e| Error::Fetch(e.to_string()))?;
        if !parsed.success {
            return Err(Error::Fetch(
                "firecrawl fetch returned success=false".to_string(),
            ));
        }

        Ok(FirecrawlScrapeResult {
            markdown: parsed.data.and_then(|d| d.markdown).unwrap_or_default(),
            elapsed_ms: t0.elapsed().as_millis(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct FirecrawlScrapeResult {
    pub markdown: String,
    pub elapsed_ms: u128,
}

#[derive(Debug, Deserialize)]
struct FirecrawlScrapeResponse {
    success: bool,
    data: Option<FirecrawlScrapeData>,
}

#[derive(Debug, Deserialize)]
struct FirecrawlScrapeData {
    markdown: Option<String>,
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
    fn empty_api_key_is_treated_as_missing() {
        let _g = EnvGuard::set("WEBPIPE_FIRECRAWL_API_KEY", "");
        assert!(firecrawl_api_key_from_env().is_none());
    }

    #[test]
    fn parses_minimal_firecrawl_response_shape() {
        let js = r##"
        { "success": true, "data": { "markdown": "# Hi" } }
        "##;
        let parsed: FirecrawlScrapeResponse = serde_json::from_str(js).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.data.unwrap().markdown.unwrap(), "# Hi");
    }
}
