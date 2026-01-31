use serde::Serialize;

pub(crate) fn warning_hint(code: &'static str) -> Option<&'static str> {
    match code {
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
        "image_no_text_extraction" => Some(
            "This is an image and no text/OCR backend is available in this environment (tesseract/vision).",
        ),
        "links_unavailable" => Some(
            "Link extraction is unavailable for this backend/content type (e.g. firecrawl or pdf).",
        ),
        "headers_unavailable" => Some("Headers are unavailable for this backend (e.g. firecrawl)."),
        "text_unavailable_for_pdf" => Some(
            "This looks like a PDF; use web_extract to extract text from PDFs (web_fetch is bytes/text-only).",
        ),
        "semantic_backend_not_configured" => Some(
            "Semantic rerank requested but embeddings backend is not configured. Set an embeddings API key or disable semantic_rerank.",
        ),
        "blocked_by_js_challenge" => Some(
            "This looks like a JS/CAPTCHA/auth wall. Try fetch_backend=\"firecrawl\" (if configured) or choose a different URL.",
        ),
        "main_content_low_signal" => Some(
            "Extraction appears dominated by navigation/boilerplate. Try fetch_backend=\"firecrawl\" (if configured) or increase max_chars.",
        ),
        "chunks_filtered_low_signal" => Some(
            "Some extracted chunks looked like JS/app-shell gunk and were filtered. If you need raw output, try include_text=true or fetch_backend=\"firecrawl\".",
        ),
        "all_chunks_low_signal" => Some(
            "All extracted chunks appear low-signal (likely app-shell/JS bundle/auth wall). Try fetch_backend=\"firecrawl\" (if configured), choose a different URL, or pass urls=[...] that point to a specific article/docs page.",
        ),
        "extract_input_truncated" => Some(
            "The fetched body was large; extraction only used the first WEBPIPE_EXTRACT_MAX_BYTES bytes. To change this, lower max_bytes or increase WEBPIPE_EXTRACT_MAX_BYTES (server env).",
        ),
        "truncation_retry_used" => Some(
            "The initial fetch was truncated by max_bytes, so we retried once with a larger max_bytes (bounded) to recover tail content.",
        ),
        "truncation_retry_failed" => Some(
            "The initial fetch was truncated by max_bytes, and the bounded truncation retry failed. Consider increasing max_bytes or trying a different URL.",
        ),
        "firecrawl_fallback_on_low_signal" => Some(
            "Local extraction looked like low-signal app-shell/JS gunk, so we retried this URL via Firecrawl (bounded).",
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
            "This looks like a GitHub repo root page (often low-signal). We rewrote to fetch the repo README from raw.githubusercontent.com. If you need richer docs, pass a docs URL (or set urls=[...] to specific pages).",
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
        "pdf_shellout_unavailable" => Some(
            "PDF shellout tools are unavailable here. Install `pdftotext` (poppler) or `mutool` (MuPDF), and set WEBPIPE_PDF_SHELLOUT=auto (or a specific tool) to enable higher-robustness PDF extraction.",
        ),
        "arxiv_abs_rewritten_to_pdf" => Some(
            "This looks like an arXiv abstract page (/abs/...). We rewrote to fetch the corresponding PDF (/pdf/...pdf) for better full-text extraction.",
        ),
        _ => None,
    }
}

pub(crate) fn warning_hints_from(codes: &[&'static str]) -> serde_json::Value {
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
