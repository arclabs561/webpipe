use serde::{Deserialize, Serialize};
use std::time::Instant;
use webpipe_core::{Error, Result};

fn perplexity_api_key_from_env() -> Option<String> {
    std::env::var("WEBPIPE_PERPLEXITY_API_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("PERPLEXITY_API_KEY")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
}

#[derive(Debug, Clone)]
pub struct PerplexityClient {
    client: reqwest::Client,
    api_key: String,
}

impl PerplexityClient {
    pub fn from_env(client: reqwest::Client) -> Result<Self> {
        let api_key = perplexity_api_key_from_env().ok_or_else(|| {
            Error::NotConfigured(
                "missing WEBPIPE_PERPLEXITY_API_KEY (or PERPLEXITY_API_KEY)".to_string(),
            )
        })?;
        Ok(Self { client, api_key })
    }

    fn endpoint_chat_completions() -> String {
        // Docs: https://docs.perplexity.ai/api-reference/chat-completions-post
        //
        // Allow override for testing/debugging (do not include secrets here).
        std::env::var("WEBPIPE_PERPLEXITY_ENDPOINT")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "https://api.perplexity.ai/chat/completions".to_string())
    }

    pub async fn chat_completions(
        &self,
        req: ChatCompletionsRequest,
    ) -> Result<ChatCompletionsResponse> {
        let t0 = Instant::now();
        let resp = self
            .client
            .post(Self::endpoint_chat_completions())
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.api_key),
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&req)
            .send()
            .await
            .map_err(|e| Error::Llm(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Llm(format!(
                "perplexity chat.completions HTTP {status}"
            )));
        }

        let mut parsed: ChatCompletionsResponse =
            resp.json().await.map_err(|e| Error::Llm(e.to_string()))?;
        parsed.timings_ms = Some(serde_json::json!({ "request": t0.elapsed().as_millis() }));
        Ok(parsed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionsRequest {
    pub model: String,
    pub messages: Vec<Message>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,

    /// Perplexity docs: "search_mode" exists on the endpoint. Use only when needed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_mode: Option<String>,

    /// Perplexity-specific; only applicable for sonar-deep-research.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionsResponse {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub citations: Option<Vec<String>>,
    #[serde(default)]
    pub usage: Option<serde_json::Value>,
    /// Best-effort extra timing info we attach after parsing.
    #[serde(default)]
    pub timings_ms: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    pub index: Option<u64>,
    pub message: ChoiceMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChoiceMessage {
    pub role: String,
    pub content: String,
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
    fn empty_key_is_treated_as_missing() {
        let _g = EnvGuard::set("WEBPIPE_PERPLEXITY_API_KEY", "   ");
        assert!(perplexity_api_key_from_env().is_none());
    }
}
