use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use chrono::{TimeZone, Utc};
use memora_core::claims::{Claim, ClaimStore};
use memora_core::note::{Frontmatter, Note, NoteSource, Privacy};
use memora_core::Index;
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
        object: "HMC".to_string(),
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
