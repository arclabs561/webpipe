use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use webpipe_core::{Error, FetchBackend, FetchRequest, FetchResponse, FetchSource, Result};

pub mod arxiv;
pub mod cache_search;
pub mod compare;
pub mod extract;
pub mod firecrawl;
pub mod links;
pub mod ollama;
pub mod openai_compat;
pub mod papers;
pub mod perplexity;
pub mod search;
pub mod semantic;
pub mod shellout;
pub mod textprep;
#[cfg(feature = "vision-gemini")]
pub mod vision_gemini;
pub mod youtube;

#[derive(Debug, Clone)]
pub struct FsCache {
    root: PathBuf,
}

impl FsCache {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn cache_meta_headers(headers: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        // Cache metadata should be privacy-safe. Avoid persisting sensitive headers like Set-Cookie.
        //
        // We intentionally keep this allowlist small and expand only when a new field is proven
        // useful and safe.
        let mut out = BTreeMap::new();
        for (k, v) in headers {
            match k.trim().to_ascii_lowercase().as_str() {
                "content-type" | "content-length" | "etag" | "last-modified" | "cache-control" => {
                    out.insert(k.clone(), v.clone());
                }
                _ => {}
            }
        }
        out
    }

    fn key_for_fetch(req: &FetchRequest) -> String {
        Self::key_for_fetch_v2(req)
    }

    fn key_for_fetch_v2(req: &FetchRequest) -> String {
        // Deterministic key: url + relevant knobs. Keep it stable and readable-ish.
        let mut h = Sha256::new();
        h.update(b"url:");
        h.update(req.url.as_bytes());
        h.update(b"\nmax_bytes:");
        match req.max_bytes {
            Some(n) => h.update(n.to_string().as_bytes()),
            None => h.update(b"none"),
        }
        h.update(b"\nheaders:");
        let allow_unsafe = matches!(
            std::env::var("WEBPIPE_ALLOW_UNSAFE_HEADERS")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "1" | "true" | "yes" | "on"
        );
        for (k, v) in &req.headers {
            // Safety default: do not allow request-secret headers to influence caching keys unless
            // explicitly opted in. This avoids "hashing secrets" and avoids cache fragmentation.
            if !allow_unsafe {
                let kl = k.trim().to_ascii_lowercase();
                if matches!(
                    kl.as_str(),
                    "authorization" | "cookie" | "proxy-authorization"
                ) {
                    continue;
                }
            }
            h.update(k.as_bytes());
            h.update(b"=");
            h.update(v.as_bytes());
            h.update(b"\n");
        }
        hex::encode(h.finalize())
    }

    fn key_for_fetch_legacy_v1(req: &FetchRequest) -> String {
        // Legacy key function kept for backwards-compatible cache reads.
        // It treated max_bytes=None as "0", which collides with max_bytes=Some(0).
        let mut h = Sha256::new();
        h.update(b"url:");
        h.update(req.url.as_bytes());
        h.update(b"\nmax_bytes:");
        h.update(req.max_bytes.unwrap_or(0).to_string().as_bytes());
        h.update(b"\nheaders:");
        let allow_unsafe = matches!(
            std::env::var("WEBPIPE_ALLOW_UNSAFE_HEADERS")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "1" | "true" | "yes" | "on"
        );
        for (k, v) in &req.headers {
            if !allow_unsafe {
                let kl = k.trim().to_ascii_lowercase();
                if matches!(
                    kl.as_str(),
                    "authorization" | "cookie" | "proxy-authorization"
                ) {
                    continue;
                }
            }
            h.update(k.as_bytes());
            h.update(b"=");
            h.update(v.as_bytes());
            h.update(b"\n");
        }
        hex::encode(h.finalize())
    }

    fn paths(&self, key: &str) -> (PathBuf, PathBuf) {
        let dir = self.root.join(&key[0..2]).join(&key[2..4]);
        let meta = dir.join(format!("{key}.json"));
        let body = dir.join(format!("{key}.bin"));
        (meta, body)
    }

    pub fn get(&self, req: &FetchRequest) -> Result<Option<FetchResponse>> {
        if !req.cache.read {
            return Ok(None);
        }
        let key_v2 = Self::key_for_fetch_v2(req);
        let (meta_p, body_p) = self.paths(&key_v2);
        let (meta_bytes, body, used_legacy_key) = if meta_p.exists() && body_p.exists() {
            (
                fs::read(&meta_p).map_err(|e| Error::Cache(e.to_string()))?,
                fs::read(&body_p).map_err(|e| Error::Cache(e.to_string()))?,
                false,
            )
        } else {
            let key_legacy = Self::key_for_fetch_legacy_v1(req);
            let (meta_p2, body_p2) = self.paths(&key_legacy);
            if !meta_p2.exists() || !body_p2.exists() {
                return Ok(None);
            }
            (
                fs::read(&meta_p2).map_err(|e| Error::Cache(e.to_string()))?,
                fs::read(&body_p2).map_err(|e| Error::Cache(e.to_string()))?,
                true,
            )
        };

        let mut meta: serde_json::Value =
            serde_json::from_slice(&meta_bytes).map_err(|e| Error::Cache(e.to_string()))?;
        let fetched_at = meta
            .get("fetched_at_epoch_s")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if let Some(ttl_s) = req.cache.ttl_s {
            let now_s = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::from_secs(0))
                .as_secs();
            if now_s.saturating_sub(fetched_at) > ttl_s {
                return Ok(None);
            }
        }

        // Re-hydrate minimal response.
        let status = meta.get("status").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
        let url = meta
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or(&req.url)
            .to_string();
        let final_url = meta
            .get("final_url")
            .and_then(|v| v.as_str())
            .unwrap_or(&req.url)
            .to_string();
        let content_type = meta
            .get("content_type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let truncated = meta
            .get("truncated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut headers = BTreeMap::new();
        if let Some(h) = meta.get_mut("headers").and_then(|v| v.as_object_mut()) {
            for (k, v) in h.iter() {
                if let Some(s) = v.as_str() {
                    headers.insert(k.clone(), s.to_string());
                }
            }
        }

        let out = FetchResponse {
            url,
            final_url,
            status,
            content_type,
            headers,
            bytes: body,
            truncated,
            source: FetchSource::Cache,
            timings_ms: BTreeMap::new(),
        };

        // Best-effort migration: if we hit via a legacy key and writes are enabled,
        // copy into the v2 key space so future reads are fast and unambiguous.
        if used_legacy_key && req.cache.write {
            // Ignore errors: migration should not fail the read path.
            let _ = self.put(req, &out);
        }

        Ok(Some(out))
    }

    pub fn put(&self, req: &FetchRequest, resp: &FetchResponse) -> Result<()> {
        if !req.cache.write {
            return Ok(());
        }
        let key = Self::key_for_fetch(req);
        let (meta_p, body_p) = self.paths(&key);
        if let Some(parent) = meta_p.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::Cache(e.to_string()))?;
        }
        let now_s = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::from_secs(0))
            .as_secs();

        let meta = serde_json::json!({
            "schema_version": 1,
            "fetched_at_epoch_s": now_s,
            "url": resp.url,
            "final_url": resp.final_url,
            "status": resp.status,
            "content_type": resp.content_type,
            "headers": Self::cache_meta_headers(&resp.headers),
            "truncated": resp.truncated,
        });

        fs::write(&body_p, &resp.bytes).map_err(|e| Error::Cache(e.to_string()))?;
        fs::write(
            &meta_p,
            serde_json::to_vec(&meta).map_err(|e| Error::Cache(e.to_string()))?,
        )
        .map_err(|e| Error::Cache(e.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct LocalFetcher {
    client: reqwest::Client,
    cache: Option<FsCache>,
}

impl LocalFetcher {
    pub fn new(cache_dir: Option<PathBuf>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("webpipe-local/0.1")
            .redirect(reqwest::redirect::Policy::limited(10))
            // Safety defaults: avoid “hang forever” on DNS/TLS/body stalls.
            // Per-request timeouts (FetchRequest.timeout_ms) can still override this.
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| Error::Fetch(e.to_string()))?;
        let cache = cache_dir.map(FsCache::new);
        Ok(Self { client, cache })
    }

    fn allow_unsafe_request_headers() -> bool {
        // Safety default: do not forward secrets (Authorization/Cookie) to arbitrary URLs.
        // Opt-in escape hatch for debugging / private controlled endpoints only.
        matches!(
            std::env::var("WEBPIPE_ALLOW_UNSAFE_HEADERS")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "1" | "true" | "yes" | "on"
        )
    }

    fn is_sensitive_request_header(name: &reqwest::header::HeaderName) -> bool {
        // Avoid relying on constant equality here; compare by normalized name.
        // (Header names are case-insensitive; HeaderName::as_str() is canonical lower-case.)
        matches!(
            name.as_str(),
            "authorization" | "cookie" | "proxy-authorization"
        )
    }

    fn default_cache_dir() -> PathBuf {
        // Keep it local + user-owned; caller can override.
        std::env::temp_dir().join("webpipe-cache")
    }

    pub fn with_default_cache() -> Result<Self> {
        Self::new(Some(Self::default_cache_dir()))
    }

    /// Cache-only lookup. Never performs a network request.
    ///
    /// Returns:
    /// - Ok(Some(resp)) if the cache has a fresh entry (respecting ttl_s)
    /// - Ok(None) if cache is disabled or a miss/expired
    pub fn cache_get(&self, req: &FetchRequest) -> Result<Option<FetchResponse>> {
        let Some(cache) = self.cache.clone() else {
            return Ok(None);
        };
        cache.get(req)
    }

    fn apply_headers(
        &self,
        mut rb: reqwest::RequestBuilder,
        headers: &BTreeMap<String, String>,
    ) -> reqwest::RequestBuilder {
        let allow_unsafe = Self::allow_unsafe_request_headers();
        for (k, v) in headers {
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(v),
            ) {
                if !allow_unsafe && Self::is_sensitive_request_header(&name) {
                    continue;
                }
                rb = rb.header(name, value);
            }
        }
        rb
    }
}

#[async_trait::async_trait]
impl FetchBackend for LocalFetcher {
    async fn fetch(&self, req: &FetchRequest) -> Result<FetchResponse> {
        let mut timings_ms = BTreeMap::new();

        if let Some(cache) = self.cache.clone() {
            let req2 = req.clone();
            let t0 = std::time::Instant::now();
            let hit = tokio::task::spawn_blocking(move || cache.get(&req2))
                .await
                .map_err(|e| Error::Cache(format!("cache get join failed: {e}")))??;
            timings_ms.insert("cache_get".to_string(), t0.elapsed().as_millis());
            if let Some(mut hit) = hit {
                hit.timings_ms = timings_ms;
                return Ok(hit);
            }
        }

        let t_req = std::time::Instant::now();
        let url = url::Url::parse(&req.url).map_err(|e| Error::InvalidUrl(e.to_string()))?;

        // YouTube: transcript-first via yt-dlp (opt-in/auto).
        //
        // Rationale: HTML scraping is brittle; yt-dlp already implements the moving target logic.
        // We convert transcripts into a stable text/plain body so the rest of the pipeline (chunking,
        // cache search, semantic rerank) can reuse existing logic.
        let yt_mode = youtube::youtube_transcripts_mode_from_env();
        let yt_enabled = yt_mode != "off";
        if yt_enabled && youtube::youtube_video_id(&url).is_some() {
            let timeout = req.timeout().unwrap_or(Duration::from_secs(20));
            let url_s = req.url.clone();
            let t0 = std::time::Instant::now();
            let r = tokio::task::spawn_blocking(move || {
                youtube::fetch_transcript_via_ytdlp(&url_s, timeout)
            })
            .await
            .map_err(|e| Error::Fetch(format!("youtube transcript join failed: {e}")))?;
            match r {
                Ok(text) => {
                    timings_ms.insert("youtube_transcript".to_string(), t0.elapsed().as_millis());
                    let mut bytes = text.into_bytes();
                    let max_bytes = req.max_bytes.unwrap_or(u64::MAX) as usize;
                    let mut truncated = false;
                    if bytes.len() > max_bytes {
                        bytes.truncate(max_bytes);
                        truncated = true;
                    }
                    let out = FetchResponse {
                        url: req.url.clone(),
                        final_url: req.url.clone(),
                        status: 200,
                        content_type: Some("text/x-youtube-transcript".to_string()),
                        headers: BTreeMap::new(),
                        bytes,
                        truncated,
                        source: FetchSource::Network,
                        timings_ms: timings_ms.clone(),
                    };
                    if let Some(cache) = self.cache.clone() {
                        let req2 = req.clone();
                        let out2 = out.clone();
                        let t_put = std::time::Instant::now();
                        tokio::task::spawn_blocking(move || cache.put(&req2, &out2))
                            .await
                            .map_err(|e| Error::Cache(format!("cache put join failed: {e}")))??;
                        timings_ms.insert("cache_put".to_string(), t_put.elapsed().as_millis());
                    }
                    return Ok(FetchResponse { timings_ms, ..out });
                }
                Err(e) => {
                    if yt_mode == "yt-dlp" || yt_mode == "strict" {
                        return Err(Error::Fetch(format!("youtube transcript failed: {e}")));
                    }
                    // auto mode: fall through to normal HTML fetch.
                }
            }
        }

        let mut rb = self.client.get(url);
        if let Some(to) = req.timeout() {
            rb = rb.timeout(to);
        }
        rb = self.apply_headers(rb, &req.headers);
        let resp = rb.send().await.map_err(|e| Error::Fetch(e.to_string()))?;
        let final_url = resp.url().to_string();
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let mut headers = BTreeMap::new();
        for (k, v) in resp.headers().iter() {
            if let Ok(s) = v.to_str() {
                headers.insert(k.as_str().to_string(), s.to_string());
            }
        }

        let max_bytes = req.max_bytes.unwrap_or(u64::MAX) as usize;
        let mut truncated = false;
        let mut bytes = Vec::new();
        let mut stream = resp.bytes_stream();
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| Error::Fetch(e.to_string()))?;
            if bytes.len().saturating_add(chunk.len()) > max_bytes {
                let can_take = max_bytes.saturating_sub(bytes.len());
                bytes.extend_from_slice(&chunk[..can_take]);
                truncated = true;
                break;
            }
            bytes.extend_from_slice(&chunk);
        }

        timings_ms.insert("network_fetch".to_string(), t_req.elapsed().as_millis());
        let out = FetchResponse {
            url: req.url.clone(),
            final_url,
            status,
            content_type,
            headers,
            bytes,
            truncated,
            source: FetchSource::Network,
            timings_ms: timings_ms.clone(),
        };

        if let Some(cache) = self.cache.clone() {
            let req2 = req.clone();
            let out2 = out.clone();
            let t_put = std::time::Instant::now();
            tokio::task::spawn_blocking(move || cache.put(&req2, &out2))
                .await
                .map_err(|e| Error::Cache(format!("cache put join failed: {e}")))??;
            timings_ms.insert("cache_put".to_string(), t_put.elapsed().as_millis());
        }

        Ok(FetchResponse { timings_ms, ..out })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::header, http::StatusCode, routing::get, Router};
    use proptest::prelude::*;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::sync::Mutex;
    use webpipe_core::FetchCachePolicy;

    // Env vars are process-global; serialize tests that mutate them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test]
    async fn local_fetcher_hits_cache() {
        let app = Router::new().route(
            "/",
            get(|| async { ([(header::CONTENT_TYPE, "text/plain")], "hello") }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let tmp = tempfile::tempdir().unwrap();
        let fetcher = LocalFetcher::new(Some(tmp.path().to_path_buf())).unwrap();

        let req = FetchRequest {
            url: format!("http://{}/", addr),
            timeout_ms: Some(2_000),
            max_bytes: Some(1_000_000),
            headers: BTreeMap::new(),
            cache: FetchCachePolicy {
                read: true,
                write: true,
                ttl_s: Some(60),
            },
        };

        let r1 = fetcher.fetch(&req).await.unwrap();
        assert_eq!(r1.source, FetchSource::Network);
        let r2 = fetcher.fetch(&req).await.unwrap();
        assert_eq!(r2.source, FetchSource::Cache);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn local_fetcher_drops_sensitive_request_headers_by_default() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("WEBPIPE_ALLOW_UNSAFE_HEADERS");

        let app = Router::new().route(
            "/",
            get(|headers: axum::http::HeaderMap| async move {
                // Fail closed: if the client forwarded secrets, we error.
                let mut present = Vec::new();
                if headers.contains_key(header::AUTHORIZATION) {
                    present.push("authorization");
                }
                if headers.contains_key(header::COOKIE) {
                    present.push("cookie");
                }
                if headers.contains_key(header::PROXY_AUTHORIZATION) {
                    present.push("proxy-authorization");
                }
                if !present.is_empty() {
                    return (
                        StatusCode::BAD_REQUEST,
                        format!("sensitive header was forwarded: {}", present.join(",")),
                    );
                }
                // Echo back a couple safe headers for observability.
                let accept_lang = headers
                    .get(header::ACCEPT_LANGUAGE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                (StatusCode::OK, format!("ok accept-language={accept_lang}"))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let fetcher = LocalFetcher::new(None).unwrap();

        let mut hdrs = BTreeMap::new();
        hdrs.insert("Authorization".to_string(), "Bearer secret".to_string());
        hdrs.insert("Cookie".to_string(), "session=secret".to_string());
        hdrs.insert(
            "Proxy-Authorization".to_string(),
            "Basic secret".to_string(),
        );
        hdrs.insert("Accept-Language".to_string(), "en-US".to_string());

        let req = FetchRequest {
            url: format!("http://{}/", addr),
            timeout_ms: Some(2_000),
            max_bytes: Some(100_000),
            headers: hdrs,
            cache: FetchCachePolicy {
                read: false,
                write: false,
                ttl_s: None,
            },
        };

        let resp = fetcher.fetch(&req).await.unwrap();
        let body = resp.text_lossy();
        assert_eq!(
            resp.status, 200,
            "unexpected status={} body={body}",
            resp.status
        );
        assert!(body.contains("ok accept-language=en-US"));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn local_fetcher_can_forward_sensitive_headers_when_explicitly_allowed() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("WEBPIPE_ALLOW_UNSAFE_HEADERS", "true");

        let app = Router::new().route(
            "/",
            get(|headers: axum::http::HeaderMap| async move {
                // Here we *expect* Authorization to be forwarded (explicit opt-in).
                let auth = headers
                    .get(header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                if auth.is_empty() {
                    return (StatusCode::BAD_REQUEST, "missing authorization".to_string());
                }
                (StatusCode::OK, format!("auth={auth}"))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let fetcher = LocalFetcher::new(None).unwrap();
        let mut hdrs = BTreeMap::new();
        hdrs.insert("authorization".to_string(), "Bearer ok".to_string()); // lower-case to test normalization

        let req = FetchRequest {
            url: format!("http://{}/", addr),
            timeout_ms: Some(2_000),
            max_bytes: Some(100_000),
            headers: hdrs,
            cache: FetchCachePolicy {
                read: false,
                write: false,
                ttl_s: None,
            },
        };

        let resp = fetcher.fetch(&req).await.unwrap();
        assert_eq!(resp.status, 200);
        assert!(resp.text_lossy().contains("auth=Bearer ok"));

        // Clean up to avoid leaking env state across tests.
        std::env::remove_var("WEBPIPE_ALLOW_UNSAFE_HEADERS");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn cache_key_ignores_sensitive_headers_by_default_but_not_when_opted_in() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let app = Router::new().route(
            "/",
            get(|| async { ([(header::CONTENT_TYPE, "text/plain")], "hello") }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let tmp = tempfile::tempdir().unwrap();
        let fetcher = LocalFetcher::new(Some(tmp.path().to_path_buf())).unwrap();

        let base_req = FetchRequest {
            url: format!("http://{}/", addr),
            timeout_ms: Some(2_000),
            max_bytes: Some(100_000),
            headers: BTreeMap::new(),
            cache: FetchCachePolicy {
                read: true,
                write: true,
                ttl_s: Some(60),
            },
        };

        // Default: sensitive headers must NOT affect the cache key.
        std::env::remove_var("WEBPIPE_ALLOW_UNSAFE_HEADERS");
        {
            let mut req_a = base_req.clone();
            req_a
                .headers
                .insert("Authorization".to_string(), "Bearer A".to_string());
            let r1 = fetcher.fetch(&req_a).await.unwrap();
            assert_eq!(r1.source, FetchSource::Network);

            let mut req_b = base_req.clone();
            req_b
                .headers
                .insert("Authorization".to_string(), "Bearer B".to_string());
            let r2 = fetcher.fetch(&req_b).await.unwrap();
            assert_eq!(
                r2.source,
                FetchSource::Cache,
                "Authorization should not change cache key by default"
            );
        }

        // Opt-in: sensitive headers MAY affect caching (and should avoid reusing the prior cache entry).
        std::env::set_var("WEBPIPE_ALLOW_UNSAFE_HEADERS", "true");
        {
            let mut req_a = base_req.clone();
            req_a
                .headers
                .insert("Authorization".to_string(), "Bearer A".to_string());
            let r1 = fetcher.fetch(&req_a).await.unwrap();
            assert_eq!(r1.source, FetchSource::Network);

            let mut req_b = base_req.clone();
            req_b
                .headers
                .insert("Authorization".to_string(), "Bearer B".to_string());
            let r2 = fetcher.fetch(&req_b).await.unwrap();
            assert_eq!(
                r2.source,
                FetchSource::Network,
                "Authorization should change cache key when WEBPIPE_ALLOW_UNSAFE_HEADERS=true"
            );
        }

        std::env::remove_var("WEBPIPE_ALLOW_UNSAFE_HEADERS");
    }

    fn collect_files_recursively(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_files_recursively(&p, out);
            } else {
                out.push(p);
            }
        }
    }

    #[tokio::test]
    async fn cache_meta_does_not_persist_set_cookie() {
        let app = Router::new().route(
            "/",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "text/plain"),
                        (header::SET_COOKIE, "session=secret"),
                    ],
                    "hello",
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let tmp = tempfile::tempdir().unwrap();
        let fetcher = LocalFetcher::new(Some(tmp.path().to_path_buf())).unwrap();

        let req = FetchRequest {
            url: format!("http://{}/", addr),
            timeout_ms: Some(2_000),
            max_bytes: Some(100_000),
            headers: BTreeMap::new(),
            cache: FetchCachePolicy {
                read: true,
                write: true,
                ttl_s: Some(60),
            },
        };

        let r1 = fetcher.fetch(&req).await.unwrap();
        assert_eq!(r1.source, FetchSource::Network);

        let mut files = Vec::new();
        collect_files_recursively(tmp.path(), &mut files);
        let metas: Vec<PathBuf> = files
            .into_iter()
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
            .collect();
        assert_eq!(metas.len(), 1, "expected one cache meta json file");

        let bytes = std::fs::read(&metas[0]).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let headers = v.get("headers").and_then(|h| h.as_object()).unwrap();
        assert!(
            !headers.contains_key("set-cookie"),
            "cache meta must not persist set-cookie"
        );
        assert!(
            headers.contains_key("content-type"),
            "cache meta should keep content-type"
        );
    }

    #[test]
    fn cache_key_v2_distinguishes_none_from_zero() {
        let base = FetchRequest {
            url: "https://example.com/".to_string(),
            timeout_ms: None,
            max_bytes: None,
            headers: BTreeMap::new(),
            cache: FetchCachePolicy {
                read: true,
                write: true,
                ttl_s: None,
            },
        };
        let mut none = base.clone();
        none.max_bytes = None;
        let mut zero = base.clone();
        zero.max_bytes = Some(0);

        let k2_none = FsCache::key_for_fetch_v2(&none);
        let k2_zero = FsCache::key_for_fetch_v2(&zero);
        assert_ne!(k2_none, k2_zero, "v2 must distinguish None vs Some(0)");

        let k1_none = FsCache::key_for_fetch_legacy_v1(&none);
        let k1_zero = FsCache::key_for_fetch_legacy_v1(&zero);
        assert_eq!(
            k1_none, k1_zero,
            "legacy v1 treated None and Some(0) as identical"
        );
    }

    #[test]
    fn cache_get_reads_legacy_v1_key_and_migrates_to_v2() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = FsCache::new(tmp.path().to_path_buf());

        let req = FetchRequest {
            url: "https://example.com/".to_string(),
            timeout_ms: None,
            max_bytes: None, // legacy collision case
            headers: BTreeMap::new(),
            cache: FetchCachePolicy {
                read: true,
                write: true, // enable migration
                ttl_s: None,
            },
        };

        // Manually write a legacy v1 cache entry.
        let legacy_key = FsCache::key_for_fetch_legacy_v1(&req);
        let (meta_p, body_p) = cache.paths(&legacy_key);
        std::fs::create_dir_all(meta_p.parent().unwrap()).unwrap();
        std::fs::write(&body_p, b"hello").unwrap();
        let meta = serde_json::json!({
            "schema_version": 1,
            "fetched_at_epoch_s": 0,
            "url": req.url,
            "final_url": "https://example.com/".to_string(),
            "status": 200,
            "content_type": "text/plain",
            "headers": { "content-type": "text/plain", "set-cookie": "secret" },
            "truncated": false,
        });
        std::fs::write(&meta_p, serde_json::to_vec(&meta).unwrap()).unwrap();

        let got = cache.get(&req).unwrap().expect("expected legacy cache hit");
        assert_eq!(got.bytes, b"hello");

        // The read should migrate into v2 key space.
        let v2_key = FsCache::key_for_fetch_v2(&req);
        let (meta2_p, body2_p) = cache.paths(&v2_key);
        assert!(meta2_p.exists(), "expected v2 meta to be written");
        assert!(body2_p.exists(), "expected v2 body to be written");
    }

    proptest! {
        #[test]
        fn key_for_fetch_v2_is_hex_and_never_panics(
            url in any::<String>(),
            max_bytes in proptest::option::of(any::<u64>()),
            hdr_pairs in prop::collection::vec((any::<String>(), any::<String>()), 0..20),
        ) {
            let mut headers = BTreeMap::new();
            for (k, v) in hdr_pairs {
                headers.insert(k, v);
            }
            let req = FetchRequest {
                url,
                timeout_ms: None,
                max_bytes,
                headers,
                cache: FetchCachePolicy { read: true, write: true, ttl_s: None },
            };

            let k = FsCache::key_for_fetch_v2(&req);
            prop_assert_eq!(k.len(), 64);
            prop_assert!(k.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));

            // And the path derivation should not panic for keys we generate.
            let cache = FsCache::new(std::env::temp_dir().join("webpipe-proptest-cache"));
            let (_meta, _body) = cache.paths(&k);
        }

        #[test]
        fn cache_meta_headers_never_persists_set_cookie(
            other_pairs in prop::collection::vec((any::<String>(), any::<String>()), 0..20),
            set_cookie_val in any::<String>(),
        ) {
            let mut headers = BTreeMap::new();
            // Random other headers.
            for (k, v) in other_pairs {
                headers.insert(k, v);
            }
            // Adversarial variants of Set-Cookie.
            headers.insert("Set-Cookie".to_string(), set_cookie_val.clone());
            headers.insert(" set-cookie ".to_string(), set_cookie_val.clone());
            headers.insert("SET-COOKIE".to_string(), set_cookie_val);

            let out = FsCache::cache_meta_headers(&headers);
            for k in out.keys() {
                prop_assert_ne!(k.trim().to_ascii_lowercase(), "set-cookie");
            }
            // Allowlist invariant: every stored key must be one of the allowlisted header names.
            for k in out.keys() {
                let kl = k.trim().to_ascii_lowercase();
                prop_assert!(
                    matches!(
                        kl.as_str(),
                        "content-type" | "content-length" | "etag" | "last-modified" | "cache-control"
                    ),
                    "unexpected cached meta header key: {kl}"
                );
            }
        }
    }
}
