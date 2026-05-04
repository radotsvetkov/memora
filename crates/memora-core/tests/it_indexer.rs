use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use memora_core::claims::ClaimExtractor;
use memora_core::indexer::{FrontmatterFixMode, Indexer};
use memora_core::{note, Embedder, Index, Vault, VaultEvent, VectorIndex};
use memora_llm::{CompletionRequest, CompletionResponse, LlmClient, LlmDestination, LlmError};
use tempfile::tempdir;

fn write_note(path: &Path, id: &str, summary: &str, tags: &[&str], body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tags_line = tags
        .iter()
        .map(|tag| format!("\"{tag}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let content = format!(
        r#"---
id: {id}
region: test/integration
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-02T00:00:00Z
summary: "{summary}"
tags: [{tags_line}]
refs: []
---
{body}
"#
    );
    fs::write(path, content)?;
    Ok(())
}

fn setup_vault(root: &Path) -> Result<Vec<PathBuf>> {
    let paths = vec![
        root.join("alpha.md"),
        root.join("work/beta.md"),
        root.join("work/gamma.md"),
        root.join("personal/delta.md"),
        root.join("personal/epsilon.md"),
    ];

    write_note(
        &paths[0],
        "note-alpha",
        "Alpha summary about astronomy",
        &["alpha", "space"],
        "alpha body with comet nebula and star chart",
    )?;
    write_note(
        &paths[1],
        "note-beta",
        "Beta summary",
        &["beta"],
        "beta body references alpha concepts",
    )?;
    write_note(
        &paths[2],
        "note-gamma",
        "Gamma summary",
        &["gamma"],
        "gamma body and project notes",
    )?;
    write_note(
        &paths[3],
        "note-delta",
        "Delta summary",
        &["delta"],
        "delta body with routine updates",
    )?;
    write_note(
        &paths[4],
        "note-epsilon",
        "Epsilon summary with comet focus",
        &["epsilon", "space"],
        "comet comet comet trajectory and observations",
    )?;

    Ok(paths)
}

struct DeterministicEmbedder {
    dim: usize,
    model_id: String,
}

impl DeterministicEmbedder {
    fn new(dim: usize) -> Self {
        Self {
            dim,
            model_id: "test/deterministic".to_string(),
        }
    }
}

#[async_trait]
impl Embedder for DeterministicEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for text in texts {
            let mut vec = Vec::with_capacity(self.dim);
            let mut seed = blake3::hash(text.as_bytes()).as_bytes().to_vec();
            while seed.len() < self.dim * 4 {
                let next = blake3::hash(&seed).as_bytes().to_vec();
                seed.extend_from_slice(&next);
            }
            for chunk in seed.chunks_exact(4).take(self.dim) {
                let bits = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let normalized = (bits as f32 / u32::MAX as f32) * 2.0 - 1.0;
                vec.push(normalized);
            }
            out.push(vec);
        }
        Ok(out)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

struct RateLimitedExtractorLlm;

#[async_trait]
impl LlmClient for RateLimitedExtractorLlm {
    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Err(LlmError::RateLimited)
    }

    fn model_name(&self) -> &str {
        "test/rate-limited"
    }

    fn destination(&self) -> LlmDestination {
        LlmDestination::CloudKnown
    }
}

#[tokio::test]
async fn full_rebuild_and_reindex_flow() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    let note_paths = setup_vault(&vault_root)?;

    let index_path = temp.path().join("index").join("memora.db");
    let index = Index::open(&index_path)?;
    let vault = Vault::new(&vault_root);
    let embedder: Arc<dyn Embedder> = Arc::new(DeterministicEmbedder::new(64));
    let vector_index = Arc::new(Mutex::new(VectorIndex::open_or_create(
        &temp.path().join("index").join("vectors"),
        embedder.dim(),
    )?));
    let indexer = Indexer::new(&vault, &index, embedder, vector_index);

    let stats = indexer.full_rebuild().await?;
    assert_eq!(stats.inserted, 5);
    assert_eq!(stats.errors, 0);

    let before = index
        .get_note("note-epsilon")?
        .expect("epsilon should be indexed before update");
    let updated_content = r#"---
id: note-epsilon
region: test/integration
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-03T00:00:00Z
summary: "Epsilon summary with comet focus"
tags: ["epsilon", "space"]
refs: []
---
comet comet comet comet trajectory and observations updated
"#;
    fs::write(&note_paths[4], updated_content)?;

    let reparsed = note::parse(&note_paths[4])?;
    index.upsert_note(&reparsed, &reparsed.body)?;

    let after = index
        .get_note("note-epsilon")?
        .expect("epsilon should be indexed after update");
    assert_ne!(before.body_hash, after.body_hash);

    let results = index.bm25_search("comet", 5)?;
    assert!(!results.is_empty());
    assert_eq!(results[0].0, "note-epsilon");

    Ok(())
}

#[tokio::test]
async fn it_indexer_auto_fixes_real_obsidian_style_vault() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    fs::create_dir_all(vault_root.join("journal"))?;
    fs::create_dir_all(vault_root.join("projects"))?;

    let fixture_notes = vec![
        (
            vault_root.join("Daily Note.md"),
            "Daily planning kickoff\n- review backlog\n",
        ),
        (
            vault_root.join("Project Idea.md"),
            "Build a lightweight claim graph cache\n",
        ),
        (
            vault_root.join("2026-04-30.md"),
            "Thursday retrospective\nWins and blockers\n",
        ),
        (
            vault_root.join("journal/Morning.md"),
            "Morning reflection\nHydrate and stretch\n",
        ),
        (
            vault_root.join("projects/Launch Checklist.md"),
            "Ship checklist draft\n",
        ),
    ];
    for (path, body) in &fixture_notes {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, body)?;
    }

    let index_path = temp.path().join("index").join("memora.db");
    let index = Index::open(&index_path)?;
    let vault = Vault::new(&vault_root);
    let embedder: Arc<dyn Embedder> = Arc::new(DeterministicEmbedder::new(64));
    let vector_index = Arc::new(Mutex::new(VectorIndex::open_or_create(
        &temp.path().join("index").join("vectors"),
        embedder.dim(),
    )?));
    let indexer = Indexer::new(&vault, &index, embedder, vector_index)
        .with_frontmatter_fix_mode(FrontmatterFixMode::RewriteMissing);

    let stats = indexer.full_rebuild().await?;
    assert_eq!(stats.inserted, 5);
    assert_eq!(stats.errors, 0);

    for (path, original_body) in fixture_notes {
        let file = fs::read_to_string(&path)?;
        assert!(file.starts_with("---\n"));
        assert!(file.ends_with(&original_body));
    }

    Ok(())
}

#[tokio::test]
async fn full_rebuild_updates_region_after_file_move() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    fs::create_dir_all(&vault_root)?;

    let moved_note = vault_root.join("move-me.md");
    fs::write(
        &moved_note,
        r#"---
id: move-me
region: default
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Move me"
tags: []
refs: []
---
Body text for move.
"#,
    )?;

    let index_path = temp.path().join("index").join("memora.db");
    let index = Index::open(&index_path)?;
    let vault = Vault::new(&vault_root);
    let embedder: Arc<dyn Embedder> = Arc::new(DeterministicEmbedder::new(64));
    let vector_index = Arc::new(Mutex::new(VectorIndex::open_or_create(
        &temp.path().join("index").join("vectors"),
        embedder.dim(),
    )?));
    let indexer = Indexer::new(&vault, &index, embedder, vector_index);

    indexer.full_rebuild().await?;
    assert_eq!(
        index
            .get_note("move-me")?
            .expect("note should be indexed")
            .region,
        "default"
    );

    let moved_path = vault_root.join("work").join("move-me.md");
    fs::create_dir_all(moved_path.parent().expect("moved note should have parent"))?;
    fs::rename(&moved_note, &moved_path)?;

    indexer.full_rebuild().await?;

    let reparsed = note::parse(&moved_path)?;
    assert_eq!(reparsed.fm.region, "work");
    assert_eq!(
        index
            .get_note("move-me")?
            .expect("moved note should remain indexed")
            .region,
        "work"
    );

    Ok(())
}

#[tokio::test]
async fn modified_event_updates_frontmatter_updated_timestamp() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    fs::create_dir_all(&vault_root)?;
    let note_path = vault_root.join("update-me.md");
    fs::write(
        &note_path,
        r#"---
id: update-me
region: default
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Update me"
tags: []
refs: []
---
Initial body text.
"#,
    )?;

    let index_path = temp.path().join("index").join("memora.db");
    let index = Index::open(&index_path)?;
    let vault = Vault::new(&vault_root);
    let embedder: Arc<dyn Embedder> = Arc::new(DeterministicEmbedder::new(64));
    let vector_index = Arc::new(Mutex::new(VectorIndex::open_or_create(
        &temp.path().join("index").join("vectors"),
        embedder.dim(),
    )?));
    let indexer = Indexer::new(&vault, &index, embedder, vector_index);
    indexer
        .handle_event(VaultEvent::Created(note_path.clone()))
        .await?;

    let before = note::parse(&note_path)?.fm.updated;
    std::thread::sleep(Duration::from_secs(1));
    fs::write(
        &note_path,
        r#"---
id: update-me
region: default
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Update me"
tags: []
refs: []
---
Initial body text with new content.
"#,
    )?;
    indexer
        .handle_event(VaultEvent::Modified(note_path.clone()))
        .await?;

    let reparsed = note::parse(&note_path)?;
    let after = reparsed.fm.updated;
    assert!(after > before);
    assert!(!after.to_rfc3339().contains('.'));

    let db_updated = index
        .get_note("update-me")?
        .expect("note should exist in index")
        .updated;
    let db_updated_dt = DateTime::parse_from_rfc3339(&db_updated)?.with_timezone(&Utc);
    assert_eq!(db_updated_dt, after);

    Ok(())
}

#[tokio::test]
async fn modified_event_syncs_frontmatter_refs_from_wikilinks() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    fs::create_dir_all(&vault_root)?;
    let target_path = vault_root.join("target-note.md");
    fs::write(
        &target_path,
        r#"---
id: target-note
region: default
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Target"
tags: []
refs: []
---
Target body.
"#,
    )?;

    let note_path = vault_root.join("source-note.md");
    fs::write(
        &note_path,
        r#"---
id: source-note
region: default
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Source"
tags: []
refs: []
---
No links yet.
"#,
    )?;

    let index_path = temp.path().join("index").join("memora.db");
    let index = Index::open(&index_path)?;
    let vault = Vault::new(&vault_root);
    let embedder: Arc<dyn Embedder> = Arc::new(DeterministicEmbedder::new(64));
    let vector_index = Arc::new(Mutex::new(VectorIndex::open_or_create(
        &temp.path().join("index").join("vectors"),
        embedder.dim(),
    )?));
    let indexer = Indexer::new(&vault, &index, embedder, vector_index);

    indexer
        .handle_event(VaultEvent::Created(target_path.clone()))
        .await?;
    indexer
        .handle_event(VaultEvent::Created(note_path.clone()))
        .await?;

    std::thread::sleep(Duration::from_secs(1));
    fs::write(
        &note_path,
        r#"---
id: source-note
region: default
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Source"
tags: []
refs: []
---
Now linking to [[target-note]].
"#,
    )?;
    indexer
        .handle_event(VaultEvent::Modified(note_path.clone()))
        .await?;

    let reparsed = note::parse(&note_path)?;
    assert_eq!(reparsed.fm.refs, vec!["target-note".to_string()]);

    Ok(())
}

#[tokio::test]
async fn full_rebuild_normalizes_invalid_source_and_privacy() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    fs::create_dir_all(&vault_root)?;
    let note_path = vault_root.join("invalid-meta.md");
    fs::write(
        &note_path,
        r#"---
id: invalid-meta
region: default
source: unknown
privacy: ultra-secret
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Invalid enums"
tags: []
refs: []
---
Body.
"#,
    )?;

    let index_path = temp.path().join("index").join("memora.db");
    let index = Index::open(&index_path)?;
    let vault = Vault::new(&vault_root);
    let embedder: Arc<dyn Embedder> = Arc::new(DeterministicEmbedder::new(64));
    let vector_index = Arc::new(Mutex::new(VectorIndex::open_or_create(
        &temp.path().join("index").join("vectors"),
        embedder.dim(),
    )?));
    let indexer = Indexer::new(&vault, &index, embedder, vector_index)
        .with_frontmatter_fix_mode(FrontmatterFixMode::RewriteMissing);

    let stats = indexer.full_rebuild().await?;
    assert_eq!(stats.errors, 0);

    let reparsed = note::parse(&note_path)?;
    assert_eq!(reparsed.fm.source.to_string(), "personal");
    assert_eq!(reparsed.fm.privacy.to_string(), "private");
    assert!(index.get_note("invalid-meta")?.is_some());

    Ok(())
}

#[tokio::test]
async fn full_rebuild_prunes_deleted_notes() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    fs::create_dir_all(&vault_root)?;

    let note_path = vault_root.join("prune-me.md");
    fs::write(
        &note_path,
        r#"---
id: prune-me
region: default
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Prune me"
tags: []
refs: []
---
Body.
"#,
    )?;

    let index_path = temp.path().join("index").join("memora.db");
    let index = Index::open(&index_path)?;
    let vault = Vault::new(&vault_root);
    let embedder: Arc<dyn Embedder> = Arc::new(DeterministicEmbedder::new(64));
    let vector_index = Arc::new(Mutex::new(VectorIndex::open_or_create(
        &temp.path().join("index").join("vectors"),
        embedder.dim(),
    )?));
    let indexer = Indexer::new(&vault, &index, embedder, vector_index);

    indexer.full_rebuild().await?;
    assert!(index.get_note("prune-me")?.is_some());

    fs::remove_file(&note_path)?;
    indexer.full_rebuild().await?;

    assert!(index.get_note("prune-me")?.is_none());
    assert!(!index.all_ids()?.iter().any(|id| id == "prune-me"));

    Ok(())
}

#[tokio::test]
async fn full_rebuild_rewrites_duplicate_note_ids_to_unique_values() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    fs::create_dir_all(vault_root.join("a"))?;
    fs::create_dir_all(vault_root.join("b"))?;

    let first = vault_root.join("a/first.md");
    let second = vault_root.join("b/second.md");
    let duplicate_id_frontmatter = |body: &str| {
        format!(
            r#"---
id: dup-note
region: default
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Duplicate id"
tags: []
refs: []
---
{body}
"#
        )
    };
    fs::write(&first, duplicate_id_frontmatter("first body"))?;
    fs::write(&second, duplicate_id_frontmatter("second body"))?;

    let index_path = temp.path().join("index").join("memora.db");
    let index = Index::open(&index_path)?;
    let vault = Vault::new(&vault_root);
    let embedder: Arc<dyn Embedder> = Arc::new(DeterministicEmbedder::new(64));
    let vector_index = Arc::new(Mutex::new(VectorIndex::open_or_create(
        &temp.path().join("index").join("vectors"),
        embedder.dim(),
    )?));
    let indexer = Indexer::new(&vault, &index, embedder, vector_index)
        .with_frontmatter_fix_mode(FrontmatterFixMode::RewriteMissing);

    indexer.full_rebuild().await?;

    let first_note = note::parse(&first)?;
    let second_note = note::parse(&second)?;
    let ids = [first_note.fm.id.as_str(), second_note.fm.id.as_str()];
    assert!(ids.contains(&"dup-note"));
    assert!(ids.contains(&"dup-note-2"));
    assert!(index.get_note("dup-note")?.is_some());
    assert!(index.get_note("dup-note-2")?.is_some());

    Ok(())
}

#[tokio::test]
async fn parallel_indexing_processes_all_notes() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    fs::create_dir_all(&vault_root)?;
    for i in 0..20 {
        let path = vault_root.join(format!("n{i}.md"));
        write_note(
            &path,
            &format!("note-{i}"),
            &format!("summary {i}"),
            &[],
            &format!("Body content for note {i} with enough characters for indexing."),
        )?;
    }

    let index_path = temp.path().join("index").join("memora.db");
    let index = Index::open(&index_path)?;
    let vault = Vault::new(&vault_root);
    let embedder: Arc<dyn Embedder> = Arc::new(DeterministicEmbedder::new(64));
    let vector_index = Arc::new(Mutex::new(VectorIndex::open_or_create(
        &temp.path().join("index").join("vectors"),
        embedder.dim(),
    )?));
    let indexer = Indexer::new(&vault, &index, embedder, vector_index).with_parallel_notes(4);

    let stats = indexer.full_rebuild().await?;
    assert_eq!(stats.inserted, 20);
    assert_eq!(stats.errors, 0);
    assert_eq!(index.all_ids()?.len(), 20);

    Ok(())
}

#[tokio::test]
async fn full_rebuild_counts_rate_limited_extractions_as_errors() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    fs::create_dir_all(&vault_root)?;
    let note_path = vault_root.join("rate-limited.md");
    write_note(
        &note_path,
        "rate-limited-note",
        "Rate limited summary",
        &["rate-limit"],
        "akmon decided to switch to stainless templates for generation.",
    )?;

    let index_path = temp.path().join("index").join("memora.db");
    let index = Index::open(&index_path)?;
    let vault = Vault::new(&vault_root);
    let embedder: Arc<dyn Embedder> = Arc::new(DeterministicEmbedder::new(64));
    let vector_index = Arc::new(Mutex::new(VectorIndex::open_or_create(
        &temp.path().join("index").join("vectors"),
        embedder.dim(),
    )?));
    let claim_extractor = ClaimExtractor {
        llm: Arc::new(RateLimitedExtractorLlm),
        model_label: "test/rate-limited".to_string(),
    };
    let indexer = Indexer::new(&vault, &index, embedder, vector_index).with_claims(claim_extractor);

    let stats = indexer.full_rebuild().await?;
    assert_eq!(stats.inserted, 1);
    assert_eq!(stats.claims_extracted, 0);
    assert_eq!(stats.error_rate_limited, 1);
    assert_eq!(stats.error_parse, 0);
    assert_eq!(stats.error_invalid, 0);
    assert_eq!(stats.total_extraction_errors(), 1);
    assert_eq!(stats.errors, 1);

    Ok(())
}
