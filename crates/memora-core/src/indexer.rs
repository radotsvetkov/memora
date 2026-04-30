use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

use crate::claims::{
    ClaimExtractor, ClaimStore, ContradictionDetector, Provenance, StalenessTracker,
};
use crate::embed::{normalize_text, Embedder};
use crate::index::{Index, RebuildStats, VectorIndex};
use crate::note;
use crate::note::{FrontmatterAction, NoteSource};
use crate::vault::{scan, Vault, VaultEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrontmatterFixMode {
    #[default]
    Strict,
    RewriteMissing,
    InferInMemoryOnly,
}

pub struct Indexer<'a> {
    pub vault: &'a Vault,
    pub index: &'a Index,
    pub embedder: Arc<dyn Embedder>,
    pub vector_index: Arc<Mutex<VectorIndex>>,
    pub claim_extractor: Option<ClaimExtractor<'a>>,
    pub claim_store: Option<ClaimStore<'a>>,
    pub extract_reference_notes: bool,
    pub frontmatter_fix_mode: FrontmatterFixMode,
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
            claim_extractor: None,
            claim_store: None,
            extract_reference_notes: false,
            frontmatter_fix_mode: FrontmatterFixMode::Strict,
        }
    }

    pub fn with_claims(
        mut self,
        claim_extractor: ClaimExtractor<'a>,
        claim_store: ClaimStore<'a>,
    ) -> Self {
        self.claim_extractor = Some(claim_extractor);
        self.claim_store = Some(claim_store);
        self
    }

    pub fn with_reference_claim_extraction(mut self, enabled: bool) -> Self {
        self.extract_reference_notes = enabled;
        self
    }

    pub fn with_frontmatter_fix_mode(mut self, mode: FrontmatterFixMode) -> Self {
        self.frontmatter_fix_mode = mode;
        self
    }

    pub async fn full_rebuild(&self) -> Result<RebuildStats> {
        let mut stats = RebuildStats::default();
        for path in scan(self.vault.root()) {
            match self.parse_note_for_indexing(&path) {
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
                let parsed = self.parse_note_for_indexing(&path)?;
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

    fn parse_note_for_indexing(&self, path: &std::path::Path) -> Result<crate::note::Note> {
        match self.frontmatter_fix_mode {
            FrontmatterFixMode::Strict => note::parse(path).map_err(Into::into),
            FrontmatterFixMode::RewriteMissing => {
                let (note, action) = note::parse_or_infer(path, self.vault.root())?;
                if action == FrontmatterAction::InferredAndRewritten {
                    tracing::info!("rewrote frontmatter for {}", path.display());
                }
                Ok(note)
            }
            FrontmatterFixMode::InferInMemoryOnly => {
                let (note, action) = note::parse_or_infer_in_memory(path, self.vault.root())?;
                if action == FrontmatterAction::InferredInMemoryOnly {
                    tracing::info!(path = %path.display(), "inferred frontmatter in-memory only");
                }
                Ok(note)
            }
        }
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
        let mut old_claim_ids = Vec::new();
        if let (Some(_extractor), Some(claim_store)) = (&self.claim_extractor, &self.claim_store) {
            let should_extract =
                self.extract_reference_notes || parsed.fm.source != NoteSource::Reference;
            if should_extract {
                old_claim_ids = claim_store.claim_ids_for_note(&parsed.fm.id)?;
            }
        }

        self.index.upsert_note(parsed, &parsed.body)?;

        if let (Some(extractor), Some(claim_store)) = (&self.claim_extractor, &self.claim_store) {
            let should_extract =
                self.extract_reference_notes || parsed.fm.source != NoteSource::Reference;
            if should_extract {
                let claims = extractor.extract(parsed, &parsed.body).await?;
                self.index.with_transaction(|tx| {
                    claim_store.delete_for_note_in_tx(tx, &parsed.fm.id)?;
                    for claim in &claims {
                        claim_store.upsert_in_tx(tx, claim)?;
                    }
                    Ok(())
                })?;

                let provenance = Provenance::new(self.index);
                let stale_tracker = StalenessTracker::new(self.index, &provenance);
                stale_tracker.mark_source_edited_claims(&old_claim_ids)?;
                let contradiction_detector = ContradictionDetector {
                    store: claim_store,
                    stale: &stale_tracker,
                    llm: extractor.llm,
                };
                for claim in &claims {
                    let superseded = contradiction_detector.check_new_claim(claim).await?;
                    if !superseded.is_empty() {
                        tracing::info!(
                            claim_id = %claim.id,
                            count = superseded.len(),
                            "supersession recorded"
                        );
                    }
                }
            }
        }
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
