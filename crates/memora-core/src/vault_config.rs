use std::path::{Path, PathBuf};

use anyhow::Result;
use figment::providers::{Format, Serialized, Toml};
use figment::Figment;
use memora_llm::LlmProvider;
use serde::{Deserialize, Serialize};

use crate::config::PrivacyConfig;
use crate::note::Privacy;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VaultConfig {
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub embed: EmbedConfig,
    #[serde(default)]
    pub retrieval: RetrievalConfig,
    #[serde(default)]
    pub privacy: VaultPrivacyConfig,
}

impl VaultConfig {
    pub fn load(vault_root: &Path) -> Result<Self> {
        let vault_config = config_path(vault_root);
        let global_config = global_config_path();
        Self::load_from_paths(&vault_config, global_config.as_deref())
    }

    fn load_from_paths(vault_config: &Path, global_config: Option<&Path>) -> Result<Self> {
        let chosen = if vault_config.exists() {
            Some(vault_config.to_path_buf())
        } else {
            global_config
                .filter(|path| path.exists())
                .map(|path| path.to_path_buf())
        };

        let figment = match chosen {
            Some(path) => {
                Figment::from(Serialized::defaults(Self::default())).merge(Toml::file(path))
            }
            None => Figment::from(Serialized::defaults(Self::default())),
        };
        Ok(figment.extract()?)
    }

    pub fn privacy_config(&self) -> PrivacyConfig {
        self.privacy.to_core()
    }

    pub fn llm_provider(&self) -> LlmProvider {
        match self.llm.provider.as_str() {
            "anthropic" => LlmProvider::Anthropic,
            "openai" => LlmProvider::OpenAi,
            _ => LlmProvider::Ollama,
        }
    }
}

pub fn config_path(vault_root: &Path) -> PathBuf {
    vault_root.join(".memora").join("config.toml")
}

pub fn global_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".config").join("memora").join("config.toml"))
}

/// Whether cloud LLM/embedding egress is explicitly enabled. Off by default, so
/// configuring a cloud provider never silently sends vault content off-machine.
/// Used by the embedder builder, the LLM client gate, and the MCP server.
pub fn network_llm_enabled() -> bool {
    std::env::var("MEMORA_ENABLE_NETWORK_LLM")
        .map(|value| value == "1")
        .unwrap_or(false)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    pub model: Option<String>,
    #[serde(default)]
    pub embedding_model: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "ollama".to_string(),
            model: None,
            embedding_model: None,
            endpoint: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedConfig {
    pub provider: String,
    pub model: String,
    pub dim: usize,
    #[serde(default)]
    pub embedding_model: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
}

impl Default for EmbedConfig {
    fn default() -> Self {
        Self {
            provider: "deterministic".to_string(),
            model: "memora/deterministic".to_string(),
            dim: 64,
            embedding_model: None,
            endpoint: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalConfig {
    pub top_k: usize,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self { top_k: 5 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultPrivacyConfig {
    pub default_note_privacy: String,
    pub redact_secret_in_cloud: bool,
    pub warn_on_secret_query: bool,
}

impl Default for VaultPrivacyConfig {
    fn default() -> Self {
        Self {
            default_note_privacy: "private".to_string(),
            redact_secret_in_cloud: true,
            warn_on_secret_query: true,
        }
    }
}

impl VaultPrivacyConfig {
    pub fn to_core(&self) -> PrivacyConfig {
        let default_note_privacy = match self.default_note_privacy.as_str() {
            "public" => Privacy::Public,
            "secret" => Privacy::Secret,
            _ => Privacy::Private,
        };
        PrivacyConfig {
            default_note_privacy,
            redact_secret_in_cloud: self.redact_secret_in_cloud,
            warn_on_secret_query: self.warn_on_secret_query,
        }
    }
}
