use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::client::{
    shared_http_client, CompletionRequest, CompletionResponse, LlmClient, LlmDestination, LlmError,
};

const DEFAULT_MODEL: &str = "llama3.1:8b";
const DEFAULT_HOST: &str = "http://localhost:11434";

/// Ollama local endpoint-backed LLM client.
pub struct OllamaClient {
    pub(crate) http: reqwest::Client,
    pub(crate) model: String,
}

impl OllamaClient {
    /// Build a new Ollama client.
    pub fn new(model: Option<String>) -> Result<Self, LlmError> {
        Ok(Self {
            http: shared_http_client(),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        })
    }

    fn endpoint() -> String {
        let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string());
        format!("{host}/api/chat")
    }
}

#[async_trait::async_trait]
impl LlmClient for OllamaClient {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let model = req.model.clone().unwrap_or_else(|| self.model.clone());
        let body = OllamaRequest {
            model: model.clone(),
            messages: req
                .messages
                .into_iter()
                .map(|message| OllamaMessage {
                    role: message.role,
                    content: message.content,
                })
                .collect(),
            stream: false,
            options: OllamaOptions {
                temperature: req.temperature,
                num_predict: req.max_tokens,
            },
            format: req.json_mode.then_some("json".to_string()),
        };

        debug!(model = %model, "sending ollama completion request");

        let response = self.http.post(Self::endpoint()).json(&body).send().await?;

        let status = response.status();
        let raw = response.text().await?;
        if status.as_u16() == 429 {
            return Err(LlmError::RateLimited);
        }
        if status.is_server_error() {
            return Err(LlmError::ServerError(status.as_u16(), raw));
        }
        if !status.is_success() {
            return Err(LlmError::Auth(raw));
        }

        let parsed: OllamaResponse = serde_json::from_str(&raw)?;
        Ok(CompletionResponse {
            text: parsed.message.map(|msg| msg.content).unwrap_or_default(),
            model: parsed.model.unwrap_or(model),
            input_tokens: parsed.prompt_eval_count,
            output_tokens: parsed.eval_count,
        })
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn destination(&self) -> LlmDestination {
        LlmDestination::Local
    }
}

#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
    options: OllamaOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
}

#[derive(Debug, Serialize)]
struct OllamaMessage {
    role: crate::client::Role,
    content: String,
}

#[derive(Debug, Serialize)]
struct OllamaOptions {
    temperature: f32,
    num_predict: u32,
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    message: Option<OllamaResponseMessage>,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
    #[serde(default)]
    eval_count: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OllamaResponseMessage {
    #[serde(default)]
    content: String,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::await_holding_lock)]

    use std::sync::{Mutex, OnceLock};

    use mockito::{Matcher, Server};

    use crate::client::{CompletionRequest, LlmError, Message, Role};

    use super::OllamaClient;
    use crate::LlmClient;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn make_request(json_mode: bool) -> CompletionRequest {
        CompletionRequest {
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
        std::env::set_var("OLLAMA_HOST", server.url());

        let _mock = server
            .mock("POST", "/api/chat")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                  "model":"llama3.1:8b",
                  "message":{"role":"assistant","content":"ok"},
                  "prompt_eval_count":8,
                  "eval_count":3
                }"#,
            )
            .create();

        let client = OllamaClient::new(None).expect("client should construct");
        let out = client
            .complete(make_request(false))
            .await
            .expect("request should succeed");

        assert_eq!(out.text, "ok");
        assert_eq!(out.model, "llama3.1:8b");
        assert_eq!(out.input_tokens, Some(8));
        assert_eq!(out.output_tokens, Some(3));
    }

    #[tokio::test]
    async fn complete_maps_429_to_rate_limited() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut server = Server::new_async().await;
        std::env::set_var("OLLAMA_HOST", server.url());

        let _mock = server.mock("POST", "/api/chat").with_status(429).create();

        let client = OllamaClient::new(None).expect("client should construct");
        let err = client
            .complete(make_request(false))
            .await
            .expect_err("request should fail");
        assert!(matches!(err, LlmError::RateLimited));
    }

    #[tokio::test]
    async fn complete_maps_500_to_server_error() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut server = Server::new_async().await;
        std::env::set_var("OLLAMA_HOST", server.url());

        let _mock = server
            .mock("POST", "/api/chat")
            .with_status(500)
            .with_body("ollama crashed")
            .create();

        let client = OllamaClient::new(None).expect("client should construct");
        let err = client
            .complete(make_request(false))
            .await
            .expect_err("request should fail");
        assert!(matches!(err, LlmError::ServerError(500, body) if body == "ollama crashed"));
    }

    #[tokio::test]
    async fn json_mode_sets_format_to_json() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut server = Server::new_async().await;
        std::env::set_var("OLLAMA_HOST", server.url());

        let _mock = server
            .mock("POST", "/api/chat")
            .match_body(Matcher::Regex(r#""format":"json""#.to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"model":"llama3.1:8b","message":{"content":"{\"ok\":true}"}}"#)
            .create();

        let client = OllamaClient::new(None).expect("client should construct");
        let _ = client
            .complete(make_request(true))
            .await
            .expect("request should succeed");
    }
}
