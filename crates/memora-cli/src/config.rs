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
    pub indexing: IndexingConfig,
    pub retrieval: RetrievalConfig,
    pub watch: WatchConfig,
    pub frontmatter: FrontmatterConfig,
    pub consolidation: ConsolidationConfig,
    pub challenger: ChallengerConfig,
    pub privacy: PrivacyConfig,
}

impl AppConfig {
    pub fn load(vault_root: &Path) -> Result<Self> {
        let vault_config = config_path(vault_root);
        let global_config = global_config_path();
        Self::load_from_paths(&vault_config, global_config.as_deref())
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
}

pub fn config_path(vault_root: &Path) -> PathBuf {
    vault_root.join(".memora").join("config.toml")
}

pub fn global_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".config").join("memora").join("config.toml"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    pub model: Option<String>,
    /// Used for `/api/embeddings` when `[embed] provider = "ollama"`. Falls back to `model`.
    #[serde(default)]
    pub embedding_model: Option<String>,
    /// Ollama base URL (e.g. `http://localhost:11434`). Falls back to `OLLAMA_HOST` or localhost.
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

fn default_index_parallelism() -> usize {
    8
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingConfig {
    #[serde(default = "default_index_parallelism")]
    pub parallelism: usize,
}

impl Default for IndexingConfig {
    fn default() -> Self {
        Self {
            parallelism: default_index_parallelism(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedConfig {
    pub provider: String,
    pub model: String,
    pub dim: usize,
    /// Ollama model name for `/api/embeddings` (e.g. `nomic-embed-text`). Preferred over `[llm].embedding_model`.
    #[serde(default)]
    pub embedding_model: Option<String>,
    /// Optional Ollama base URL for embeddings only; defaults to `[llm].endpoint` then `OLLAMA_HOST`.
    #[serde(default)]
    pub endpoint: Option<String>,
}

impl Default for EmbedConfig {
    fn default() -> Self {
        Self {
            provider: "deterministic".to_string(),
            model: "memora-cli/deterministic".to_string(),
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

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn config_loads_from_vault_local_first() {
        let temp = tempdir().expect("create tempdir");
        let vault = temp.path().join("vault");
        fs::create_dir_all(vault.join(".memora")).expect("create vault config dir");
        let global_root = temp.path().join("home/.config/memora");
        fs::create_dir_all(&global_root).expect("create global config dir");

        let vault_cfg = vault.join(".memora/config.toml");
        let global_cfg = global_root.join("config.toml");
        fs::write(&vault_cfg, "[llm]\nprovider = \"anthropic\"\n").expect("write vault config");
        fs::write(&global_cfg, "[llm]\nprovider = \"openai\"\n").expect("write global config");

        let cfg = AppConfig::load_from_paths(&vault_cfg, Some(&global_cfg)).expect("load config");
        assert_eq!(cfg.llm.provider, "anthropic");
    }

    #[test]
    fn config_falls_back_to_global() {
        let temp = tempdir().expect("create tempdir");
        let vault_cfg = temp.path().join("vault/.memora/config.toml");
        let global_cfg = temp.path().join("home/.config/memora/config.toml");
        fs::create_dir_all(
            global_cfg
                .parent()
                .expect("global config path should have a parent directory"),
        )
        .expect("create global config dir");
        fs::write(&global_cfg, "[llm]\nprovider = \"anthropic\"\n").expect("write global config");

        let cfg = AppConfig::load_from_paths(&vault_cfg, Some(&global_cfg)).expect("load config");
        assert_eq!(cfg.llm.provider, "anthropic");
    }

    #[test]
    fn config_returns_default_when_neither() {
        let temp = tempdir().expect("create tempdir");
        let vault_cfg = temp.path().join("vault/.memora/config.toml");
        let global_cfg = temp.path().join("home/.config/memora/config.toml");

        let cfg = AppConfig::load_from_paths(&vault_cfg, Some(&global_cfg)).expect("load config");
        assert_eq!(cfg.llm.provider, AppConfig::default().llm.provider);
        assert_eq!(cfg.embed.model, AppConfig::default().embed.model);
        assert_eq!(
            cfg.indexing.parallelism,
            AppConfig::default().indexing.parallelism
        );
    }

    #[test]
    fn config_deserializes_embed_embedding_model_from_toml() {
        let temp = tempdir().expect("create tempdir");
        let vault = temp.path().join("vault");
        fs::create_dir_all(vault.join(".memora")).expect("create .memora");
        let path = vault.join(".memora/config.toml");
        fs::write(
            &path,
            r#"[llm]
provider = "ollama"
model = "qwen2.5:14b-instruct-q5_K_M"

[embed]
provider = "ollama"
model = "memora-cli/deterministic"
dim = 768
embedding_model = "nomic-embed-text"
"#,
        )
        .expect("write config");

        let cfg = AppConfig::load_from_paths(&path, None).expect("load");
        assert_eq!(
            cfg.embed.embedding_model.as_deref(),
            Some("nomic-embed-text")
        );
        assert_eq!(
            cfg.llm.model.as_deref(),
            Some("qwen2.5:14b-instruct-q5_K_M")
        );
    }
}
