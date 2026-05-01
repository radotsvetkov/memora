use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RefsSyncMode {
    #[default]
    SyncFromWikilinks,
    Manual,
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
    pub refs_sync_mode: RefsSyncMode,
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
            refs_sync_mode: RefsSyncMode::SyncFromWikilinks,
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

    pub fn with_refs_sync_mode(mut self, mode: RefsSyncMode) -> Self {
        self.refs_sync_mode = mode;
        self
    }

    pub async fn full_rebuild(&self) -> Result<RebuildStats> {
        let mut stats = RebuildStats::default();
        let mut seen_ids = std::collections::HashSet::new();
        for path in scan(self.vault.root()) {
            match self.parse_note_for_indexing(&path) {
                Ok(parsed) => {
                    let parsed = match self.align_frontmatter_to_filesystem(path.as_path(), parsed)
                    {
                        Ok(parsed) => parsed,
                        Err(err) => {
                            let error_chain =
                                err.chain().map(ToString::to_string).collect::<Vec<_>>();
                            stats.skipped += 1;
                            stats.errors += 1;
                            tracing::warn!(
                                path = %path.display(),
                                error = %err,
                                error_chain = ?error_chain,
                                "failed to align note region during rebuild"
                            );
                            continue;
                        }
                    };
                    seen_ids.insert(parsed.fm.id.clone());
                    if let Err(err) = self.upsert_note(&parsed).await {
                        let error_chain = err.chain().map(ToString::to_string).collect::<Vec<_>>();
                        stats.errors += 1;
                        tracing::warn!(
                            path = %path.display(),
                            error = %err,
                            error_chain = ?error_chain,
                            "failed to index note"
                        );
                    } else {
                        stats.inserted += 1;
                    }
                }
                Err(err) => {
                    let error_chain = err.chain().map(ToString::to_string).collect::<Vec<_>>();
                    stats.skipped += 1;
                    stats.errors += 1;
                    tracing::warn!(
                        path = %path.display(),
                        error = %err,
                        error_chain = ?error_chain,
                        "failed to index note"
                    );
                }
            }
        }
        let pruned = self.prune_missing_notes(&seen_ids)?;
        if pruned > 0 {
            tracing::info!(pruned, "pruned deleted notes during full rebuild");
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
            VaultEvent::Modified(path) | VaultEvent::Created(path) | VaultEvent::Renamed(path) => {
                let parsed = self.parse_note_for_indexing(&path)?;
                let parsed = self.align_frontmatter_to_filesystem(path.as_path(), parsed)?;
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

    fn align_frontmatter_to_filesystem(
        &self,
        path: &std::path::Path,
        mut parsed: crate::note::Note,
    ) -> Result<crate::note::Note> {
        let mut changed = false;

        if let Some(conflicting_path) = self.conflicting_path_for_note_id(path, &parsed.fm.id)? {
            let new_id = self.next_available_note_id(&parsed.fm.id)?;
            tracing::warn!(
                path = %path.display(),
                conflicting_path = %conflicting_path.display(),
                old_id = %parsed.fm.id,
                new_id = %new_id,
                "detected duplicate note id; rewriting with unique id"
            );
            parsed.fm.id = new_id;
            changed = true;
        }

        let expected_region = note::derive_region_from_path(path, self.vault.root());
        if parsed.fm.region != expected_region {
            tracing::info!(
                path = %path.display(),
                old_region = %parsed.fm.region,
                new_region = %expected_region,
                "updating note region to match folder path"
            );
            parsed.fm.region = expected_region;
            changed = true;
        }

        let expected_updated = file_modified_utc(path)?;
        if parsed.fm.updated != expected_updated {
            tracing::info!(
                path = %path.display(),
                old_updated = %parsed.fm.updated.to_rfc3339(),
                new_updated = %expected_updated.to_rfc3339(),
                "updating note timestamp to match file mtime"
            );
            parsed.fm.updated = expected_updated;
            changed = true;
        }

        if self.refs_sync_mode == RefsSyncMode::SyncFromWikilinks
            && parsed.fm.refs != parsed.wikilinks
        {
            tracing::info!(
                path = %path.display(),
                old_refs = ?parsed.fm.refs,
                new_refs = ?parsed.wikilinks,
                "updating frontmatter refs to match detected wikilinks"
            );
            parsed.fm.refs = parsed.wikilinks.clone();
            changed = true;
        }

        if !changed {
            return Ok(parsed);
        }
        note::rewrite_with_frontmatter(path, &parsed.fm, &parsed.body)?;
        self.parse_note_for_indexing(path)
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

fn file_modified_utc(path: &std::path::Path) -> Result<DateTime<Utc>> {
    let modified = fs::metadata(path)?.modified()?;
    let modified = DateTime::<Utc>::from(modified);
    Ok(note::truncate_datetime_to_seconds(modified))
}

impl<'a> Indexer<'a> {
    fn prune_missing_notes(&self, seen_ids: &std::collections::HashSet<String>) -> Result<usize> {
        let mut pruned = 0usize;
        for id in self.index.all_ids()? {
            if seen_ids.contains(&id) {
                continue;
            }
            let Some(row) = self.index.get_note(&id)? else {
                continue;
            };
            let resolved_path = resolve_indexed_path(self.vault.root(), &row.path);
            if resolved_path.exists() {
                continue;
            }
            self.index.delete_note(&id)?;
            self.vector_index
                .lock()
                .map_err(|_| anyhow::anyhow!("vector index mutex poisoned"))?
                .delete(&id)?;
            pruned += 1;
        }
        Ok(pruned)
    }

    fn conflicting_path_for_note_id(
        &self,
        path: &std::path::Path,
        id: &str,
    ) -> Result<Option<PathBuf>> {
        let Some(row) = self.index.get_note(id)? else {
            return Ok(None);
        };
        let existing_path = resolve_indexed_path(self.vault.root(), &row.path);
        if existing_path == path {
            return Ok(None);
        }
        if !existing_path.exists() {
            tracing::info!(
                note_id = %id,
                stale_path = %existing_path.display(),
                "dropping stale index row before reusing note id"
            );
            self.index.delete_note(id)?;
            self.vector_index
                .lock()
                .map_err(|_| anyhow::anyhow!("vector index mutex poisoned"))?
                .delete(id)?;
            return Ok(None);
        }
        Ok(Some(existing_path))
    }

    fn next_available_note_id(&self, base_id: &str) -> Result<String> {
        let mut suffix = 2usize;
        loop {
            let candidate = format!("{base_id}-{suffix}");
            if self.index.get_note(&candidate)?.is_none() {
                return Ok(candidate);
            }
            suffix += 1;
        }
    }
}

fn resolve_indexed_path(vault_root: &std::path::Path, indexed_path: &str) -> PathBuf {
    let raw = PathBuf::from(indexed_path);
    if raw.is_absolute() || raw.exists() {
        raw
    } else {
        vault_root.join(raw)
    }
}
