use std::fmt::{Display, Formatter};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::anthropic::AnthropicClient;
use crate::ollama::OllamaClient;
use crate::openai::OpenAiClient;

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Shared message role across all providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// A system instruction message.
    System,
    /// A user-authored message.
    User,
    /// An assistant-authored message.
    Assistant,
}

impl Display for Role {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System => write!(f, "system"),
            Self::User => write!(f, "user"),
            Self::Assistant => write!(f, "assistant"),
        }
    }
}

/// A chat message sent to or received from an LLM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Message role.
    pub role: Role,
    /// Message text content.
    pub content: String,
}

/// Provider-agnostic completion request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// Optional model override.
    pub model: Option<String>,
    /// Optional system instruction.
    pub system: Option<String>,
    /// Chat messages.
    pub messages: Vec<Message>,
    /// Maximum output tokens.
    pub max_tokens: u32,
    /// Sampling temperature.
    pub temperature: f32,
    /// Whether JSON-only output is required.
    pub json_mode: bool,
}

impl Default for CompletionRequest {
    fn default() -> Self {
        Self {
            model: None,
            system: None,
            messages: Vec::new(),
            max_tokens: 1_024,
            temperature: 0.2,
            json_mode: false,
        }
    }
}

/// Provider-agnostic completion response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionResponse {
    /// Output text.
    pub text: String,
    /// Model used by the provider.
    pub model: String,
    /// Optional input token count.
    pub input_tokens: Option<u32>,
    /// Optional output token count.
    pub output_tokens: Option<u32>,
}

/// Errors returned by provider implementations.
#[derive(Debug, Error)]
pub enum LlmError {
    /// Transport-level HTTP error.
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    /// Authentication or authorization error.
    #[error("authentication error: {0}")]
    Auth(String),
    /// Provider rate-limited the request.
    #[error("rate limited")]
    RateLimited,
    /// Provider server-side error.
    #[error("server error {0}: {1}")]
    ServerError(u16, String),
    /// Invalid JSON payload in request/response processing.
    #[error("invalid json: {0}")]
    InvalidJson(#[from] serde_json::Error),
    /// Missing required API key in environment.
    #[error("missing api key in environment variable {0}")]
    MissingApiKey(&'static str),
}

/// Destination category for privacy policy enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmDestination {
    /// A local, on-device model endpoint.
    Local,
    /// A known cloud provider endpoint.
    CloudKnown,
    /// A cloud endpoint whose residency/compliance is unknown.
    CloudUnknown,
}

/// LLM provider type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProvider {
    /// Anthropic hosted API.
    Anthropic,
    /// OpenAI hosted API.
    OpenAi,
    /// Local Ollama endpoint.
    Ollama,
}

/// Unified trait implemented by all provider clients.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Execute a single completion request.
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError>;

    /// Return the default model configured for this client.
    fn model_name(&self) -> &str;

    /// Return where this client sends prompts.
    fn destination(&self) -> LlmDestination;

    /// Structured JSON output (`json_mode`); passes `schema_hint` as optional system instruction.
    async fn chat_json(
        &self,
        prompt: &str,
        schema_hint: Option<&str>,
        max_tokens: u32,
        temperature: f32,
    ) -> Result<String, LlmError> {
        let resp = self
            .complete(CompletionRequest {
                model: None,
                system: schema_hint.map(|s| s.to_string()),
                messages: vec![Message {
                    role: Role::User,
                    content: prompt.to_string(),
                }],
                max_tokens,
                temperature,
                json_mode: true,
            })
            .await?;
        Ok(resp.text)
    }
}

/// Construct a shared provider client with provider defaults.
///
/// For Ollama, pass `ollama_endpoint` / `ollama_embedding_model` from config (`None` uses env /
/// defaults).
pub fn make_client(
    provider: LlmProvider,
    model: Option<String>,
    ollama_endpoint: Option<String>,
    ollama_embedding_model: Option<String>,
) -> Result<Arc<dyn LlmClient>, LlmError> {
    match provider {
        LlmProvider::Anthropic => Ok(Arc::new(AnthropicClient::new(
            model.or_else(|| Some("claude-sonnet-4-6".to_string())),
        )?)),
        LlmProvider::OpenAi => Ok(Arc::new(OpenAiClient::new(
            model.or_else(|| Some("gpt-4o-mini".to_string())),
        )?)),
        LlmProvider::Ollama => Ok(Arc::new(OllamaClient::new(
            model.or_else(|| Some("llama3.1:8b".to_string())),
            ollama_endpoint,
            ollama_embedding_model,
        )?)),
    }
}

pub(crate) fn shared_http_client() -> reqwest::Client {
    HTTP_CLIENT.get_or_init(reqwest::Client::new).clone()
}
