use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use figment::providers::{Format, Serialized, Toml};
use figment::Figment;
use memora_core::indexer::RefsSyncMode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    pub llm: LlmConfig,
    pub embed: EmbedConfig,
    pub retrieval: RetrievalConfig,
    pub watch: WatchConfig,
    pub frontmatter: FrontmatterConfig,
    pub consolidation: ConsolidationConfig,
    pub challenger: ChallengerConfig,
    pub privacy: PrivacyConfig,
}

impl AppConfig {
    pub fn load(vault_root: &Path) -> Result<Self> {
        let path = config_path(vault_root);
        let figment = Figment::from(Serialized::defaults(Self::default())).merge(Toml::file(path));
        Ok(figment.extract()?)
    }

    pub fn write_default(vault_root: &Path) -> Result<PathBuf> {
        let path = config_path(vault_root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(&Self::default())?;
        fs::write(&path, content)?;
        Ok(path)
    }
}

pub fn config_path(vault_root: &Path) -> PathBuf {
    vault_root.join(".memora").join("config.toml")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    pub model: Option<String>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "ollama".to_string(),
            model: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedConfig {
    pub provider: String,
    pub model: String,
    pub dim: usize,
}

impl Default for EmbedConfig {
    fn default() -> Self {
        Self {
            provider: "deterministic".to_string(),
            model: "memora-cli/deterministic".to_string(),
            dim: 64,
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
pub struct WatchConfig {
    pub debounce_ms: u64,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self { debounce_ms: 250 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontmatterConfig {
    pub refs_mode: String,
}

impl FrontmatterConfig {
    pub fn refs_sync_mode(&self) -> Result<RefsSyncMode> {
        match self.refs_mode.as_str() {
            "sync_from_wikilinks" => Ok(RefsSyncMode::SyncFromWikilinks),
            "manual" => Ok(RefsSyncMode::Manual),
            other => Err(anyhow!(
                "invalid frontmatter.refs_mode `{other}`; expected `sync_from_wikilinks` or `manual`"
            )),
        }
    }
}

impl Default for FrontmatterConfig {
    fn default() -> Self {
        Self {
            refs_mode: "sync_from_wikilinks".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationConfig {
    pub daily_at: String,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            daily_at: "03:00".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChallengerConfig {
    pub daily_at: String,
}

impl Default for ChallengerConfig {
    fn default() -> Self {
        Self {
            daily_at: "07:00".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyConfig {
    pub default_note_privacy: String,
    pub redact_secret_in_cloud: bool,
    pub warn_on_secret_query: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            default_note_privacy: "private".to_string(),
            redact_secret_in_cloud: true,
            warn_on_secret_query: true,
        }
    }
}
