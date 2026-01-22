use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("fetch failed: {0}")]
    Fetch(String),
    #[error("cache error: {0}")]
    Cache(String),
    #[error("search failed: {0}")]
    Search(String),
    #[error("llm failed: {0}")]
    Llm(String),
    #[error("not configured: {0}")]
    NotConfigured(String),
    #[error("not supported: {0}")]
    NotSupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Budget {
    /// Best-effort cost units (your own units; provider adapters map into it).
    pub max_cost_units: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchCachePolicy {
    /// If true, allow reading from cache.
    pub read: bool,
    /// If true, allow writing to cache.
    pub write: bool,
    /// If set, cached entries older than this are treated as a miss.
    pub ttl_s: Option<u64>,
}

impl Default for FetchCachePolicy {
    fn default() -> Self {
        Self {
            read: true,
            write: true,
            ttl_s: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchRequest {
    pub url: String,
    /// Timeout for the operation (network + processing).
    pub timeout_ms: Option<u64>,
    /// Hard cap on bytes read from the response body.
    pub max_bytes: Option<u64>,
    /// Optional headers to add (best-effort; adapter may drop unsafe headers).
    pub headers: BTreeMap<String, String>,
    pub cache: FetchCachePolicy,
}

impl FetchRequest {
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout_ms.map(Duration::from_millis)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FetchSource {
    Cache,
    Network,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResponse {
    pub url: String,
    pub final_url: String,
    pub status: u16,
    pub content_type: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub bytes: Vec<u8>,
    pub truncated: bool,
    pub source: FetchSource,
    pub timings_ms: BTreeMap<String, u128>,
}

impl FetchResponse {
    pub fn text_lossy(&self) -> String {
        String::from_utf8_lossy(&self.bytes).to_string()
    }
}

#[async_trait::async_trait]
pub trait FetchBackend: Send + Sync {
    async fn fetch(&self, req: &FetchRequest) -> Result<FetchResponse>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    pub max_results: Option<usize>,
    pub language: Option<String>,
    pub country: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub url: String,
    pub title: Option<String>,
    pub snippet: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub provider: String,
    pub cost_units: u64,
    pub timings_ms: BTreeMap<String, u128>,
}

#[async_trait::async_trait]
pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn search(&self, q: &SearchQuery) -> Result<SearchResponse>;
}
