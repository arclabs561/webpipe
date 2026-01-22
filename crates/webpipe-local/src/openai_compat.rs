use serde::{Deserialize, Serialize};
use webpipe_core::{Error, Result};

fn env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn openai_compat_base_url_from_env() -> Option<String> {
    env("WEBPIPE_OPENAI_COMPAT_BASE_URL")
}

fn openai_compat_api_key_from_env() -> Option<String> {
    env("WEBPIPE_OPENAI_COMPAT_API_KEY")
}

fn openai_compat_model_from_env() -> Option<String> {
    env("WEBPIPE_OPENAI_COMPAT_MODEL")
}

#[derive(Debug, Clone)]
pub struct OpenAiCompatClient {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
}

impl OpenAiCompatClient {
    pub fn from_env(client: reqwest::Client, model_override: Option<String>) -> Result<Self> {
        let base_url = openai_compat_base_url_from_env().ok_or_else(|| {
            Error::NotConfigured("missing WEBPIPE_OPENAI_COMPAT_BASE_URL".to_string())
        })?;
        let api_key = openai_compat_api_key_from_env();

        let model = model_override
            .or_else(openai_compat_model_from_env)
            .ok_or_else(|| {
                Error::NotConfigured(
                    "missing model for openai_compat (set llm_model or WEBPIPE_OPENAI_COMPAT_MODEL)"
                        .to_string(),
                )
            })?;

        Ok(Self {
            client,
            base_url,
            api_key,
            model,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn endpoint_chat_completions(&self) -> String {
        format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        )
    }

    pub async fn chat(
        &self,
        system: &str,
        user: &str,
        timeout_ms: u64,
        max_tokens: Option<u64>,
        temperature: Option<f64>,
        top_p: Option<f64>,
    ) -> Result<String> {
        let req = ChatCompletionsRequest {
            model: self.model.clone(),
            messages: vec![
                Message {
                    role: "system".to_string(),
                    content: system.to_string(),
                },
                Message {
                    role: "user".to_string(),
                    content: user.to_string(),
                },
            ],
            max_tokens,
            temperature,
            top_p,
            stream: Some(false),
        };

        let mut rb = self
            .client
            .post(self.endpoint_chat_completions())
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .header(reqwest::header::CONTENT_TYPE, "application/json");
        if let Some(k) = &self.api_key {
            rb = rb.header(reqwest::header::AUTHORIZATION, format!("Bearer {k}"));
        }

        let resp = rb
            .json(&req)
            .send()
            .await
            .map_err(|e| Error::Llm(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Llm(format!(
                "openai_compat chat.completions HTTP {status}"
            )));
        }

        let parsed: ChatCompletionsResponse =
            resp.json().await.map_err(|e| Error::Llm(e.to_string()))?;
        Ok(parsed
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default())
    }
}

#[derive(Debug, Clone, Serialize)]
struct ChatCompletionsRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ChatCompletionsResponse {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Debug, Clone, Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Debug, Clone, Deserialize)]
struct ChoiceMessage {
    content: String,
}
