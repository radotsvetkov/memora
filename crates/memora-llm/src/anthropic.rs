use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::client::{
    shared_http_client, CompletionRequest, CompletionResponse, LlmClient, LlmDestination, LlmError,
};

const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
const API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const JSON_ONLY_INSTRUCTION: &str = "Respond with valid JSON only. No prose, no markdown fences.";

/// Anthropic API-backed LLM client.
pub struct AnthropicClient {
    pub(crate) http: reqwest::Client,
    pub(crate) api_key: SecretString,
    pub(crate) model: String,
}

impl AnthropicClient {
    /// Build a new Anthropic client from environment.
    pub fn new(model: Option<String>) -> Result<Self, LlmError> {
        let api_key =
            std::env::var(API_KEY_ENV).map_err(|_| LlmError::MissingApiKey(API_KEY_ENV))?;

        Ok(Self {
            http: shared_http_client(),
            api_key: SecretString::new(api_key),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        })
    }

    fn endpoint() -> String {
        let base =
            std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        format!("{base}/v1/messages")
    }

    fn effective_system(req: &CompletionRequest) -> Option<String> {
        if req.json_mode {
            let current = req.system.clone().unwrap_or_default();
            if current.is_empty() {
                Some(JSON_ONLY_INSTRUCTION.to_string())
            } else {
                Some(format!("{current}\n{JSON_ONLY_INSTRUCTION}"))
            }
        } else {
            req.system.clone()
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for AnthropicClient {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let model = req.model.clone().unwrap_or_else(|| self.model.clone());
        let body = AnthropicRequest {
            model: model.clone(),
            max_tokens: req.max_tokens,
            temperature: req.temperature,
            system: Self::effective_system(&req),
            messages: req
                .messages
                .into_iter()
                .map(|m| AnthropicMessage {
                    role: m.role,
                    content: m.content,
                })
                .collect(),
        };

        debug!(model = %model, "sending anthropic completion request");

        let response = self
            .http
            .post(Self::endpoint())
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let raw = response.text().await?;
        if status.as_u16() == 429 {
            return Err(LlmError::RateLimited);
        }
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(LlmError::Auth(raw));
        }
        if status.is_server_error() {
            return Err(LlmError::ServerError(status.as_u16(), raw));
        }
        if !status.is_success() {
            return Err(LlmError::Auth(raw));
        }

        let parsed: AnthropicResponse = serde_json::from_str(&raw)?;
        let text = parsed
            .content
            .first()
            .map(|block| block.text.clone())
            .unwrap_or_default();

        Ok(CompletionResponse {
            text,
            model: parsed.model.unwrap_or(model),
            input_tokens: parsed.usage.as_ref().map(|u| u.input_tokens),
            output_tokens: parsed.usage.as_ref().map(|u| u.output_tokens),
        })
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn destination(&self) -> LlmDestination {
        LlmDestination::CloudKnown
    }
}

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    temperature: f32,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: crate::client::Role,
    content: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    content: Vec<AnthropicContentBlock>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::await_holding_lock)]

    use std::sync::{Mutex, OnceLock};

    use mockito::{Matcher, Server};

    use crate::client::{CompletionRequest, LlmError, Message, Role};

    use super::AnthropicClient;
    use crate::LlmClient;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn make_request(json_mode: bool, system: Option<&str>) -> CompletionRequest {
        CompletionRequest {
            system: system.map(|s| s.to_string()),
            messages: vec![Message {
                role: Role::User,
                content: "hello".to_string(),
            }],
            json_mode,
            ..CompletionRequest::default()
        }
    }

    #[tokio::test]
    async fn complete_happy_path_parses_response() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut server = Server::new_async().await;
        std::env::set_var("ANTHROPIC_BASE_URL", server.url());
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        let _mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                  "model":"claude-sonnet-4-6",
                  "content":[{"type":"text","text":"ok"}],
                  "usage":{"input_tokens":11,"output_tokens":7}
                }"#,
            )
            .create();

        let client = AnthropicClient::new(None).expect("client should construct");
        let out = client
            .complete(make_request(false, Some("you are concise")))
            .await
            .expect("request should succeed");

        assert_eq!(out.text, "ok");
        assert_eq!(out.model, "claude-sonnet-4-6");
        assert_eq!(out.input_tokens, Some(11));
        assert_eq!(out.output_tokens, Some(7));
    }

    #[tokio::test]
    async fn complete_maps_429_to_rate_limited() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut server = Server::new_async().await;
        std::env::set_var("ANTHROPIC_BASE_URL", server.url());
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        let _mock = server
            .mock("POST", "/v1/messages")
            .with_status(429)
            .create();

        let client = AnthropicClient::new(None).expect("client should construct");
        let err = client
            .complete(make_request(false, None))
            .await
            .expect_err("request should fail");
        assert!(matches!(err, LlmError::RateLimited));
    }

    #[tokio::test]
    async fn complete_maps_500_to_server_error() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut server = Server::new_async().await;
        std::env::set_var("ANTHROPIC_BASE_URL", server.url());
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        let _mock = server
            .mock("POST", "/v1/messages")
            .with_status(500)
            .with_body("upstream exploded")
            .create();

        let client = AnthropicClient::new(None).expect("client should construct");
        let err = client
            .complete(make_request(false, None))
            .await
            .expect_err("request should fail");
        assert!(matches!(err, LlmError::ServerError(500, body) if body == "upstream exploded"));
    }

    #[tokio::test]
    async fn json_mode_appends_json_instruction_to_system_message() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut server = Server::new_async().await;
        std::env::set_var("ANTHROPIC_BASE_URL", server.url());
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");

        let _mock = server
            .mock("POST", "/v1/messages")
            .match_body(Matcher::Regex(
                r#"(?s)"system":"be strict\\nRespond with valid JSON only\. No prose, no markdown fences\.""#
                    .to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"model":"claude-sonnet-4-6","content":[{"type":"text","text":"{\"ok\":true}"}]}"#,
            )
            .create();

        let client = AnthropicClient::new(None).expect("client should construct");
        let _ = client
            .complete(make_request(true, Some("be strict")))
            .await
            .expect("request should succeed");
    }

    #[test]
    fn new_without_api_key_returns_missing_api_key() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("ANTHROPIC_API_KEY");

        let out = AnthropicClient::new(None);
        assert!(matches!(
            out,
            Err(LlmError::MissingApiKey("ANTHROPIC_API_KEY"))
        ));
    }
}
