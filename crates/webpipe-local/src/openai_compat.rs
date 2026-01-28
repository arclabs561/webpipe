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

#[derive(Debug, Clone, Default)]
pub struct ChatOptions {
    /// If set, request that the provider include (or omit) a reasoning trace, if supported.
    pub include_reasoning: Option<bool>,
    /// If set, request a reasoning effort level, if supported (e.g. minimal|low|medium|high).
    pub reasoning_effort: Option<String>,
}

impl OpenAiCompatClient {
    pub fn new(
        client: reqwest::Client,
        base_url: String,
        api_key: Option<String>,
        model: String,
    ) -> Result<Self> {
        if base_url.trim().is_empty() {
            return Err(Error::NotConfigured(
                "missing openai_compat base_url".to_string(),
            ));
        }
        if model.trim().is_empty() {
            return Err(Error::NotConfigured(
                "missing model for openai_compat".to_string(),
            ));
        }
        Ok(Self {
            client,
            base_url,
            api_key,
            model,
        })
    }

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

        Self::new(client, base_url, api_key, model)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    fn endpoint_chat_completions(&self) -> String {
        format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        )
    }

    fn endpoint_embeddings(&self) -> String {
        format!("{}/v1/embeddings", self.base_url.trim_end_matches('/'))
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
        self.chat_with_options(
            system,
            user,
            timeout_ms,
            max_tokens,
            temperature,
            top_p,
            ChatOptions::default(),
        )
        .await
    }

    pub async fn chat_json(
        &self,
        system: &str,
        user: &str,
        timeout_ms: u64,
        max_tokens: Option<u64>,
        temperature: Option<f64>,
        top_p: Option<f64>,
    ) -> Result<String> {
        self.chat_json_with_options(
            system,
            user,
            timeout_ms,
            max_tokens,
            temperature,
            top_p,
            ChatOptions::default(),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn chat_messages_with_options(
        &self,
        messages: Vec<ChatMessage>,
        timeout_ms: u64,
        max_tokens: Option<u64>,
        temperature: Option<f64>,
        top_p: Option<f64>,
        json_mode: bool,
        options: ChatOptions,
    ) -> Result<String> {
        let req = ChatCompletionsRequest {
            model: self.model.clone(),
            messages,
            max_tokens,
            temperature,
            top_p,
            stream: Some(false),
            response_format: if json_mode {
                Some(ResponseFormat {
                    kind: "json_object".to_string(),
                })
            } else {
                None
            },
            include_reasoning: options.include_reasoning,
            reasoning: options
                .reasoning_effort
                .as_ref()
                .map(|effort| ReasoningConfig {
                    effort: effort.trim().to_string(),
                })
                .filter(|rc| !rc.effort.is_empty()),
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

    #[allow(clippy::too_many_arguments)]
    pub async fn chat_with_options(
        &self,
        system: &str,
        user: &str,
        timeout_ms: u64,
        max_tokens: Option<u64>,
        temperature: Option<f64>,
        top_p: Option<f64>,
        options: ChatOptions,
    ) -> Result<String> {
        self.chat_messages_with_options(
            vec![ChatMessage::system(system), ChatMessage::user(user)],
            timeout_ms,
            max_tokens,
            temperature,
            top_p,
            false,
            options,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn chat_json_with_options(
        &self,
        system: &str,
        user: &str,
        timeout_ms: u64,
        max_tokens: Option<u64>,
        temperature: Option<f64>,
        top_p: Option<f64>,
        options: ChatOptions,
    ) -> Result<String> {
        self.chat_messages_with_options(
            vec![ChatMessage::system(system), ChatMessage::user(user)],
            timeout_ms,
            max_tokens,
            temperature,
            top_p,
            true,
            options,
        )
        .await
    }

    pub async fn embeddings(&self, input: Vec<String>, timeout_ms: u64) -> Result<Vec<Vec<f32>>> {
        if input.is_empty() {
            return Ok(Vec::new());
        }

        let req = EmbeddingsRequest {
            model: self.model.clone(),
            input,
        };

        let mut rb = self
            .client
            .post(self.endpoint_embeddings())
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
                "openai_compat embeddings HTTP {status}"
            )));
        }

        let parsed: EmbeddingsResponse =
            resp.json().await.map_err(|e| Error::Llm(e.to_string()))?;
        let mut out: Vec<Vec<f32>> = Vec::new();
        for d in parsed.data {
            out.push(d.embedding);
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Serialize)]
struct ChatCompletionsRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    include_reasoning: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningConfig>,
}

#[derive(Debug, Clone, Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Clone, Serialize)]
struct ReasoningConfig {
    effort: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: &str) -> Self {
        Self {
            role: "system".to_string(),
            content: content.to_string(),
        }
    }

    pub fn user(content: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: content.to_string(),
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.to_string(),
        }
    }
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

#[derive(Debug, Clone, Serialize)]
struct EmbeddingsRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct EmbeddingsResponse {
    #[serde(default)]
    data: Vec<EmbeddingsDatum>,
}

#[derive(Debug, Clone, Deserialize)]
struct EmbeddingsDatum {
    #[serde(default)]
    embedding: Vec<f32>,
}
