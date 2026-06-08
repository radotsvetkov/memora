use std::sync::Arc;

use anyhow::{anyhow, Result};
use memora_core::{build_embedder, Embedder, Index, Vault, VectorIndex};
use memora_llm::{make_client, LlmClient, LlmProvider};

use crate::config::{AppConfig, EmbedConfig, LlmConfig};

/// Map a configured provider string to an [`LlmProvider`]. Unknown values fall
/// back to the local Ollama provider.
pub fn provider_from_config(cfg: &LlmConfig) -> LlmProvider {
    match cfg.provider.as_str() {
        "anthropic" => LlmProvider::Anthropic,
        "openai" => LlmProvider::OpenAi,
        _ => LlmProvider::Ollama,
    }
}

fn is_cloud_provider(provider: LlmProvider) -> bool {
    matches!(provider, LlmProvider::Anthropic | LlmProvider::OpenAi)
}

/// Cloud embedding providers transmit note bodies and query text to a remote
/// endpoint, so they are gated behind `MEMORA_ENABLE_NETWORK_LLM` exactly like
/// the cloud LLM providers. Local providers (`deterministic`, `ollama`) stay
/// off the network and are never gated.
fn is_cloud_embed_provider(provider: &str) -> bool {
    matches!(provider, "openai")
}

fn network_llm_enabled() -> bool {
    std::env::var("MEMORA_ENABLE_NETWORK_LLM")
        .map(|value| value == "1")
        .unwrap_or(false)
}

/// Build an LLM client for the configured provider, refusing to construct a
/// cloud client unless `MEMORA_ENABLE_NETWORK_LLM=1` is set. Local providers
/// (Ollama) are always allowed.
///
/// This mirrors the gate the MCP server applies in `memora-mcp`, closing the
/// hole where `provider = "anthropic"` in config would silently route vault
/// content to the cloud from any CLI command.
pub fn make_gated_client(cfg: &LlmConfig) -> Result<Arc<dyn LlmClient>> {
    let provider = provider_from_config(cfg);
    if is_cloud_provider(provider) && !network_llm_enabled() {
        return Err(anyhow!(
            "LLM provider `{}` sends prompts to a cloud endpoint, but network LLM access is \
             disabled. Set MEMORA_ENABLE_NETWORK_LLM=1 to allow it, or use a local provider \
             (`[llm] provider = \"ollama\"`).",
            cfg.provider
        ));
    }
    make_client(
        provider,
        cfg.model.clone(),
        cfg.endpoint.clone(),
        cfg.embedding_model.clone(),
    )
    .map_err(|err| anyhow!("failed to configure LLM client: {err}"))
}

pub fn open_index(vault: &std::path::Path) -> anyhow::Result<Index> {
    Index::open(&vault.join(".memora").join("memora.db")).map_err(Into::into)
}

pub fn open_vector(vault: &std::path::Path, cfg: &EmbedConfig) -> anyhow::Result<VectorIndex> {
    VectorIndex::open_or_create(&vault.join(".memora").join("vectors"), cfg.dim)
}

pub fn open_vault(vault: &std::path::Path) -> Vault {
    Vault::new(vault.to_path_buf())
}

/// Build an embedder for the configured provider, refusing to construct a cloud
/// embedder unless `MEMORA_ENABLE_NETWORK_LLM=1` is set. Local providers
/// (`deterministic`, `ollama`) are always allowed.
///
/// This mirrors [`make_gated_client`] and closes the same privacy hole on the
/// embedding path: `[embed] provider = "openai"` would otherwise route note
/// bodies and query text to a cloud endpoint with no network-flag check. The
/// gate fires at construction time (fail-fast), before any text is embedded —
/// matching the LLM gate's behaviour and not relying on the egress happening
/// only at the first `embed` call.
pub fn build_embedder_from_app(
    cfg: &EmbedConfig,
    llm: &LlmConfig,
) -> anyhow::Result<Arc<dyn Embedder>> {
    if is_cloud_embed_provider(cfg.provider.as_str()) && !network_llm_enabled() {
        return Err(anyhow!(
            "embed provider `{}` sends note bodies and query text to a cloud endpoint, but network \
             LLM access is disabled. Set MEMORA_ENABLE_NETWORK_LLM=1 to allow it, or use a local \
             provider (`[embed] provider = \"ollama\"` or `\"deterministic\"`).",
            cfg.provider
        ));
    }
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
    use std::sync::{Mutex, OnceLock};

    use memora_core::embed::build::resolve_ollama_embedding_model;

    use super::*;

    /// Serialises the tests that mutate the process-global
    /// `MEMORA_ENABLE_NETWORK_LLM` env var so they cannot race each other.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn openai_embed_config() -> EmbedConfig {
        EmbedConfig {
            provider: "openai".into(),
            model: "text-embedding-3-small".into(),
            dim: 1536,
            embedding_model: None,
            endpoint: None,
        }
    }

    fn local_llm_config() -> LlmConfig {
        LlmConfig {
            provider: "ollama".into(),
            model: None,
            embedding_model: None,
            endpoint: None,
        }
    }

    #[test]
    fn cloud_embed_provider_is_gated_by_network_flag() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let embed = openai_embed_config();
        let llm = local_llm_config();

        // Without the flag the cloud embedder is refused at construction time.
        std::env::remove_var("MEMORA_ENABLE_NETWORK_LLM");
        let err = build_embedder_from_app(&embed, &llm)
            .map(|_| ())
            .expect_err("cloud embed provider must be gated without the network flag");
        assert!(
            err.to_string().contains("MEMORA_ENABLE_NETWORK_LLM"),
            "error should point at the network flag, got: {err}"
        );

        // With the flag set, construction is allowed to proceed. The real
        // OpenAI embedder reads OPENAI_API_KEY at construction (it is not
        // validated until the first request), so a dummy key is enough here.
        std::env::set_var("MEMORA_ENABLE_NETWORK_LLM", "1");
        std::env::set_var("OPENAI_API_KEY", "test-key-not-used");
        build_embedder_from_app(&embed, &llm)
            .map(|_| ())
            .expect("cloud embed provider should be allowed with the network flag");
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("MEMORA_ENABLE_NETWORK_LLM");
    }

    #[test]
    fn local_embed_provider_works_without_network_flag() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("MEMORA_ENABLE_NETWORK_LLM");

        let embed = EmbedConfig {
            provider: "deterministic".into(),
            model: "unused".into(),
            dim: 64,
            embedding_model: None,
            endpoint: None,
        };
        let llm = local_llm_config();
        build_embedder_from_app(&embed, &llm)
            .map(|_| ())
            .expect("local embed provider must work without the network flag");
    }

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
