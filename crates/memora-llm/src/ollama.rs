use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::client::{
    shared_http_client, CompletionRequest, CompletionResponse, LlmClient, LlmDestination, LlmError,
};

const DEFAULT_MODEL: &str = "llama3.1:8b";
const DEFAULT_HOST: &str = "http://localhost:11434";
const DEFAULT_KEEP_ALIVE: &str = "24h";

/// Ollama local endpoint-backed LLM client.
pub struct OllamaClient {
    pub(crate) http: reqwest::Client,
    chat_model: String,
    embedding_model: Option<String>,
    base_url: String,
}

impl OllamaClient {
    /// Build a new Ollama client with optional endpoint (`None` uses `OLLAMA_HOST` or localhost).
    pub fn new(
        chat_model: Option<String>,
        endpoint: Option<String>,
        embedding_model: Option<String>,
    ) -> Result<Self, LlmError> {
        let base_url = endpoint
            .or_else(|| std::env::var("OLLAMA_HOST").ok())
            .unwrap_or_else(|| DEFAULT_HOST.to_string());
        let base_url = base_url.trim_end_matches('/').to_string();
        Ok(Self {
            http: shared_http_client(),
            chat_model: chat_model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            embedding_model,
            base_url,
        })
    }

    fn chat_endpoint(&self) -> String {
        format!("{}/api/chat", self.base_url)
    }

    fn embeddings_endpoint(&self) -> String {
        format!("{}/api/embeddings", self.base_url)
    }

    /// Chat model used for completions / extraction.
    pub fn chat_model_name(&self) -> &str {
        &self.chat_model
    }

    /// Model used for `/api/embeddings` (defaults to chat model).
    pub fn resolved_embedding_model(&self) -> String {
        self.embedding_model
            .as_deref()
            .unwrap_or(&self.chat_model)
            .to_string()
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Embed a single text via `/api/embeddings`.
    pub async fn embed_one(&self, prompt: &str) -> Result<Vec<f32>, LlmError> {
        let model_name = self.resolved_embedding_model();
        let body = OllamaEmbedRequest {
            model: model_name.clone(),
            prompt: prompt.to_string(),
        };
        info!(
            embed_model = %model_name,
            "constructing embedding API request"
        );
        debug!(model = %model_name, "sending ollama embeddings request");
        let response = self
            .http
            .post(self.embeddings_endpoint())
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let raw = response.text().await?;
        if status.as_u16() == 429 {
            return Err(LlmError::RateLimited);
        }
        if !status.is_success() {
            if raw.to_ascii_lowercase().contains("model")
                && (raw.contains("not found") || raw.contains("pull"))
            {
                warn!(
                    model = %model_name,
                    "ollama embeddings failed — pull the embedding model with: ollama pull {}",
                    model_name
                );
            }
            return Err(LlmError::ServerError(status.as_u16(), raw));
        }

        let parsed: OllamaEmbedResponse = serde_json::from_str(&raw)?;
        Ok(parsed.embedding)
    }
}

#[async_trait::async_trait]
impl LlmClient for OllamaClient {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let model = req.model.clone().unwrap_or_else(|| self.chat_model.clone());
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
            keep_alive: DEFAULT_KEEP_ALIVE.to_string(),
        };

        debug!(model = %model, keep_alive = %body.keep_alive, "sending ollama completion request");

        let response = self
            .http
            .post(self.chat_endpoint())
            .json(&body)
            .send()
            .await?;

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
        &self.chat_model
    }

    fn destination(&self) -> LlmDestination {
        LlmDestination::Local
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct OllamaRequest {
    pub(crate) model: String,
    pub(crate) messages: Vec<OllamaMessage>,
    pub(crate) stream: bool,
    pub(crate) options: OllamaOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) format: Option<String>,
    pub(crate) keep_alive: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct OllamaMessage {
    role: crate::client::Role,
    content: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct OllamaOptions {
    temperature: f32,
    num_predict: u32,
}

#[derive(Debug, Serialize)]
struct OllamaEmbedRequest {
    model: String,
    prompt: String,
}

#[derive(Debug, Deserialize)]
struct OllamaEmbedResponse {
    #[serde(default)]
    embedding: Vec<f32>,
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

    use super::{OllamaClient, OllamaOptions, OllamaRequest, DEFAULT_KEEP_ALIVE};
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

    #[test]
    fn ollama_request_includes_keep_alive() {
        let body = OllamaRequest {
            model: "llama3.1:8b".into(),
            messages: vec![],
            stream: false,
            options: OllamaOptions {
                temperature: 0.0,
                num_predict: 1,
            },
            format: None,
            keep_alive: DEFAULT_KEEP_ALIVE.into(),
        };
        let j = serde_json::to_string(&body).expect("serialize");
        assert!(
            j.contains("\"keep_alive\":\"24h\""),
            "expected keep_alive in JSON: {j}"
        );
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

        let client = OllamaClient::new(None, None, None).expect("client should construct");
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

        let client = OllamaClient::new(None, None, None).expect("client should construct");
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

        let client = OllamaClient::new(None, None, None).expect("client should construct");
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

        let client = OllamaClient::new(None, None, None).expect("client should construct");
        let _ = client
            .complete(make_request(true))
            .await
            .expect("request should succeed");
    }

    #[test]
    fn resolved_embedding_model_uses_explicit_option_not_chat_model() {
        let client = OllamaClient::new(
            Some("qwen2.5:14b-instruct-q5_K_M".into()),
            None,
            Some("nomic-embed-text".into()),
        )
        .expect("client");
        assert_eq!(client.resolved_embedding_model(), "nomic-embed-text");
        assert_ne!(client.resolved_embedding_model(), client.chat_model_name());
    }

    #[tokio::test]
    async fn embed_calls_embeddings_endpoint() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut server = Server::new_async().await;

        let _mock = server
            .mock("POST", "/api/embeddings")
            .match_body(Matcher::Regex(r#""model":"nomic-embed-text""#.to_string()))
            .with_status(200)
            .with_body(r#"{"embedding":[0.25,-0.5]}"#)
            .create();

        let client = OllamaClient::new(
            Some("llama3.1:8b".into()),
            Some(server.url()),
            Some("nomic-embed-text".into()),
        )
        .expect("client");

        let v = client.embed_one("hello").await.expect("embed");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], 0.25);
    }
}
