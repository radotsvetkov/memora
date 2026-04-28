use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

use crate::embed::{normalize_text, Embedder};
use crate::index::{Index, RebuildStats, VectorIndex};
use crate::note;
use crate::vault::{scan, Vault, VaultEvent};

pub struct Indexer<'a> {
    pub vault: &'a Vault,
    pub index: &'a Index,
    pub embedder: Arc<dyn Embedder>,
    pub vector_index: Arc<Mutex<VectorIndex>>,
}

impl<'a> Indexer<'a> {
    pub fn new(
        vault: &'a Vault,
        index: &'a Index,
        embedder: Arc<dyn Embedder>,
        vector_index: Arc<Mutex<VectorIndex>>,
    ) -> Self {
        Self {
            vault,
            index,
            embedder,
            vector_index,
        }
    }

    pub async fn full_rebuild(&self) -> Result<RebuildStats> {
        let mut stats = RebuildStats::default();
        for path in scan(self.vault.root()) {
            match note::parse(&path) {
                Ok(parsed) => {
                    if let Err(err) = self.upsert_note(&parsed).await {
                        stats.errors += 1;
                        tracing::warn!(path = %path.display(), error = %err, "failed to upsert parsed note");
                    } else {
                        stats.inserted += 1;
                    }
                }
                Err(err) => {
                    stats.skipped += 1;
                    stats.errors += 1;
                    tracing::warn!(path = %path.display(), error = %err, "failed to parse note during rebuild");
                }
            }
        }
        self.vector_index
            .lock()
            .map_err(|_| anyhow::anyhow!("vector index mutex poisoned"))?
            .save()
            .context("save vector index after full rebuild")?;
        Ok(stats)
    }

    pub async fn handle_event(&self, ev: VaultEvent) -> Result<()> {
        match ev {
            VaultEvent::Modified(path) | VaultEvent::Created(path) => {
                let parsed = note::parse(&path)?;
                self.upsert_note(&parsed).await?;
            }
            VaultEvent::Deleted(path) => {
                if let Some(id) = self.index.id_by_path(&path)? {
                    self.index.delete_note(&id)?;
                    self.vector_index
                        .lock()
                        .map_err(|_| anyhow::anyhow!("vector index mutex poisoned"))?
                        .delete(&id)?;
                }
            }
        }
        Ok(())
    }

    pub async fn vector_search(&self, query: &str, k: usize) -> Result<Vec<(String, f32)>> {
        let normalized = normalize_text(query);
        let vectors = self.embedder.embed(&[normalized]).await?;
        let query_vec = vectors
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("embedder returned no vector for query"))?;
        self.vector_index
            .lock()
            .map_err(|_| anyhow::anyhow!("vector index mutex poisoned"))?
            .search(&query_vec, k)
    }

    async fn upsert_note(&self, parsed: &crate::note::Note) -> Result<()> {
        self.index.upsert_note(parsed, &parsed.body)?;
        let text = make_embedding_text(&parsed.fm.summary, &parsed.body);
        let vectors = self.embedder.embed(&[text]).await?;
        let vector = vectors
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("embedder returned no vector for note"))?;
        self.vector_index
            .lock()
            .map_err(|_| anyhow::anyhow!("vector index mutex poisoned"))?
            .upsert(&parsed.fm.id, &vector)?;
        Ok(())
    }
}

fn make_embedding_text(summary: &str, body: &str) -> String {
    let body_head: String = body.chars().take(2_000).collect();
    normalize_text(&format!("{summary}\n{body_head}"))
}
