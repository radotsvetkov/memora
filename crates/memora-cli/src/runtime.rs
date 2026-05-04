use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use memora_core::{Embedder, Index, OllamaEmbedder, Vault, VectorIndex};
use memora_llm::OllamaClient;

use crate::config::{EmbedConfig, LlmConfig};

pub fn open_index(vault: &std::path::Path) -> Result<Index> {
    Index::open(&vault.join(".memora").join("memora.db")).map_err(Into::into)
}

pub fn open_vector(vault: &std::path::Path, cfg: &EmbedConfig) -> Result<VectorIndex> {
    VectorIndex::open_or_create(&vault.join(".memora").join("vectors"), cfg.dim)
}

pub fn open_vault(vault: &std::path::Path) -> Vault {
    Vault::new(vault.to_path_buf())
}

#[derive(Debug, Clone)]
pub struct DeterministicEmbedder {
    dim: usize,
}

impl DeterministicEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

#[async_trait]
impl Embedder for DeterministicEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for text in texts {
            let mut bytes = blake3::hash(text.as_bytes()).as_bytes().to_vec();
            while bytes.len() < self.dim * 4 {
                bytes.extend_from_slice(blake3::hash(&bytes).as_bytes());
            }
            let mut vector = Vec::with_capacity(self.dim);
            for chunk in bytes.chunks_exact(4).take(self.dim) {
                let bits = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                vector.push((bits as f32 / u32::MAX as f32) * 2.0 - 1.0);
            }
            out.push(vector);
        }
        Ok(out)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        "memora-cli/deterministic"
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

/// Resolution order: `[embed].embedding_model`, then legacy `[llm].embedding_model`.
pub(crate) fn resolve_ollama_embedding_model(cfg: &EmbedConfig, llm: &LlmConfig) -> Result<String> {
    cfg.embedding_model
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
                 ~/.config/memora/config.toml (e.g. \"nomic-embed-text\") or [llm].embedding_model \
                 for legacy setups."
            )
        })
}

pub fn build_embedder(cfg: &EmbedConfig, llm: &LlmConfig) -> Result<Arc<dyn Embedder>> {
    match cfg.provider.as_str() {
        "ollama" => {
            log_ollama_embed_config_warnings(cfg, llm);
            let embedding_model = resolve_ollama_embedding_model(cfg, llm)?;
            let endpoint = cfg.endpoint.clone().or_else(|| llm.endpoint.clone());
            let client = OllamaClient::new(llm.model.clone(), endpoint, Some(embedding_model))
                .map_err(|e| anyhow::anyhow!(e.to_string()))
                .context("configure Ollama embedding client")?;
            Ok(Arc::new(OllamaEmbedder::new(Arc::new(client), cfg.dim)))
        }
        _ => Ok(Arc::new(DeterministicEmbedder::new(cfg.dim))),
    }
}

#[cfg(test)]
mod build_embedder_tests {
    use super::*;

    #[test]
    fn resolve_prefers_embed_embedding_model_over_llm_chat_model() {
        let embed = EmbedConfig {
            provider: "ollama".into(),
            model: "unused-for-ollama-embeddings".into(),
            dim: 768,
            embedding_model: Some("nomic-embed-text".into()),
            endpoint: None,
        };
        let llm = LlmConfig {
            provider: "ollama".into(),
            model: Some("qwen2.5:14b-instruct-q5_K_M".into()),
            embedding_model: None,
            endpoint: None,
        };
        let m = resolve_ollama_embedding_model(&embed, &llm).expect("resolve");
        assert_eq!(m, "nomic-embed-text");
        assert_ne!(m, "qwen2.5:14b-instruct-q5_K_M");
    }

    #[test]
    fn resolve_falls_back_to_llm_embedding_model_when_embed_unset() {
        let embed = EmbedConfig {
            provider: "ollama".into(),
            model: "x".into(),
            dim: 768,
            embedding_model: None,
            endpoint: None,
        };
        let llm = LlmConfig {
            provider: "ollama".into(),
            model: Some("qwen2.5:14b-instruct-q5_K_M".into()),
            embedding_model: Some("nomic-embed-text".into()),
            endpoint: None,
        };
        assert_eq!(
            resolve_ollama_embedding_model(&embed, &llm).expect("resolve"),
            "nomic-embed-text"
        );
    }
}
