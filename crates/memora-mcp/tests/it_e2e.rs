use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use memora_core::claims::{Claim, ClaimStore};
use memora_core::indexer::Indexer;
use memora_core::note::{Frontmatter, Note, NoteSource, Privacy};
use memora_core::{Embedder, Index, Vault, VaultEvent, VectorIndex};
use memora_mcp::tools::MemoraRuntime;
use tempfile::tempdir;

fn seed_note(vault: &std::path::Path, index: &Index) -> Result<String> {
    let now = Utc
        .with_ymd_and_hms(2026, 4, 1, 0, 0, 0)
        .single()
        .ok_or_else(|| anyhow::anyhow!("invalid datetime"))?;
    let rel = PathBuf::from("sample").join("seed.md");
    let note = Note {
        path: rel.clone(),
        fm: Frontmatter {
            id: "seed-note".to_string(),
            region: "sample".to_string(),
            source: NoteSource::Personal,
            privacy: Privacy::Private,
            created: now,
            updated: now,
            summary: "Seed note".to_string(),
            tags: vec!["seed".to_string()],
            refs: vec![],
        },
        body: "Rado works at HMC.".to_string(),
        wikilinks: vec![],
    };
    let abs = vault.join(&rel);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&abs, memora_core::note::render(&note))?;
    index.upsert_note(&note, &note.body)?;
    let claim = Claim {
        id: "aaaaaaaaaaaaaaaa".to_string(),
        subject: "Rado".to_string(),
        predicate: "works_at".to_string(),
        object: Some("HMC".to_string()),
        note_id: note.fm.id.clone(),
        span_start: 0,
        span_end: "Rado works at HMC".len(),
        span_fingerprint: Claim::compute_fingerprint("Rado works at HMC"),
        valid_from: now,
        valid_until: None,
        confidence: 1.0,
        privacy: Privacy::Private,
        extracted_by: "test".to_string(),
        extracted_at: now,
    };
    ClaimStore::new(index).upsert(&claim)?;
    Ok(note.fm.id)
}

struct TestEmbedder;

#[async_trait]
impl Embedder for TestEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![0.25; 64]).collect())
    }

    fn dim(&self) -> usize {
        64
    }

    fn model_id(&self) -> &str {
        "test/embedder"
    }
}

#[tokio::test]
async fn mcp_runtime_e2e_tools() -> Result<()> {
    let temp = tempdir()?;
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault)?;
    let index_db = vault.join(".memora").join("memora.db");
    let vector = vault.join(".memora").join("vectors");
    fs::create_dir_all(vault.join(".memora"))?;
    let index = Index::open(&index_db)?;
    let _seed_id = seed_note(&vault, &index)?;
    fs::write(vault.join("world_map.md"), "# World Map\n")?;

    let runtime = MemoraRuntime {
        vault_root: vault.clone(),
        index_db,
        vector_index: vector,
    };

    let cited = runtime
        .invoke_tool(
            "memora_query_cited",
            serde_json::json!({"query":"Rado work","k":5}),
        )
        .await?;
    let verified = cited
        .get("verified_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    assert!(verified >= 1);

    let capture = runtime
        .invoke_tool(
            "memora_capture",
            serde_json::json!({
                "region":"sample",
                "summary":"Captured",
                "body":"Captured body.",
                "tags":["capture"],
                "privacy":"private"
            }),
        )
        .await?;
    let captured_path = capture
        .get("path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing capture path"))?;
    assert!(vault.join(captured_path).exists());

    runtime
        .invoke_tool(
            "memora_consolidate",
            serde_json::json!({"scope":"region:sample"}),
        )
        .await?;
    let atlas = vault.join("sample").join("_atlas.md");
    assert!(atlas.exists());

    let challenge = runtime
        .invoke_tool("memora_challenge", serde_json::json!({}))
        .await?;
    assert!(challenge.get("stale_alerts").is_some());
    assert!(challenge.get("contradiction_alerts").is_some());
    assert!(challenge.get("cross_region_alerts").is_some());
    assert!(challenge.get("frontier_alerts").is_some());
    Ok(())
}

#[tokio::test]
async fn moved_note_updates_region_in_frontmatter_index_and_query() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    fs::create_dir_all(&vault_root)?;
    let index_db = vault_root.join(".memora").join("memora.db");
    let vector_index_path = vault_root.join(".memora").join("vectors");
    fs::create_dir_all(vault_root.join(".memora"))?;

    let index = Index::open(&index_db)?;
    let vault = Vault::new(&vault_root);
    let embedder: Arc<dyn Embedder> = Arc::new(TestEmbedder);
    let vector_index = Arc::new(Mutex::new(VectorIndex::open_or_create(
        &vector_index_path,
        embedder.dim(),
    )?));
    let indexer = Indexer::new(&vault, &index, embedder, vector_index);

    let root_note_path = vault_root.join("moved-note.md");
    let initial = r#"---
id: moved-note
region: default
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Move test note"
tags: []
refs: []
---
relocation-token appears in this note body.
"#;
    fs::write(&root_note_path, initial)?;
    indexer
        .handle_event(VaultEvent::Created(root_note_path.clone()))
        .await?;
    assert_eq!(
        index
            .get_note("moved-note")?
            .expect("note should be indexed")
            .region,
        "default"
    );

    let moved_path = vault_root.join("work").join("moved-note.md");
    fs::create_dir_all(moved_path.parent().expect("moved path should have parent"))?;
    fs::rename(&root_note_path, &moved_path)?;
    indexer
        .handle_event(VaultEvent::Renamed(moved_path.clone()))
        .await?;

    let reparsed = memora_core::note::parse(&moved_path)?;
    assert_eq!(reparsed.fm.region, "work");
    assert_eq!(
        index
            .get_note("moved-note")?
            .expect("moved note should be indexed")
            .region,
        "work"
    );

    let runtime = MemoraRuntime {
        vault_root: vault_root.clone(),
        index_db,
        vector_index: vector_index_path,
    };
    let query = runtime
        .invoke_tool(
            "memora_query",
            serde_json::json!({"query":"relocation-token", "k":5}),
        )
        .await?;
    let hits = query
        .get("hits")
        .and_then(serde_json::Value::as_array)
        .expect("hits should be an array");
    let moved_hit = hits
        .iter()
        .find(|hit| hit.get("id").and_then(serde_json::Value::as_str) == Some("moved-note"))
        .expect("moved note should be queryable");
    assert_eq!(
        moved_hit
            .get("region")
            .and_then(serde_json::Value::as_str)
            .expect("region should be present"),
        "work"
    );

    Ok(())
}
