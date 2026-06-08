use std::sync::Arc;

use anyhow::{anyhow, Context, Result};

use memora_llm::OllamaClient;

use super::deterministic::DeterministicEmbedder;
use super::{Embedder, OllamaEmbedder, OpenAiEmbedder};
use crate::vault_config::{EmbedConfig, LlmConfig};

use crate::vault_config::network_llm_enabled;

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
        "openai" => {
            // Cloud egress: gate before constructing anything that could send
            // note text to OpenAI (fail-fast, before the first embed call).
            if !network_llm_enabled() {
                return Err(anyhow!(
                    "embed provider `openai` sends note bodies and query text to a cloud \
                     endpoint, but network LLM access is disabled. Set \
                     MEMORA_ENABLE_NETWORK_LLM=1 to allow it, or use a local provider \
                     (`[embed] provider = \"ollama\"` or `\"deterministic\"`)."
                ));
            }
            Ok(Arc::new(
                OpenAiEmbedder::new().context("configure OpenAI embedding client")?,
            ))
        }
        // `deterministic` (and any unrecognised value) stays fully local.
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

#[cfg(test)]
mod gate_tests {
    use std::sync::{Mutex, OnceLock};

    use super::build_embedder;
    use crate::vault_config::{EmbedConfig, LlmConfig};

    /// Serialises tests that mutate the process-global `MEMORA_ENABLE_NETWORK_LLM`.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn openai_embed() -> EmbedConfig {
        EmbedConfig {
            provider: "openai".into(),
            model: "text-embedding-3-small".into(),
            dim: 1536,
            embedding_model: None,
            endpoint: None,
        }
    }

    fn local_llm() -> LlmConfig {
        LlmConfig {
            provider: "ollama".into(),
            model: None,
            embedding_model: None,
            endpoint: None,
        }
    }

    #[test]
    fn openai_embedder_is_gated_in_core_so_mcp_is_covered() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        // Without the flag, the cloud embedder is refused at construction — this
        // is the gate the MCP server relies on (it calls build_embedder directly).
        std::env::remove_var("MEMORA_ENABLE_NETWORK_LLM");
        let err = build_embedder(&openai_embed(), &local_llm())
            .map(|_| ())
            .expect_err("openai embed must be gated in core without the network flag");
        assert!(
            err.to_string().contains("MEMORA_ENABLE_NETWORK_LLM"),
            "error should point at the network flag, got: {err}"
        );
    }

    #[test]
    fn deterministic_embedder_needs_no_flag() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("MEMORA_ENABLE_NETWORK_LLM");
        let embed = EmbedConfig {
            provider: "deterministic".into(),
            model: "unused".into(),
            dim: 64,
            embedding_model: None,
            endpoint: None,
        };
        build_embedder(&embed, &local_llm())
            .map(|_| ())
            .expect("local embedder must build without the network flag");
    }
}
