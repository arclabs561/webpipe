use serde::Serialize;

pub(crate) fn warning_hint(code: &str) -> Option<&'static str> {
    match code {
        "boilerplate_reduced" => Some(
            "Boilerplate/navigation was reduced. If the remaining text is still noisy, try fetch_backend=\"firecrawl\" (if configured) or pass urls=[...] that point to a specific article/docs page.",
        ),
        "text_truncated_by_max_chars" => Some(
            "Text was truncated by max_chars. Increase max_chars (bounded; e.g. 60_000) or use include_text=false + top_chunks for a smaller, higher-signal evidence pack.",
        ),
        "text_windowed_for_query" => Some(
            "Text was windowed around the best-matching query chunk (to avoid nav-first truncation on long docs pages). If you need the document prefix, increase max_chars or set include_text=true and fetch the page directly.",
        ),
        "cache_doc_reused" => Some(
            "Multiple cache entries mapped to the same URL. The server may deduplicate and keep the best-scoring match to reduce repetition.",
        ),
        "cache_only" => Some(
            "This result was served from cache (no_network=true). To refresh, set no_network=false.",
        ),
        "no_network_may_require_warm_cache" => Some(
            "no_network=true: some URLs could not be fetched from cache. Pre-warm cache with web_fetch (no_network=false, cache_write=true), then retry with no_network=true.",
        ),
        "unknown_seed_id" => Some(
            "Some seed_ids were not recognized. Call web_seed_urls to see valid ids, or pass urls=[...] explicitly.",
        ),
        "empty_extraction" => Some(
            "The response had bytes but extracted text was empty. Consider switching fetch_backend (local vs firecrawl) or increasing max_bytes.",
        ),
        "body_truncated_by_max_bytes" => Some(
            "The response body was truncated by max_bytes. Increase max_bytes, or enable retry_on_truncation=true (and optionally truncation_retry_max_bytes) to recover tail content.",
        ),
        "image_no_text_extraction" => Some(
            "This is an image and no text/OCR backend is available in this environment (tesseract/vision).",
        ),
        "links_unavailable" => Some(
            "Link extraction is unavailable for this backend/content type (e.g. firecrawl or pdf).",
        ),
        "links_timeout" => Some(
            "Link extraction exceeded its bounded timeout and returned no links. Increase WEBPIPE_LINKS_TIMEOUT_MS or disable include_links.",
        ),
        "headers_unavailable" => Some("Headers are unavailable for this backend (e.g. firecrawl)."),
        "text_unavailable_for_pdf" => Some(
            "This looks like a PDF; use web_extract to extract text from PDFs (web_fetch is bytes/text-only).",
        ),
        "semantic_backend_not_configured" => Some(
            "Semantic rerank requested but embeddings backend is not configured. Set an embeddings API key or disable semantic_rerank.",
        ),
        "semantic_rerank_timeout" => Some(
            "Semantic rerank exceeded its bounded timeout and was skipped. If you need it, increase WEBPIPE_SEMANTIC_TIMEOUT_MS or disable semantic_rerank/semantic_auto_fallback.",
        ),
        "blocked_by_js_challenge" => Some(
            "This looks like a JS/CAPTCHA/auth wall. Try fetch_backend=\"firecrawl\" (if configured) or choose a different URL.",
        ),
        "client_side_redirect" => Some(
            "This page appears to be a client-side redirect/interstitial (meta refresh / JS). Use the redirect target URL directly (or increase max_urls) so the tool can fetch the real content.",
        ),
        "silently_throttled" => Some(
            "This looks like a throttling/interstitial page even though the HTTP status was OK. Try a different source URL, reduce request rate, or switch fetch_backend (render/firecrawl).",
        ),
        "http_rate_limited" => Some(
            "HTTP 429 (Too Many Requests): you are being rate-limited. Wait and retry (respect Retry-After when present), reduce request rate (e.g. set WEBPIPE_RATE_LIMIT), and prefer cache-first workflows once you have candidate URLs.",
        ),
        "http_status_error" => Some(
            "HTTP status was >= 400 (likely an error/challenge page). The result is shown for auditability, but its chunks should not be treated as high-quality evidence.",
        ),
        "main_content_low_signal" => Some(
            "Extraction appears dominated by navigation/boilerplate. Try fetch_backend=\"firecrawl\" (if configured) or increase max_chars.",
        ),
        "chunks_filtered_low_signal" => Some(
            "Some extracted chunks looked like JS/app-shell gunk and were filtered. If you need raw output, try include_text=true or fetch_backend=\"firecrawl\".",
        ),
        "structure_html_skipped_long_token" => Some(
            "HTML structure parsing was skipped because the page contained an extremely long unbroken token (often minified JS/base64 blobs). Structure was derived from extracted text instead.",
        ),
        "all_chunks_low_signal" => Some(
            "All extracted chunks appear low-signal (likely app-shell/JS bundle/auth wall). Try fetch_backend=\"firecrawl\" (if configured), choose a different URL, or pass urls=[...] that point to a specific article/docs page.",
        ),
        "extract_input_truncated" => Some(
            "The fetched body was large; extraction only used the first WEBPIPE_EXTRACT_MAX_BYTES bytes. To change this, lower max_bytes or increase WEBPIPE_EXTRACT_MAX_BYTES (server env).",
        ),
        "extract_pipeline_timeout" => Some(
            "Extraction exceeded its bounded pipeline timeout and returned a minimal empty result. Try reducing max_bytes/max_chars, switching fetch_backend, or increasing WEBPIPE_EXTRACT_PIPELINE_TIMEOUT_MS.",
        ),
        "truncation_retry_used" => Some(
            "The initial fetch was truncated by max_bytes, so we retried once with a larger max_bytes (bounded) to recover tail content.",
        ),
        "retried_due_to_truncation" => Some(
            "The initial fetch was truncated by max_bytes, so we retried once with a larger max_bytes (bounded) to recover tail content.",
        ),
        "truncation_retry_failed" => Some(
            "The initial fetch was truncated by max_bytes, and the bounded truncation retry failed. Consider increasing max_bytes or trying a different URL.",
        ),
        "firecrawl_fallback_on_low_signal" => Some(
            "Local extraction looked like low-signal app-shell/JS gunk, so we retried this URL via Firecrawl (bounded).",
        ),
        "render_fallback_on_empty_extraction" => Some(
            "Local extraction was empty, so we retried this URL via Playwright render (bounded). If this keeps happening, consider using fetch_backend=\"render\" directly for this workflow.",
        ),
        "render_fallback_on_low_signal" => Some(
            "Local extraction looked low-signal (likely JS/app-shell), so we retried this URL via Playwright render (bounded). If the page is highly dynamic, try increasing timeout_ms.",
        ),
        "render_fallback_disabled" => Some(
            "Render fallback was requested but is disabled (WEBPIPE_RENDER_DISABLE=1). Unset WEBPIPE_RENDER_DISABLE (and install Playwright) to enable render fallback, or disable render_fallback_* flags.",
        ),
        "render_fallback_failed" => Some(
            "Render fallback failed for this URL. Try increasing timeout_ms, or use fetch_backend=\"render\" directly to debug. In anonymous mode, ensure WEBPIPE_ANON_PROXY points to an HTTP proxy (not socks5h://).",
        ),
        "deadline_exceeded_partial" => Some(
            "Hard deadline hit; returned partial results. Increase deadline_ms (or reduce max_urls/timeout_ms) if you need more coverage.",
        ),
        "no_query_overlap_any_url" => Some(
            "No extracted chunks matched the query tokens across the selected URLs. Try different URLs (or deeper links), increase max_chars, or enable render_fallback_on_low_signal / fetch_backend=\"render\" for JS-heavy docs.",
        ),
        "no_query_overlap_doc" => Some(
            "This cached document had no chunk-level overlap with the query, so it was skipped for evidence selection. Refine the query or restrict domains (or pass urls=[...] to search only intended sources).",
        ),
        "no_query_overlap_docs_dropped" => Some(
            "Most cached documents did not match the query and were dropped from results to keep output compact. If you expected matches, refine the query or increase max_docs/max_scan_entries (debug toolset), or warm cache with relevant URLs first.",
        ),
        "cache_search_timeout" => Some(
            "Cache search+extract exceeded its bounded timeout and returned no results. Increase WEBPIPE_CACHE_SEARCH_TIMEOUT_MS (or reduce max_scan_entries/max_docs).",
        ),
        "cache_io_timeout" => Some(
            "Cache filesystem IO exceeded its bounded timeout; cache was bypassed to keep the tool responsive. If this happens often, check filesystem health or increase WEBPIPE_CACHE_IO_TIMEOUT_MS.",
        ),
        "render_fallback_not_configured" => Some(
            "Render fallback could not run (missing configuration). Install Playwright (Node) + browsers, and ensure Node can require('playwright').",
        ),
        "render_fallback_not_supported" => Some(
            "Render fallback is not supported in the current mode/config (e.g. privacy_mode=offline, or anonymous mode with a socks5h:// proxy). For anonymous render, use an HTTP proxy endpoint (Tor users often run Privoxy).",
        ),
        "perplexity_search_mode_off_rejected" => Some(
            "Tried to disable provider-side browsing (search_mode=\"off\"), but the provider rejected it; we retried without search_mode.",
        ),
        "tavily_used" => Some(
            "Tavily search may consume paid credits/quota. If you have a tight Tavily budget, prefer provider=\"brave\" or provider=\"auto\" with small max_results, and avoid auto_mode=\"merge\" unless you need maximum recall.",
        ),
        "unsafe_request_headers_dropped" => Some(
            "Some request headers were dropped by default for safety (Authorization/Cookie/Proxy-Authorization). To allow forwarding them, set WEBPIPE_ALLOW_UNSAFE_HEADERS=true (only for trusted endpoints).",
        ),
        "github_repo_rewritten_to_raw_readme" => Some(
            "This looks like a GitHub repo root page (often low-signal). We rewrote to fetch the repo README from raw.githubusercontent.com. If you need more than the README (e.g. API surface), use repo_ingest (GitHub API, bounded) or pass urls=[...] that point to specific docs/code pages.",
        ),
        "github_blob_rewritten_to_raw" => Some(
            "This looks like a GitHub file view URL (/blob/...). We rewrote to fetch the raw file from raw.githubusercontent.com.",
        ),
        "github_pr_rewritten_to_patch" => Some(
            "This looks like a GitHub PR page (/pull/...). We rewrote to fetch the .patch artifact for higher-signal text.",
        ),
        "github_commit_rewritten_to_patch" => Some(
            "This looks like a GitHub commit page (/commit/...). We rewrote to fetch the .patch artifact for higher-signal text.",
        ),
        "gist_rewritten_to_raw" => Some(
            "This looks like a GitHub Gist page. We rewrote to fetch the raw gist content (…/raw) for higher-signal text.",
        ),
        "github_issue_rewritten_to_api" => Some(
            "This looks like a GitHub issue page. We rewrote to fetch issue JSON from the GitHub API for higher-signal text.",
        ),
        "github_release_rewritten_to_api" => Some(
            "This looks like a GitHub release page. We rewrote to fetch release JSON from the GitHub API for higher-signal text.",
        ),
        "semantic_auto_fallback_used" => Some(
            "Semantic rerank ran automatically because lexical chunk scoring looked ineffective for this query. To avoid embeddings latency/cost, set semantic_auto_fallback=false (or leave semantic_rerank=false).",
        ),
        "pdf_extract_failed" => Some(
            "PDF text extraction failed. The PDF may be scanned/image-only or the environment may lack shellout tools. Try enabling/installing PDF tools (pdftotext/mutool) or use an OCR backend for scanned PDFs.",
        ),
        "pdf_extract_panicked" => Some(
            "The PDF parser crashed on malformed input. webpipe recovered, but the page has no usable extracted text. Try a different PDF URL, enable PDF shellout tools (pdftotext/mutool), or re-warm cache from a non-truncated source.",
        ),
        "pdf_shellout_unavailable" => Some(
            "PDF shellout tools are unavailable here. Install `pdftotext` (poppler) or `mutool` (MuPDF), and set WEBPIPE_PDF_SHELLOUT=auto (or a specific tool) to enable higher-robustness PDF extraction.",
        ),
        "pdf_strings_fallback_used" => Some(
            "PDF text extraction failed, so webpipe used a low-fidelity fallback that scans raw PDF bytes for ASCII strings. Expect lower quality evidence; for best results, install `pdftotext`/`mutool` or use a different PDF source/URL.",
        ),
        "arxiv_abs_rewritten_to_pdf" => Some(
            "This looks like an arXiv abstract page (/abs/...). We rewrote to fetch the corresponding PDF (/pdf/...pdf) for better full-text extraction.",
        ),
        "arxiv_pdf_fallback_to_html" => Some(
            "PDF extraction for this arXiv paper was degraded, so webpipe fell back to ar5iv HTML (arXiv Labs) to extract higher-signal text evidence.",
        ),
        "openreview_pdf_fallback_to_forum" => Some(
            "PDF extraction for this OpenReview paper was degraded, so webpipe fell back to the OpenReview forum page (/forum?id=...) to extract higher-signal metadata (title/abstract) as evidence.",
        ),
        "openreview_pdf_fallback_to_api" => Some(
            "PDF extraction for this OpenReview paper was degraded, so webpipe fell back to the OpenReview notes API (api.openreview.net) to extract higher-signal metadata (title/abstract) as evidence.",
        ),
        "paper_backend_failed" => Some(
            "All paper search backends failed (Semantic Scholar / OpenAlex are free-tier and may rate-limit). Try again after a brief wait, narrow the query, or use arxiv_search instead (uses arXiv Atom API, more stable). For Google Scholar coverage, set WEBPIPE_SERPAPI_API_KEY.",
        ),
        _ => None,
    }
}

pub(crate) fn warning_hints_from(codes: &[&str]) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    for c in codes {
        if let Some(h) = warning_hint(c) {
            m.insert((*c).to_string(), serde_json::json!(h));
        }
    }
    serde_json::Value::Object(m)
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ErrorCode {
    InvalidParams,
    InvalidUrl,
    NotConfigured,
    NotSupported,
    ProviderUnavailable,
    FetchFailed,
    SearchFailed,
    CacheError,
    UnexpectedError,
}

impl ErrorCode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InvalidParams => "invalid_params",
            Self::InvalidUrl => "invalid_url",
            Self::NotConfigured => "not_configured",
            Self::NotSupported => "not_supported",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::FetchFailed => "fetch_failed",
            Self::SearchFailed => "search_failed",
            Self::CacheError => "cache_error",
            Self::UnexpectedError => "unexpected_error",
        }
    }

    pub(crate) fn retryable(self) -> bool {
        match self {
            Self::FetchFailed | Self::CacheError | Self::SearchFailed => true,
            // Configuration + invalid input are not retryable without changing something.
            Self::NotConfigured
            | Self::NotSupported
            | Self::InvalidParams
            | Self::InvalidUrl
            | Self::ProviderUnavailable
            | Self::UnexpectedError => false,
        }
    }
}

/// Retain only the minimal signal set needed by agent loops, stripping verbose fields
/// (per-URL results[], request echo, search.steps, backend_provider, agentic trace, etc.).
///
/// Keeps: ok, top_chunks, warning_codes, warning_hints, schema_version, kind, elapsed_ms,
/// error (when ok=false). All other fields are removed.
///
/// Call after add_envelope_fields when minimal_output=true.
pub(crate) fn strip_minimal_output(payload: &mut serde_json::Value) {
    const KEEP: &[&str] = &[
        "ok",
        "top_chunks",
        "warning_codes",
        "warning_hints",
        "schema_version",
        "kind",
        "elapsed_ms",
        "error",
    ];
    if let Some(obj) = payload.as_object_mut() {
        obj.retain(|k, _| KEEP.contains(&k.as_str()));
    }
}

pub(crate) fn add_envelope_fields(payload: &mut serde_json::Value, kind: &str, elapsed_ms: u128) {
    payload["schema_version"] = serde_json::json!(super::SCHEMA_VERSION);
    payload["kind"] = serde_json::json!(kind);
    payload["elapsed_ms"] = serde_json::json!(elapsed_ms);
    // API-surface refinement: keep a small set of ubiquitous envelope keys stable.
    // - attempts: null or object (helps clients avoid "missing vs present" branching)
    // - request: null or object (many tools fill this in; for others we keep it explicit)
    if payload.get("attempts").is_none() {
        payload["attempts"] = serde_json::Value::Null;
    }
    if payload.get("request").is_none() {
        payload["request"] = serde_json::Value::Null;
    }
}

pub(crate) fn error_obj(
    code: ErrorCode,
    message: impl ToString,
    hint: impl ToString,
) -> serde_json::Value {
    #[derive(Serialize)]
    struct ErrorObject {
        code: &'static str,
        message: String,
        hint: String,
        retryable: bool,
    }

    let e = ErrorObject {
        code: code.as_str(),
        message: message.to_string(),
        hint: hint.to_string(),
        retryable: code.retryable(),
    };
    match serde_json::to_value(e) {
        Ok(v) => v,
        Err(_) => serde_json::json!({
            "code": code.as_str(),
            "message": message.to_string(),
            "hint": hint.to_string(),
            "retryable": code.retryable()
        }),
    }
}
