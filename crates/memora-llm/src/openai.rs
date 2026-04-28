use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::client::{
    shared_http_client, CompletionRequest, CompletionResponse, LlmClient, LlmDestination, LlmError,
};

const DEFAULT_MODEL: &str = "gpt-4o-mini";
const API_KEY_ENV: &str = "OPENAI_API_KEY";
const DEFAULT_BASE_URL: &str = "https://api.openai.com";
const JSON_ONLY_INSTRUCTION: &str = "Respond in JSON only.";

/// OpenAI API-backed LLM client.
pub struct OpenAiClient {
    pub(crate) http: reqwest::Client,
    pub(crate) api_key: SecretString,
    pub(crate) model: String,
}

impl OpenAiClient {
    /// Build a new OpenAI client from environment.
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
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        format!("{base}/v1/chat/completions")
    }

    fn with_system(req: &CompletionRequest) -> Vec<OpenAiMessage> {
        let mut out = Vec::with_capacity(req.messages.len() + 1);
        if let Some(system) = req.system.clone() {
            out.push(OpenAiMessage {
                role: crate::client::Role::System,
                content: system,
            });
        }
        out.extend(req.messages.iter().cloned().map(|m| OpenAiMessage {
            role: m.role,
            content: m.content,
        }));
        out
    }

    fn ensure_json_system(messages: &mut Vec<OpenAiMessage>) {
        if let Some(system_msg) = messages
            .iter_mut()
            .find(|message| matches!(message.role, crate::client::Role::System))
        {
            system_msg.content = format!("{}\n{JSON_ONLY_INSTRUCTION}", system_msg.content);
        } else {
            messages.insert(
                0,
                OpenAiMessage {
                    role: crate::client::Role::System,
                    content: JSON_ONLY_INSTRUCTION.to_string(),
                },
            );
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for OpenAiClient {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let model = req.model.clone().unwrap_or_else(|| self.model.clone());
        let mut messages = Self::with_system(&req);
        if req.json_mode {
            Self::ensure_json_system(&mut messages);
        }

        let body = OpenAiRequest {
            model: model.clone(),
            messages,
            max_tokens: req.max_tokens,
            temperature: req.temperature,
            response_format: req.json_mode.then_some(OpenAiResponseFormat {
                format_type: "json_object".to_string(),
            }),
        };

        debug!(model = %model, "sending openai completion request");

        let response = self
            .http
            .post(Self::endpoint())
            .bearer_auth(self.api_key.expose_secret())
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

        let parsed: OpenAiResponse = serde_json::from_str(&raw)?;
        let text = parsed
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        Ok(CompletionResponse {
            text,
            model: parsed.model.unwrap_or(model),
            input_tokens: parsed.usage.as_ref().map(|u| u.prompt_tokens),
            output_tokens: parsed.usage.as_ref().map(|u| u.completion_tokens),
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
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    max_tokens: u32,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<OpenAiResponseFormat>,
}

#[derive(Debug, Serialize)]
struct OpenAiMessage {
    role: crate::client::Role,
    content: String,
}

#[derive(Debug, Serialize)]
struct OpenAiResponseFormat {
    #[serde(rename = "type")]
    format_type: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessageResponse,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessageResponse {
    #[serde(default)]
    content: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::await_holding_lock)]

    use std::sync::{Mutex, OnceLock};

    use mockito::{Matcher, Server};

    use crate::client::{CompletionRequest, LlmError, Message, Role};

    use super::OpenAiClient;
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
        std::env::set_var("OPENAI_BASE_URL", server.url());
        std::env::set_var("OPENAI_API_KEY", "test-key");

        let _mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                  "model":"gpt-4o-mini",
                  "choices":[{"message":{"role":"assistant","content":"ok"}}],
                  "usage":{"prompt_tokens":13,"completion_tokens":5}
                }"#,
            )
            .create();

        let client = OpenAiClient::new(None).expect("client should construct");
        let out = client
            .complete(make_request(false, Some("you are concise")))
            .await
            .expect("request should succeed");

        assert_eq!(out.text, "ok");
        assert_eq!(out.model, "gpt-4o-mini");
        assert_eq!(out.input_tokens, Some(13));
        assert_eq!(out.output_tokens, Some(5));
    }

    #[tokio::test]
    async fn complete_maps_429_to_rate_limited() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut server = Server::new_async().await;
        std::env::set_var("OPENAI_BASE_URL", server.url());
        std::env::set_var("OPENAI_API_KEY", "test-key");

        let _mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(429)
            .create();

        let client = OpenAiClient::new(None).expect("client should construct");
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
        std::env::set_var("OPENAI_BASE_URL", server.url());
        std::env::set_var("OPENAI_API_KEY", "test-key");

        let _mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(500)
            .with_body("openai overloaded")
            .create();

        let client = OpenAiClient::new(None).expect("client should construct");
        let err = client
            .complete(make_request(false, None))
            .await
            .expect_err("request should fail");
        assert!(matches!(err, LlmError::ServerError(500, body) if body == "openai overloaded"));
    }

    #[tokio::test]
    async fn json_mode_sets_response_format_and_updates_system_instruction() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut server = Server::new_async().await;
        std::env::set_var("OPENAI_BASE_URL", server.url());
        std::env::set_var("OPENAI_API_KEY", "test-key");

        let _mock = server
            .mock("POST", "/v1/chat/completions")
            .match_body(Matcher::AllOf(vec![
                Matcher::Regex(r#""response_format":\{"type":"json_object"\}"#.to_string()),
                Matcher::Regex(r#""content":"be strict\\nRespond in JSON only\.""#.to_string()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"model":"gpt-4o-mini","choices":[{"message":{"content":"{\"ok\":true}"}}]}"#,
            )
            .create();

        let client = OpenAiClient::new(None).expect("client should construct");
        let _ = client
            .complete(make_request(true, Some("be strict")))
            .await
            .expect("request should succeed");
    }
}
