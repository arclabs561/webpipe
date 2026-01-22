//! Gemini vision (image -> text) backend.
//!
//! This is intentionally small and bounded. It’s meant as a robustness fallback when:
//! - we fetched an image
//! - local OCR is unavailable or failed
//! - the user explicitly enables vision via env
//!
//! Notes:
//! - This runs over HTTP (reqwest) and is async. Wire it from async callers (MCP tool handlers).
//! - Do not call this from synchronous extraction helpers.

use base64::Engine;
use serde::Serialize;

fn env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn gemini_enabled_mode_from_env() -> String {
    match env("WEBPIPE_VISION").as_deref() {
        Some("off") => "off".to_string(),
        Some("strict") => "strict".to_string(),
        Some("auto") => "auto".to_string(),
        None => "off".to_string(), // safety default: no surprise network calls
        Some(_) => "off".to_string(),
    }
}

pub fn gemini_api_key_from_env() -> Option<String> {
    env("WEBPIPE_GEMINI_API_KEY")
        .or_else(|| env("GEMINI_API_KEY"))
        .or_else(|| env("WEBPIPE_GOOGLE_API_KEY"))
        .or_else(|| env("GOOGLE_API_KEY"))
}

pub fn gemini_model_from_env() -> String {
    env("WEBPIPE_GEMINI_MODEL").unwrap_or_else(|| "gemini-2.0-flash".to_string())
}

pub fn gemini_timeout_ms_from_env() -> u64 {
    env("WEBPIPE_GEMINI_TIMEOUT_MS")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(20_000)
        .clamp(200, 120_000)
}

pub fn gemini_max_chars_from_env() -> usize {
    env("WEBPIPE_GEMINI_MAX_CHARS")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(50_000)
        .clamp(200, 200_000)
}

#[derive(Debug, Serialize)]
struct ReqPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inline_data: Option<InlineData>,
}

#[derive(Debug, Serialize)]
struct InlineData {
    mime_type: String,
    data: String,
}

#[derive(Debug, Serialize)]
struct ReqContent {
    parts: Vec<ReqPart>,
}

#[derive(Debug, Serialize)]
struct GenCfg {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
}

#[derive(Debug, Serialize)]
struct GeminiReq {
    contents: Vec<ReqContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenCfg>,
}

pub async fn gemini_image_to_text(
    client: reqwest::Client,
    bytes: &[u8],
    mime_type: &str,
) -> Result<(String, String), &'static str> {
    let key = gemini_api_key_from_env().ok_or("gemini_not_configured")?;
    let model = gemini_model_from_env();
    let timeout_ms = gemini_timeout_ms_from_env();
    let max_chars = gemini_max_chars_from_env();

    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    let prompt = env("WEBPIPE_GEMINI_PROMPT").unwrap_or_else(|| {
        "Extract the readable text from this image. Return only the text.".to_string()
    });

    let req = GeminiReq {
        contents: vec![ReqContent {
            parts: vec![
                ReqPart {
                    text: Some(prompt),
                    inline_data: None,
                },
                ReqPart {
                    text: None,
                    inline_data: Some(InlineData {
                        mime_type: mime_type.to_string(),
                        data,
                    }),
                },
            ],
        }],
        generation_config: Some(GenCfg {
            temperature: Some(0.0),
            max_output_tokens: Some(2048),
        }),
    };

    // Endpoint: Generative Language API (v1beta). Keep it simple and key-in-query like most samples.
    // Users who want proxying can set WEBPIPE_GEMINI_BASE_URL.
    let base = env("WEBPIPE_GEMINI_BASE_URL")
        .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string());
    let url = format!(
        "{base}/v1beta/models/{model}:generateContent?key={key}",
        base = base.trim_end_matches('/')
    );

    let resp = client
        .post(url)
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .json(&req)
        .send()
        .await
        .map_err(|_| "gemini_request_failed")?;

    if !resp.status().is_success() {
        return Err("gemini_non_success_status");
    }

    let v: serde_json::Value = resp.json().await.map_err(|_| "gemini_bad_json")?;
    // candidates[0].content.parts[*].text
    let mut out = String::new();
    if let Some(cands) = v.get("candidates").and_then(|x| x.as_array()) {
        if let Some(c0) = cands.first() {
            if let Some(parts) = c0
                .get("content")
                .and_then(|x| x.get("parts"))
                .and_then(|x| x.as_array())
            {
                for p in parts {
                    if let Some(t) = p.get("text").and_then(|x| x.as_str()) {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(t);
                    }
                }
            }
        }
    }
    let clipped: String = out.chars().take(max_chars).collect();
    if clipped.chars().any(|c| !c.is_whitespace()) {
        Ok((model, clipped))
    } else {
        Err("gemini_empty_output")
    }
}
