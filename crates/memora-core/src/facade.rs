//! An owned, embeddable entry point to a Memora vault.
//!
//! The internals (`ClaimStore`, `CitationValidator`, `HybridRetriever`) are all
//! lifetime-borrowed, which makes them awkward to hold onto from another program.
//! `Memora` owns the heavy resources (the SQLite index, the vector index, and the
//! embedder) and exposes `&self` methods that build the transient borrowed
//! structs internally. This is the stable surface for embedding Memora as a
//! library, for example to verify an LLM answer's citations from your own code:
//!
//! ```no_run
//! # async fn run() -> anyhow::Result<()> {
//! use memora_core::Memora;
//!
//! let memora = Memora::open("/path/to/vault")?;
//! let answer = memora.validate("drift uses MessagePack [claim:0123456789abcdef].").await?;
//! println!("{} verified, {} rejected", answer.verified_count, answer.unverified_count);
//! # Ok(())
//! # }
//! ```
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::cite::{CitationValidator, CitedAnswer};
use crate::claims::{Claim, ClaimStore};
use crate::embed::{build_embedder, Embedder};
use crate::index::{Index, VectorIndex};
use crate::retrieve::{HybridRetriever, RetrievalHit};
use crate::vault_config::VaultConfig;

/// An open Memora vault: the SQLite claim index, the vector index, the embedder,
/// and the resolved configuration. Cheap to keep around; clone-free borrows are
/// constructed per call.
pub struct Memora {
    vault_root: PathBuf,
    config: VaultConfig,
    index: Index,
    vector: VectorIndex,
    embedder: Arc<dyn Embedder>,
}

impl Memora {
    /// Open the vault at `vault_root`. Reads `{vault}/.memora/config.toml`
    /// (falling back to the global config and then defaults), opens the index at
    /// `{vault}/.memora/memora.db`, and builds the configured embedder.
    ///
    /// Cloud embedders are gated: with `[embed] provider = "openai"`, this fails
    /// unless `MEMORA_ENABLE_NETWORK_LLM=1` is set, mirroring the CLI and MCP.
    pub fn open(vault_root: impl AsRef<Path>) -> Result<Self> {
        let vault_root = vault_root.as_ref().to_path_buf();
        let config = VaultConfig::load(&vault_root)
            .with_context(|| format!("load config for vault {}", vault_root.display()))?;
        let memora_dir = vault_root.join(".memora");
        let index = Index::open(&memora_dir.join("memora.db"))
            .with_context(|| format!("open index for vault {}", vault_root.display()))?;
        let vector = VectorIndex::open_or_create(&memora_dir.join("vectors"), config.embed.dim)
            .context("open vector index")?;
        let embedder = build_embedder(&config.embed, &config.llm).context("build embedder")?;
        Ok(Self {
            vault_root,
            config,
            index,
            vector,
            embedder,
        })
    }

    /// The resolved vault configuration.
    pub fn config(&self) -> &VaultConfig {
        &self.config
    }

    /// The vault root directory.
    pub fn vault_root(&self) -> &Path {
        &self.vault_root
    }

    /// Verify the citations in `text`. This is pure verification: it re-reads each
    /// cited source span, recomputes its fingerprint, and rejects mismatches. It
    /// makes no network calls and needs no LLM. The returned [`CitedAnswer`]
    /// carries per-citation checks and a `clean_text` with unverified statements
    /// stripped.
    pub async fn validate(&self, text: &str) -> Result<CitedAnswer> {
        let store = ClaimStore::new(&self.index);
        let validator = CitationValidator {
            store: &store,
            index: &self.index,
            vault_root: &self.vault_root,
        };
        validator.validate(text).await
    }

    /// Hybrid retrieval (BM25 + embeddings + reciprocal-rank fusion) over the
    /// claim graph. No LLM call.
    pub async fn search(&self, query: &str, k: usize) -> Result<Vec<RetrievalHit>> {
        let retriever = HybridRetriever {
            index: &self.index,
            vec: &self.vector,
            embedder: self.embedder.as_ref(),
        };
        retriever.search(query, k).await
    }

    /// Fetch a single claim by id, if it exists.
    pub fn claim(&self, id: &str) -> Result<Option<Claim>> {
        let store = ClaimStore::new(&self.index);
        Ok(store.get(id)?)
    }

    /// Borrow the underlying index for advanced, read-only use.
    pub fn index(&self) -> &Index {
        &self.index
    }
}
