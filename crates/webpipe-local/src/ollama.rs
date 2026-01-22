use serde::{Deserialize, Serialize};
use webpipe_core::{Error, Result};

fn env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn env_bool(key: &str) -> bool {
    env(key)
        .and_then(|s| s.parse::<bool>().ok())
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct OllamaClient {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

impl OllamaClient {
    pub fn from_env(client: reqwest::Client) -> Result<Self> {
        // Opt-in: don't accidentally start calling localhost if the user didn't ask for it.
        let enabled = env_bool("WEBPIPE_OLLAMA_ENABLE");
        if !enabled {
            return Err(Error::NotConfigured(
                "WEBPIPE_OLLAMA_ENABLE is not set (or false)".to_string(),
            ));
        }
        let base_url =
            env("WEBPIPE_OLLAMA_BASE_URL").unwrap_or_else(|| "http://127.0.0.1:11434".to_string());
        // A pragmatic default for “small but capable” local reasoning/coding.
        // Users should override this based on what they have installed.
        let model =
            env("WEBPIPE_OLLAMA_MODEL").unwrap_or_else(|| "qwen2.5:3b-instruct".to_string());
        Ok(Self {
            client,
            base_url,
            model,
        })
    }

    fn endpoint_chat(&self) -> String {
        format!("{}/api/chat", self.base_url.trim_end_matches('/'))
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn chat(&self, system: &str, user: &str, timeout_ms: u64) -> Result<String> {
        let req = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system.to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: user.to_string(),
                },
            ],
            stream: Some(false),
        };

        let resp = self
            .client
            .post(self.endpoint_chat())
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&req)
            .send()
            .await
            .map_err(|e| Error::Llm(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Llm(format!("ollama chat HTTP {status}")));
        }

        let parsed: ChatResponse = resp.json().await.map_err(|e| Error::Llm(e.to_string()))?;
        Ok(parsed.message.content)
    }
}

#[derive(Debug, Clone, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ChatResponse {
    message: ChatMessage,
}
