use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use memora_core::note::{self, Frontmatter, Note, NoteSource, Privacy};
use memora_core::{
    Challenger, ChallengerConfig, Claim, ClaimRelation, ClaimStore, Index, StaleAlert,
};
use memora_llm::{CompletionRequest, CompletionResponse, LlmClient, LlmDestination, LlmError};
use rusqlite::params;
use tempfile::tempdir;

struct ScriptedLlm;

#[async_trait]
impl LlmClient for ScriptedLlm {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let prompt = req
            .messages
            .first()
            .map(|message| message.content.as_str())
            .unwrap_or_default();
        let text = if prompt.contains("Output JSON") {
            r#"{"action":"update","new_claim":{"s":"Rado","p":"works_at","o":"Memora Labs"}}"#
                .to_string()
        } else if prompt.contains("Summarize this contradiction") {
            "These claims conflict about the same subject.".to_string()
        } else if prompt.contains("Generate one specific factual question") {
            "What primary source confirms the Q4 launch window?".to_string()
        } else {
            "ok".to_string()
        };
        Ok(CompletionResponse {
            text,
            model: "mock/challenger".to_string(),
            input_tokens: None,
            output_tokens: None,
        })
    }

    fn model_name(&self) -> &str {
        "mock/challenger"
    }

    fn destination(&self) -> LlmDestination {
        LlmDestination::Local
    }
}

struct Fixture {
    vault: PathBuf,
    db_path: PathBuf,
    index: Index,
    llm: ScriptedLlm,
}

fn setup_fixture() -> Result<Fixture> {
    let temp = tempdir()?;
    let root = temp.keep();
    let vault = root.join("vault");
    fs::create_dir_all(&vault)?;
    let db_path = root.join("index").join("memora.db");
    let index = Index::open(&db_path)?;
    Ok(Fixture {
        vault,
        db_path,
        index,
        llm: ScriptedLlm,
    })
}

fn seed_note(
    fixture: &Fixture,
    id: &str,
    region: &str,
    rel_path: &str,
    body: &str,
) -> Result<PathBuf> {
    let note = Note {
        path: PathBuf::from(rel_path),
        fm: Frontmatter {
            id: id.to_string(),
            region: region.to_string(),
            source: NoteSource::Personal,
            privacy: Privacy::Private,
            created: Utc
                .with_ymd_and_hms(2026, 4, 1, 0, 0, 0)
                .single()
                .ok_or_else(|| anyhow::anyhow!("invalid created test datetime"))?,
            updated: Utc
                .with_ymd_and_hms(2026, 4, 2, 0, 0, 0)
                .single()
                .ok_or_else(|| anyhow::anyhow!("invalid updated test datetime"))?,
            summary: format!("summary for {id}"),
            tags: Vec::new(),
            refs: Vec::new(),
        },
        body: body.to_string(),
        wikilinks: Vec::new(),
    };
    let absolute_path = fixture.vault.join(rel_path);
    if let Some(parent) = absolute_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&absolute_path, note::render(&note))?;
    fixture.index.upsert_note(&note, body)?;
    Ok(absolute_path)
}

fn seed_claim(
    store: &ClaimStore<'_>,
    note_id: &str,
    subject: &str,
    predicate: &str,
    object: &str,
    confidence: f32,
) -> Result<Claim> {
    let span_text = format!("{subject} {predicate} {object}");
    let claim = Claim {
        id: Claim::compute_id(subject, predicate, object, note_id, 0),
        subject: subject.to_string(),
        predicate: predicate.to_string(),
        object: object.to_string(),
        note_id: note_id.to_string(),
        span_start: 0,
        span_end: span_text.len(),
        span_fingerprint: Claim::compute_fingerprint(&span_text),
        valid_from: Utc
            .with_ymd_and_hms(2026, 4, 1, 0, 0, 0)
            .single()
            .ok_or_else(|| anyhow::anyhow!("invalid claim datetime"))?,
        valid_until: None,
        confidence,
        privacy: Privacy::Private,
        extracted_by: "test/challenger".to_string(),
        extracted_at: Utc::now(),
    };
    store.upsert(&claim)?;
    Ok(claim)
}

fn mark_stale(db_path: &Path, claim_id: &str) -> Result<()> {
    let conn = rusqlite::Connection::open(db_path)?;
    conn.execute(
        "INSERT OR REPLACE INTO stale_claims (claim_id, reason, marked_at)
         VALUES (?, ?, ?)",
        params![claim_id, "source_edited", Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

fn run_challenger<'a>(fixture: &'a Fixture, store: &'a ClaimStore<'a>) -> Challenger<'a> {
    Challenger {
        db: &fixture.index,
        claim_store: store,
        llm: &fixture.llm,
        vault: &fixture.vault,
        config: ChallengerConfig::default(),
    }
}

#[tokio::test]
async fn stale_claim_emits_update_proposal() -> Result<()> {
    let fixture = setup_fixture()?;
    seed_note(
        &fixture,
        "note-stale",
        "ops/eu",
        "ops/eu/note-stale.md",
        "Rado works_at HMC",
    )?;
    seed_note(
        &fixture,
        "note-other",
        "ops/eu",
        "ops/eu/note-other.md",
        "Rado works_at Example",
    )?;
    let store = ClaimStore::new(&fixture.index);
    let stale_claim = seed_claim(&store, "note-stale", "Rado", "works_at", "HMC", 0.9)?;
    seed_claim(&store, "note-other", "Rado", "works_at", "Example", 0.9)?;
    mark_stale(&fixture.db_path, &stale_claim.id)?;

    let report = run_challenger(&fixture, &store).run_once().await?;
    assert_eq!(report.stale_alerts.len(), 1);
    let alert: &StaleAlert = &report.stale_alerts[0];
    assert_eq!(alert.claim_id, stale_claim.id);
    assert_eq!(alert.proposal_action, "update");
    assert_eq!(alert.proposal_subject.as_deref(), Some("Rado"));
    assert_eq!(alert.proposal_predicate.as_deref(), Some("works_at"));
    assert_eq!(alert.proposal_object.as_deref(), Some("Memora Labs"));
    Ok(())
}

#[tokio::test]
async fn contradiction_pair_emits_contradiction_alert() -> Result<()> {
    let fixture = setup_fixture()?;
    seed_note(
        &fixture,
        "note-a",
        "ops/eu",
        "ops/eu/note-a.md",
        "Rado works_at HMC",
    )?;
    seed_note(
        &fixture,
        "note-b",
        "ops/us",
        "ops/us/note-b.md",
        "Rado works_at Memora Labs",
    )?;
    let store = ClaimStore::new(&fixture.index);
    let left = seed_claim(&store, "note-a", "Rado", "works_at", "HMC", 0.9)?;
    let right = seed_claim(&store, "note-b", "Rado", "works_at", "Memora Labs", 0.9)?;
    store.add_relation(&left.id, &right.id, ClaimRelation::Contradicts, 1.0)?;

    let report = run_challenger(&fixture, &store).run_once().await?;
    assert_eq!(report.contradiction_alerts.len(), 1);
    let alert = &report.contradiction_alerts[0];
    assert_eq!(alert.left_claim_id, left.id);
    assert_eq!(alert.right_claim_id, right.id);
    assert!(alert.description.contains("conflict"));
    Ok(())
}

#[tokio::test]
async fn cross_region_subject_without_home_note_emits_alert() -> Result<()> {
    let fixture = setup_fixture()?;
    for (region, note_id) in [
        ("eu/events", "internorga-eu"),
        ("us/events", "internorga-us"),
        ("apac/events", "internorga-apac"),
        ("mena/events", "internorga-mena"),
    ] {
        seed_note(
            &fixture,
            note_id,
            region,
            &format!("{region}/{note_id}.md"),
            "INTERNORGA has annual expo",
        )?;
    }
    let store = ClaimStore::new(&fixture.index);
    for note_id in [
        "internorga-eu",
        "internorga-us",
        "internorga-apac",
        "internorga-mena",
    ] {
        seed_claim(
            &store,
            note_id,
            "INTERNORGA",
            "hosts_event",
            "annual expo",
            0.9,
        )?;
    }

    let report = run_challenger(&fixture, &store).run_once().await?;
    assert_eq!(report.cross_region_alerts.len(), 1);
    let alert = &report.cross_region_alerts[0];
    assert_eq!(alert.subject, "INTERNORGA");
    assert_eq!(alert.regions.len(), 4);
    assert_eq!(alert.suggested_home_note_id, "internorga");
    assert!(alert.description.contains("home note"));
    Ok(())
}

#[tokio::test]
async fn low_confidence_claim_emits_frontier_alert() -> Result<()> {
    let fixture = setup_fixture()?;
    seed_note(
        &fixture,
        "frontier-note",
        "research",
        "research/frontier-note.md",
        "Product likely launches in Q4",
    )?;
    seed_note(
        &fixture,
        "support-note",
        "research",
        "research/support-note.md",
        "Another claim to avoid rare predicate",
    )?;
    let store = ClaimStore::new(&fixture.index);
    let low = seed_claim(
        &store,
        "frontier-note",
        "Product",
        "launch_window",
        "Q4",
        0.2,
    )?;
    seed_claim(
        &store,
        "support-note",
        "Product",
        "launch_window",
        "unknown",
        0.9,
    )?;

    let report = run_challenger(&fixture, &store).run_once().await?;
    assert_eq!(report.frontier_alerts.len(), 1);
    let alert = &report.frontier_alerts[0];
    assert_eq!(alert.claim_id, low.id);
    assert!(alert.clarifying_question.ends_with('?'));
    Ok(())
}

#[tokio::test]
async fn rerun_is_idempotent_except_for_timestamps() -> Result<()> {
    let fixture = setup_fixture()?;
    seed_note(
        &fixture,
        "note-stale",
        "ops/eu",
        "ops/eu/note-stale.md",
        "Rado works_at HMC",
    )?;
    seed_note(
        &fixture,
        "note-contradict",
        "ops/us",
        "ops/us/note-contradict.md",
        "Rado works_at Memora Labs",
    )?;
    seed_note(
        &fixture,
        "note-frontier",
        "ops/apac",
        "ops/apac/note-frontier.md",
        "Product launch_window Q4",
    )?;
    seed_note(
        &fixture,
        "internorga-eu",
        "eu/events",
        "eu/events/internorga-eu.md",
        "INTERNORGA hosts_event annual expo",
    )?;
    seed_note(
        &fixture,
        "internorga-us",
        "us/events",
        "us/events/internorga-us.md",
        "INTERNORGA hosts_event annual expo",
    )?;
    seed_note(
        &fixture,
        "internorga-apac",
        "apac/events",
        "apac/events/internorga-apac.md",
        "INTERNORGA hosts_event annual expo",
    )?;
    seed_note(
        &fixture,
        "internorga-mena",
        "mena/events",
        "mena/events/internorga-mena.md",
        "INTERNORGA hosts_event annual expo",
    )?;

    let store = ClaimStore::new(&fixture.index);
    let stale_claim = seed_claim(&store, "note-stale", "Rado", "works_at", "HMC", 0.9)?;
    let contradictory_claim = seed_claim(
        &store,
        "note-contradict",
        "Rado",
        "works_at",
        "Memora Labs",
        0.9,
    )?;
    let frontier_claim = seed_claim(
        &store,
        "note-frontier",
        "Product",
        "launch_window",
        "Q4",
        0.2,
    )?;
    seed_claim(
        &store,
        "note-contradict",
        "Product",
        "launch_window",
        "unknown",
        0.9,
    )?;
    for note_id in [
        "internorga-eu",
        "internorga-us",
        "internorga-apac",
        "internorga-mena",
    ] {
        seed_claim(
            &store,
            note_id,
            "INTERNORGA",
            "hosts_event",
            "annual expo",
            0.9,
        )?;
    }
    mark_stale(&fixture.db_path, &stale_claim.id)?;
    store.add_relation(
        &stale_claim.id,
        &contradictory_claim.id,
        ClaimRelation::Contradicts,
        1.0,
    )?;

    let challenger = run_challenger(&fixture, &store);
    let first = challenger.run_once().await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    let second = challenger.run_once().await?;

    assert!(second.generated_at > first.generated_at);
    assert_eq!(
        first
            .stale_alerts
            .iter()
            .map(|a| (&a.claim_id, &a.proposal_action, &a.proposal_object))
            .collect::<Vec<_>>(),
        second
            .stale_alerts
            .iter()
            .map(|a| (&a.claim_id, &a.proposal_action, &a.proposal_object))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        first
            .contradiction_alerts
            .iter()
            .map(|a| (&a.left_claim_id, &a.right_claim_id, &a.description))
            .collect::<Vec<_>>(),
        second
            .contradiction_alerts
            .iter()
            .map(|a| (&a.left_claim_id, &a.right_claim_id, &a.description))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        first
            .cross_region_alerts
            .iter()
            .map(|a| (&a.subject, &a.regions, &a.suggested_home_note_id))
            .collect::<Vec<_>>(),
        second
            .cross_region_alerts
            .iter()
            .map(|a| (&a.subject, &a.regions, &a.suggested_home_note_id))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        first
            .frontier_alerts
            .iter()
            .map(|a| (&a.claim_id, &a.clarifying_question, &a.confidence))
            .collect::<Vec<_>>(),
        second
            .frontier_alerts
            .iter()
            .map(|a| (&a.claim_id, &a.clarifying_question, &a.confidence))
            .collect::<Vec<_>>()
    );
    assert!(first
        .frontier_alerts
        .iter()
        .any(|a| a.claim_id == frontier_claim.id));
    Ok(())
}
