use std::sync::Arc;

use memora_core::{build_embedder, Embedder, Index, Vault, VectorIndex};

use crate::config::{AppConfig, EmbedConfig, LlmConfig};

pub fn open_index(vault: &std::path::Path) -> anyhow::Result<Index> {
    Index::open(&vault.join(".memora").join("memora.db")).map_err(Into::into)
}

pub fn open_vector(vault: &std::path::Path, cfg: &EmbedConfig) -> anyhow::Result<VectorIndex> {
    VectorIndex::open_or_create(&vault.join(".memora").join("vectors"), cfg.dim)
}

pub fn open_vault(vault: &std::path::Path) -> Vault {
    Vault::new(vault.to_path_buf())
}

pub fn build_embedder_from_app(
    cfg: &EmbedConfig,
    llm: &LlmConfig,
) -> anyhow::Result<Arc<dyn Embedder>> {
    build_embedder(&cfg.to_core(), &llm.to_core())
}

pub fn privacy_config_from_app(cfg: &AppConfig) -> memora_core::PrivacyConfig {
    cfg.privacy.to_core()
}

trait ToCoreEmbed {
    fn to_core(&self) -> memora_core::vault_config::EmbedConfig;
}

trait ToCoreLlm {
    fn to_core(&self) -> memora_core::vault_config::LlmConfig;
}

impl ToCoreEmbed for EmbedConfig {
    fn to_core(&self) -> memora_core::vault_config::EmbedConfig {
        memora_core::vault_config::EmbedConfig {
            provider: self.provider.clone(),
            model: self.model.clone(),
            dim: self.dim,
            embedding_model: self.embedding_model.clone(),
            endpoint: self.endpoint.clone(),
        }
    }
}

impl ToCoreLlm for LlmConfig {
    fn to_core(&self) -> memora_core::vault_config::LlmConfig {
        memora_core::vault_config::LlmConfig {
            provider: self.provider.clone(),
            model: self.model.clone(),
            embedding_model: self.embedding_model.clone(),
            endpoint: self.endpoint.clone(),
        }
    }
}

#[cfg(test)]
mod build_embedder_tests {
    use memora_core::embed::build::resolve_ollama_embedding_model;

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
        let m = resolve_ollama_embedding_model(&embed.to_core(), &llm.to_core()).expect("resolve");
        assert_eq!(m, "nomic-embed-text");
        assert_ne!(m, "qwen2.5:14b-instruct-q5_K_M");
    }
}
