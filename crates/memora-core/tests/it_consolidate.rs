use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use memora_core::claims::{Claim, ClaimStore};
use memora_core::consolidate::atlas::AtlasWriter;
use memora_core::consolidate::world_map::WorldMapWriter;
use memora_core::note::{self, Frontmatter, Note, NoteSource, Privacy};
use memora_core::Index;
use memora_llm::{CompletionRequest, CompletionResponse, LlmClient, LlmDestination, LlmError};
use rusqlite::params;
use tempfile::tempdir;

fn write_note(
    vault: &Path,
    region: &str,
    file_name: &str,
    id: &str,
    summary: &str,
) -> Result<PathBuf> {
    let region_dir = vault.join(region);
    fs::create_dir_all(&region_dir)?;
    let path = region_dir.join(file_name);
    let body = format!("Body for {id} in {region}.");
    let note = Note {
        path: path.clone(),
        fm: Frontmatter {
            id: id.to_string(),
            region: region.to_string(),
            source: NoteSource::Personal,
            privacy: Privacy::Private,
            created: Utc
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid datetime"),
            updated: Utc
                .with_ymd_and_hms(2026, 1, 2, 0, 0, 0)
                .single()
                .expect("valid datetime"),
            summary: summary.to_string(),
            tags: vec![],
            refs: vec![],
        },
        body,
        wikilinks: vec![],
    };
    fs::write(&path, note::render(&note))?;
    Ok(path)
}

fn make_claim(
    note_id: &str,
    subject: &str,
    predicate: &str,
    object: &str,
    privacy: Privacy,
) -> Claim {
    let valid_from = Utc
        .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
        .single()
        .expect("valid datetime");
    Claim {
        id: Claim::compute_id(subject, predicate, Some(object), note_id, 0),
        subject: subject.to_string(),
        predicate: predicate.to_string(),
        object: Some(object.to_string()),
        note_id: note_id.to_string(),
        span_start: 0,
        span_end: object.len(),
        span_fingerprint: Claim::compute_fingerprint(object),
        valid_from,
        valid_until: None,
        confidence: 0.9,
        privacy,
        extracted_by: "test/consolidate".to_string(),
        extracted_at: valid_from,
    }
}

struct MockConsolidateLlm {
    subregion_claim_ids: Vec<String>,
}

#[async_trait]
impl LlmClient for MockConsolidateLlm {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let system = req.system.unwrap_or_default();
        let user = req
            .messages
            .first()
            .map(|msg| msg.content.clone())
            .unwrap_or_default();
        let text = if system.contains("sub-regions for a region") {
            serde_json::json!({
                "proposed_subregions": [
                    {
                        "name": "Split A",
                        "sample_subjects": ["S0", "S1"],
                        "claim_ids": self.subregion_claim_ids,
                    }
                ]
            })
            .to_string()
        } else if user.contains("Write 200-300 words") {
            "Index narrative with markers [claim:abc] [claim:def] [claim:ghi]".to_string()
        } else {
            "Region overview paragraph generated for test coverage.".to_string()
        };
        Ok(CompletionResponse {
            text,
            model: "mock/consolidate".to_string(),
            input_tokens: None,
            output_tokens: None,
        })
    }

    fn model_name(&self) -> &str {
        "mock/consolidate"
    }

    fn destination(&self) -> LlmDestination {
        LlmDestination::Local
    }
}

#[tokio::test]
async fn consolidate_writes_atlas_index_idempotent_and_redacts_secret() -> Result<()> {
    let temp = tempdir()?;
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault)?;
    let db_path = temp.path().join("index").join("memora.db");
    let index = Index::open(&db_path)?;

    for i in 0..10 {
        let note_id = format!("ops-{i}");
        let path = write_note(
            &vault,
            "ops",
            &format!("{note_id}.md"),
            &note_id,
            "ops summary",
        )?;
        let parsed = note::parse(&path)?;
        index.upsert_note(&parsed, &parsed.body)?;
    }

    let store = ClaimStore::new(&index);
    let public_claim = make_claim("ops-0", "Team", "tracks", "latency", Privacy::Private);
    let secret_claim = make_claim("ops-1", "Budget", "is", "classified", Privacy::Secret);
    store.upsert(&public_claim)?;
    store.upsert(&secret_claim)?;
    // Atlas synthesis runs only when the region has at least five claims.
    store.upsert(&make_claim(
        "ops-2",
        "Infra",
        "runs_on",
        "k8s",
        Privacy::Private,
    ))?;
    store.upsert(&make_claim(
        "ops-3",
        "Alerts",
        "route_to",
        "pagerduty",
        Privacy::Private,
    ))?;
    store.upsert(&make_claim(
        "ops-4",
        "CI",
        "blocks",
        "merge",
        Privacy::Private,
    ))?;
    store.add_relation(
        &public_claim.id,
        &secret_claim.id,
        memora_core::ClaimRelation::Contradicts,
        1.0,
    )?;

    let conn = rusqlite::Connection::open(&db_path)?;
    conn.execute(
        "INSERT INTO decisions (id, claim_id, title, decided_on, decided_by, status)
         VALUES (?, ?, ?, ?, ?, ?)",
        params![
            "decision-1",
            public_claim.id,
            "Adopt query cache",
            "2026-01-03T00:00:00Z",
            "team",
            "accepted"
        ],
    )?;
    conn.execute(
        "INSERT INTO stale_claims (claim_id, reason, marked_at) VALUES (?, ?, ?)",
        params![secret_claim.id, "source_edited", "2026-01-04T00:00:00Z"],
    )?;

    let llm = MockConsolidateLlm {
        subregion_claim_ids: vec![],
    };
    let writer = AtlasWriter {
        db: &index,
        claim_store: &store,
        llm: &llm,
        vault: &vault,
    };
    writer.rebuild_region("ops").await?;

    let atlas_path = vault.join("ops").join("_atlas.md");
    let index_path = vault.join("ops").join("_index.md");
    assert!(atlas_path.exists());
    assert!(index_path.exists());

    let first_atlas = fs::read_to_string(&atlas_path)?;
    let first_index = fs::read_to_string(&index_path)?;
    assert!(first_atlas.contains("# Atlas: ops"));
    assert!(first_atlas.contains("## Subjects"));
    assert!(first_atlas.contains("## Recent decisions"));
    assert!(first_atlas.contains("## Stale dependencies"));
    assert!(first_atlas.contains("## Contradictions"));
    assert!(first_atlas.contains("[redacted] [redacted]"));
    assert!(first_index.contains("# Index: ops"));

    writer.rebuild_region("ops").await?;
    let second_atlas = fs::read_to_string(&atlas_path)?;
    let second_index = fs::read_to_string(&index_path)?;
    assert_eq!(first_atlas, second_atlas);
    assert_eq!(first_index, second_index);
    Ok(())
}

#[tokio::test]
async fn consolidate_triggers_subregion_split_for_large_regions() -> Result<()> {
    let temp = tempdir()?;
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault)?;
    let db_path = temp.path().join("index").join("memora.db");
    let index = Index::open(&db_path)?;
    let store = ClaimStore::new(&index);

    let mut selected_claim_ids = Vec::new();
    for note_idx in 0..25 {
        let note_id = format!("mega-{note_idx}");
        let path = write_note(
            &vault,
            "mega",
            &format!("{note_id}.md"),
            &note_id,
            "mega summary",
        )?;
        let parsed = note::parse(&path)?;
        index.upsert_note(&parsed, &parsed.body)?;
    }

    for claim_idx in 0..250 {
        let note_id = format!("mega-{}", claim_idx % 25);
        let claim = make_claim(
            &note_id,
            &format!("S{}", claim_idx % 10),
            "owns",
            &format!("O{claim_idx}"),
            Privacy::Private,
        );
        if claim_idx < 40 {
            selected_claim_ids.push(claim.id.clone());
        }
        store.upsert(&claim)?;
    }

    let llm = MockConsolidateLlm {
        subregion_claim_ids: selected_claim_ids,
    };
    let writer = AtlasWriter {
        db: &index,
        claim_store: &store,
        llm: &llm,
        vault: &vault,
    };
    writer.rebuild_region("mega").await?;

    let split_dir = vault.join("mega").join("split-a");
    assert!(split_dir.exists());

    let moved_count = fs::read_dir(&split_dir)?.count();
    assert!(moved_count >= 1);

    let moved_entry = fs::read_dir(&split_dir)?
        .next()
        .expect("at least one moved note")?;
    let moved_path = moved_entry.path();
    let moved_note = note::parse(&moved_path)?;
    assert!(moved_note.fm.region.starts_with("mega/split-a"));
    Ok(())
}

#[tokio::test]
async fn consolidate_surfaces_challenger_findings_in_atlas_and_world_map() -> Result<()> {
    let temp = tempdir()?;
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault)?;
    let db_path = temp.path().join("index").join("memora.db");
    let index = Index::open(&db_path)?;
    let store = ClaimStore::new(&index);

    let notes = [
        ("semantic/projects/akmon", "akmon-01"),
        ("semantic/projects/akmon", "akmon-02"),
        ("semantic/projects/csv-tool", "csv-01"),
        ("semantic/projects/csv-tool", "csv-02"),
        ("semantic/projects/memora", "memora-01"),
        ("semantic/projects/memora", "memora-02"),
        ("episodic", "ep-01"),
        ("episodic", "ep-02"),
    ];
    for (region, note_id) in notes {
        let path = write_note(
            &vault,
            region,
            &format!("{note_id}.md"),
            note_id,
            &format!("summary {note_id}"),
        )?;
        let parsed = note::parse(&path)?;
        index.upsert_note(&parsed, &parsed.body)?;
    }

    store.upsert(&make_claim(
        "akmon-01",
        "akmon",
        "uses_api_client_generator",
        "stainless-templates",
        Privacy::Private,
    ))?;
    store.upsert(&make_claim(
        "akmon-02",
        "akmon",
        "will_switch_default_to",
        "stainless-templates",
        Privacy::Private,
    ))?;
    store.upsert(&make_claim(
        "akmon-01",
        "akmon",
        "uses_worker_model",
        "single-process",
        Privacy::Private,
    ))?;
    store.upsert(&make_claim(
        "akmon-02",
        "akmon",
        "uses_architecture",
        "distributed-workers",
        Privacy::Private,
    ))?;

    store.upsert(&make_claim(
        "csv-01",
        "csv-tool",
        "uses_language",
        "rust",
        Privacy::Private,
    ))?;
    store.upsert(&make_claim(
        "ep-01",
        "csv-tool",
        "implemented_in",
        "python",
        Privacy::Private,
    ))?;

    store.upsert(&make_claim(
        "memora-01",
        "retrieval-eval-notes",
        "superseded_by",
        "retrieval-eval-notes-v2",
        Privacy::Private,
    ))?;
    store.upsert(&make_claim(
        "memora-02",
        "staleness-case-a-synthesis",
        "depends_on",
        "retrieval-eval-notes",
        Privacy::Private,
    ))?;
    store.upsert(&make_claim(
        "memora-01",
        "memora",
        "decision_pending",
        "precision-vs-recall-tradeoff",
        Privacy::Private,
    ))?;
    store.upsert(&make_claim(
        "ep-02",
        "memora",
        "design_question",
        "precision-vs-recall",
        Privacy::Private,
    ))?;

    let llm = MockConsolidateLlm {
        subregion_claim_ids: vec![],
    };
    let atlas = AtlasWriter {
        db: &index,
        claim_store: &store,
        llm: &llm,
        vault: &vault,
    };
    atlas.rebuild_region("semantic/projects/akmon").await?;
    atlas.rebuild_region("semantic/projects/csv-tool").await?;
    atlas.rebuild_region("semantic/projects/memora").await?;

    let world = WorldMapWriter {
        db: &index,
        claim_store: &store,
        llm: &llm,
        vault: &vault,
    };
    world.rebuild().await?;

    let akmon = fs::read_to_string(vault.join("semantic/projects/akmon/_atlas.md"))?;
    assert!(akmon.contains("## Recent decisions"));
    assert!(akmon.contains("stainless templates"));

    let csv = fs::read_to_string(vault.join("semantic/projects/csv-tool/_atlas.md"))?;
    assert!(csv.contains("## Contradictions"));
    assert!(csv.contains("rust"));
    assert!(csv.contains("python"));

    let memora = fs::read_to_string(vault.join("semantic/projects/memora/_atlas.md"))?;
    assert!(memora.contains("## Stale dependencies"));
    assert!(memora.contains("staleness case a synthesis"));
    assert!(memora.contains("retrieval-eval-notes"));
    assert!(memora.contains("## Open questions"));

    let world_map = fs::read_to_string(vault.join("world_map.md"))?;
    assert!(world_map.contains("## Today's review"));
    assert!(!world_map.contains("(challenger placeholder)"));
    Ok(())
}
