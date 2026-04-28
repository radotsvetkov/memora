use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use memora_core::{Embedder, Index, Vault, VectorIndex};

use crate::config::EmbedConfig;

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

pub fn build_embedder(cfg: &EmbedConfig) -> Arc<dyn Embedder> {
    Arc::new(DeterministicEmbedder::new(cfg.dim))
}
