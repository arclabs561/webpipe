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

