use std::sync::Arc;

use anyhow::{anyhow, Context, Result};

use memora_llm::OllamaClient;

use super::deterministic::DeterministicEmbedder;
use super::{Embedder, OllamaEmbedder};
use crate::vault_config::{EmbedConfig, LlmConfig};

pub fn build_embedder(embed: &EmbedConfig, llm: &LlmConfig) -> Result<Arc<dyn Embedder>> {
    match embed.provider.as_str() {
        "ollama" => {
            log_ollama_embed_config_warnings(embed, llm);
            let embedding_model = resolve_ollama_embedding_model(embed, llm)?;
            let endpoint = embed.endpoint.clone().or_else(|| llm.endpoint.clone());
            let chat_model = llm
                .model
                .clone()
                .unwrap_or_else(|| "llama3.1:8b".to_string());
            let client = OllamaClient::new(Some(chat_model), endpoint, Some(embedding_model))
                .map_err(|e| anyhow!(e.to_string()))
                .context("configure Ollama embedding client")?;
            Ok(Arc::new(OllamaEmbedder::new(Arc::new(client), embed.dim)))
        }
        _ => Ok(Arc::new(DeterministicEmbedder::new(embed.dim))),
    }
}

fn log_ollama_embed_config_warnings(embed: &EmbedConfig, llm: &LlmConfig) {
    if embed.provider != "ollama" {
        return;
    }
    let embed_set = embed
        .embedding_model
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if !embed_set {
        tracing::warn!(
            "no [embed].embedding_model set in config; Ollama will fall back to [llm].embedding_model \
             and then the chat model (wrong dimensions for most setups). \
             Add embedding_model = \"nomic-embed-text\" under [embed]."
        );
        if llm.embedding_model.is_some() {
            tracing::warn!(
                "using legacy [llm].embedding_model until [embed].embedding_model is set explicitly."
            );
        }
    }
}

pub fn resolve_ollama_embedding_model(embed: &EmbedConfig, llm: &LlmConfig) -> Result<String> {
    embed
        .embedding_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| {
            llm.embedding_model
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
        })
        .ok_or_else(|| {
            anyhow!(
                "no embedding model configured. Set [embed].embedding_model in \
                 .memora/config.toml (e.g. \"nomic-embed-text\") or [llm].embedding_model \
                 for legacy setups."
            )
        })
}
