//! Provider-agnostic LLM clients for Memora.

pub mod anthropic;
pub mod client;
pub mod ollama;
pub mod openai;

pub use client::{
    make_client, CompletionRequest, CompletionResponse, LlmClient, LlmDestination, LlmError,
    LlmProvider, Message, Role,
};
pub use ollama::OllamaClient;
