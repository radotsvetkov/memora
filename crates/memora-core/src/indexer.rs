use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use futures::stream::{self, StreamExt};

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

enum RebuildPathOutcome {
    ParseFail(anyhow::Error),
    AlignFail(anyhow::Error),
    UpsertOk(String),
    UpsertFail(String, anyhow::Error),
}

pub struct Indexer<'a> {
    pub vault: &'a Vault,
    pub index: &'a Index,
    pub embedder: Arc<dyn Embedder>,
    pub vector_index: Arc<Mutex<VectorIndex>>,
    pub claim_extractor: Option<ClaimExtractor>,
    pub extract_reference_notes: bool,
    pub frontmatter_fix_mode: FrontmatterFixMode,
    pub refs_sync_mode: RefsSyncMode,
    pub parallel_notes: usize,
    pub skip_contradiction_detection: bool,
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
            extract_reference_notes: false,
            frontmatter_fix_mode: FrontmatterFixMode::Strict,
            refs_sync_mode: RefsSyncMode::SyncFromWikilinks,
            parallel_notes: 8,
            skip_contradiction_detection: false,
        }
    }

    pub fn with_claims(mut self, claim_extractor: ClaimExtractor) -> Self {
        self.claim_extractor = Some(claim_extractor);
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

    pub fn with_parallel_notes(mut self, n: usize) -> Self {
        self.parallel_notes = n.max(1);
        self
    }

    pub fn with_skip_contradiction_detection(mut self, skip: bool) -> Self {
        self.skip_contradiction_detection = skip;
        self
    }

    pub async fn full_rebuild(&self) -> Result<RebuildStats> {
        let parallel = self.parallel_notes.max(1);
        let vault = self.vault.clone();
        let index = self.index.clone();
        let embedder = self.embedder.clone();
        let vector_index = self.vector_index.clone();
        let claim_extractor = self.claim_extractor.clone();
        let extract_reference_notes = self.extract_reference_notes;
        let frontmatter_fix_mode = self.frontmatter_fix_mode;
        let refs_sync_mode = self.refs_sync_mode;
        let skip_contradiction_detection = self.skip_contradiction_detection;

        let paths: Vec<PathBuf> = scan(vault.root()).collect();

        let results: Vec<(PathBuf, RebuildPathOutcome)> = stream::iter(paths)
            .map(|path| {
                let vault = vault.clone();
                let index = index.clone();
                let embedder = embedder.clone();
                let vector_index = vector_index.clone();
                let claim_extractor = claim_extractor.clone();

                async move {
                    let parsed = match parse_note_for_indexing_shared(
                        &vault,
                        frontmatter_fix_mode,
                        path.as_path(),
                    ) {
                        Ok(v) => v,
                        Err(err) => {
                            return (
                                path,
                                RebuildPathOutcome::ParseFail(err.context("parse note")),
                            );
                        }
                    };

                    let parsed = match align_frontmatter_to_filesystem_shared(
                        &vault,
                        &index,
                        &vector_index,
                        refs_sync_mode,
                        frontmatter_fix_mode,
                        path.as_path(),
                        parsed,
                    ) {
                        Ok(v) => v,
                        Err(err) => {
                            return (
                                path,
                                RebuildPathOutcome::AlignFail(err.context("align note")),
                            );
                        }
                    };

                    let id = parsed.fm.id.clone();
                    match upsert_note_inner(
                        &index,
                        embedder,
                        vector_index,
                        claim_extractor.as_ref(),
                        extract_reference_notes,
                        skip_contradiction_detection,
                        &parsed,
                    )
                    .await
                    {
                        Ok(()) => {
                            tracing::debug!(
                                path = %path.display(),
                                indexing_parallelism = parallel,
                                "parallel indexing upsert complete"
                            );
                            (path, RebuildPathOutcome::UpsertOk(id))
                        }
                        Err(err) => (path, RebuildPathOutcome::UpsertFail(id, err)),
                    }
                }
            })
            .buffer_unordered(parallel)
            .collect()
            .await;

        let mut stats = RebuildStats::default();
        let mut seen_ids = std::collections::HashSet::new();

        for (path, outcome) in results {
            match outcome {
                RebuildPathOutcome::UpsertOk(id) => {
                    seen_ids.insert(id);
                    stats.inserted += 1;
                }
                RebuildPathOutcome::UpsertFail(id, err) => {
                    seen_ids.insert(id);
                    stats.errors += 1;
                    let error_chain = err.chain().map(ToString::to_string).collect::<Vec<_>>();
                    tracing::warn!(
                        path = %path.display(),
                        error = %err,
                        error_chain = ?error_chain,
                        "failed to index note"
                    );
                }
                RebuildPathOutcome::AlignFail(err) => {
                    let error_chain = err.chain().map(ToString::to_string).collect::<Vec<_>>();
                    stats.skipped += 1;
                    stats.errors += 1;
                    tracing::warn!(
                        path = %path.display(),
                        error = %err,
                        error_chain = ?error_chain,
                        "failed to align note region during rebuild"
                    );
                }
                RebuildPathOutcome::ParseFail(err) => {
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

        let pruned =
            prune_missing_notes_shared(self.vault, self.index, &self.vector_index, &seen_ids)?;
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
        path: &Path,
        parsed: crate::note::Note,
    ) -> Result<crate::note::Note> {
        align_frontmatter_to_filesystem_shared(
            self.vault,
            self.index,
            &self.vector_index,
            self.refs_sync_mode,
            self.frontmatter_fix_mode,
            path,
            parsed,
        )
    }

    fn parse_note_for_indexing(&self, path: &Path) -> Result<crate::note::Note> {
        parse_note_for_indexing_shared(self.vault, self.frontmatter_fix_mode, path)
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
        upsert_note_inner(
            self.index,
            self.embedder.clone(),
            Arc::clone(&self.vector_index),
            self.claim_extractor.as_ref(),
            self.extract_reference_notes,
            self.skip_contradiction_detection,
            parsed,
        )
        .await
    }
}

fn parse_note_for_indexing_shared(
    vault: &Vault,
    mode: FrontmatterFixMode,
    path: &Path,
) -> Result<crate::note::Note> {
    match mode {
        FrontmatterFixMode::Strict => note::parse(path).map_err(Into::into),
        FrontmatterFixMode::RewriteMissing => {
            let (note, action) = note::parse_or_infer(path, vault.root())?;
            if action == FrontmatterAction::InferredAndRewritten {
                tracing::info!("rewrote frontmatter for {}", path.display());
            }
            Ok(note)
        }
        FrontmatterFixMode::InferInMemoryOnly => {
            let (note, action) = note::parse_or_infer_in_memory(path, vault.root())?;
            if action == FrontmatterAction::InferredInMemoryOnly {
                tracing::info!(path = %path.display(), "inferred frontmatter in-memory only");
            }
            Ok(note)
        }
    }
}

async fn upsert_note_inner(
    index: &Index,
    embedder: Arc<dyn Embedder>,
    vector_index: Arc<Mutex<VectorIndex>>,
    claim_extractor: Option<&ClaimExtractor>,
    extract_reference_notes: bool,
    skip_contradiction_detection: bool,
    parsed: &crate::note::Note,
) -> Result<()> {
    let claim_store = ClaimStore::new(index);
    let mut old_claim_ids = Vec::new();
    if let Some(_extractor) = claim_extractor {
        let should_extract = extract_reference_notes || parsed.fm.source != NoteSource::Reference;
        if should_extract {
            old_claim_ids = claim_store.claim_ids_for_note(&parsed.fm.id)?;
        }
    }

    index.upsert_note(parsed, &parsed.body)?;

    if let Some(extractor) = claim_extractor {
        let should_extract = extract_reference_notes || parsed.fm.source != NoteSource::Reference;
        if should_extract {
            let claims = extractor.extract(parsed, &parsed.body).await?;
            index.with_transaction(|tx| {
                claim_store.delete_for_note_in_tx(tx, &parsed.fm.id)?;
                for claim in &claims {
                    claim_store.upsert_in_tx(tx, claim)?;
                }
                Ok(())
            })?;

            let provenance = Provenance::new(index);
            let stale_tracker = StalenessTracker::new(index, &provenance);
            stale_tracker.mark_source_edited_claims(&old_claim_ids)?;

            if !skip_contradiction_detection {
                let contradiction_detector = ContradictionDetector {
                    store: &claim_store,
                    stale: &stale_tracker,
                    llm: Arc::clone(&extractor.llm),
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
    }

    let text = make_embedding_text(&parsed.fm.summary, &parsed.body);
    let vectors = embedder.embed(&[text]).await?;
    let vector = vectors
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("embedder returned no vector for note"))?;
    vector_index
        .lock()
        .map_err(|_| anyhow::anyhow!("vector index mutex poisoned"))?
        .upsert(&parsed.fm.id, &vector)?;
    Ok(())
}

fn align_frontmatter_to_filesystem_shared(
    vault: &Vault,
    index: &Index,
    vector_index: &Arc<Mutex<VectorIndex>>,
    refs_sync_mode: RefsSyncMode,
    frontmatter_fix_mode: FrontmatterFixMode,
    path: &Path,
    mut parsed: crate::note::Note,
) -> Result<crate::note::Note> {
    let mut changed = false;

    if let Some(conflicting_path) =
        conflicting_path_for_note_id_shared(vault, index, vector_index, path, &parsed.fm.id)?
    {
        let new_id = next_available_note_id_shared(index, &parsed.fm.id)?;
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

    let expected_region = note::derive_region_from_path(path, vault.root());
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

    if refs_sync_mode == RefsSyncMode::SyncFromWikilinks && parsed.fm.refs != parsed.wikilinks {
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
    parse_note_for_indexing_shared(vault, frontmatter_fix_mode, path)
}

fn make_embedding_text(summary: &str, body: &str) -> String {
    let body_head: String = body.chars().take(2_000).collect();
    normalize_text(&format!("{summary}\n{body_head}"))
}

fn file_modified_utc(path: &Path) -> Result<DateTime<Utc>> {
    let modified = fs::metadata(path)?.modified()?;
    let modified = DateTime::<Utc>::from(modified);
    Ok(note::truncate_datetime_to_seconds(modified))
}

fn prune_missing_notes_shared(
    vault: &Vault,
    index: &Index,
    vector_index: &Arc<Mutex<VectorIndex>>,
    seen_ids: &std::collections::HashSet<String>,
) -> Result<usize> {
    let mut pruned = 0usize;
    for id in index.all_ids()? {
        if seen_ids.contains(&id) {
            continue;
        }
        let Some(row) = index.get_note(&id)? else {
            continue;
        };
        let resolved_path = resolve_indexed_path(vault.root(), &row.path);
        if resolved_path.exists() {
            continue;
        }
        index.delete_note(&id)?;
        vector_index
            .lock()
            .map_err(|_| anyhow::anyhow!("vector index mutex poisoned"))?
            .delete(&id)?;
        pruned += 1;
    }
    Ok(pruned)
}

fn conflicting_path_for_note_id_shared(
    vault: &Vault,
    index: &Index,
    vector_index: &Arc<Mutex<VectorIndex>>,
    path: &Path,
    id: &str,
) -> Result<Option<PathBuf>> {
    let Some(row) = index.get_note(id)? else {
        return Ok(None);
    };
    let existing_path = resolve_indexed_path(vault.root(), &row.path);
    if existing_path == path {
        return Ok(None);
    }
    if !existing_path.exists() {
        tracing::info!(
            note_id = %id,
            stale_path = %existing_path.display(),
            "dropping stale index row before reusing note id"
        );
        index.delete_note(id)?;
        vector_index
            .lock()
            .map_err(|_| anyhow::anyhow!("vector index mutex poisoned"))?
            .delete(id)?;
        return Ok(None);
    }
    Ok(Some(existing_path))
}

fn next_available_note_id_shared(index: &Index, base_id: &str) -> Result<String> {
    let mut suffix = 2usize;
    loop {
        let candidate = format!("{base_id}-{suffix}");
        if index.get_note(&candidate)?.is_none() {
            return Ok(candidate);
        }
        suffix += 1;
    }
}

fn resolve_indexed_path(vault_root: &Path, indexed_path: &str) -> PathBuf {
    let raw = PathBuf::from(indexed_path);
    if raw.is_absolute() || raw.exists() {
        raw
    } else {
        vault_root.join(raw)
    }
}
