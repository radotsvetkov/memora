use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use memora_core::claims::{
    Claim, ClaimExtractor, ClaimRelation, ClaimStore, Provenance, StalenessTracker,
};
use memora_core::indexer::Indexer;
use memora_core::vault::{Vault, VaultEvent};
use memora_core::{Embedder, Index, Privacy, VectorIndex};
use memora_llm::{CompletionRequest, CompletionResponse, LlmClient, LlmDestination, LlmError};
use tempfile::tempdir;

fn write_note(path: &Path, id: &str, created: &str, updated: &str, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = format!(
        r#"---
id: {id}
region: test/integration
source: personal
privacy: private
created: {created}
updated: {updated}
summary: "{id} summary"
tags: []
refs: []
---
{body}
"#
    );
    fs::write(path, content)?;
    Ok(())
}

fn canonical_pair(a: &str, b: &str) -> (String, String) {
    let a = a.trim().to_ascii_lowercase();
    let b = b.trim().to_ascii_lowercase();
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

struct OneDimEmbedder;

#[async_trait]
impl Embedder for OneDimEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![1.0]).collect())
    }

    fn dim(&self) -> usize {
        1
    }

    fn model_id(&self) -> &str {
        "test/one-dim"
    }
}

struct MockClaimsLlm {
    extraction_responses: HashMap<String, String>,
    equivalent_pairs: HashSet<(String, String)>,
    contradiction_yes: bool,
}

impl MockClaimsLlm {
    fn new(
        extraction_responses: HashMap<String, String>,
        equivalent_pairs: HashSet<(String, String)>,
        contradiction_yes: bool,
    ) -> Self {
        Self {
            extraction_responses,
            equivalent_pairs,
            contradiction_yes,
        }
    }
}

#[async_trait]
impl LlmClient for MockClaimsLlm {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let prompt = req
            .messages
            .first()
            .map(|msg| msg.content.as_str())
            .unwrap_or_default();

        let text = if prompt.contains("You extract atomic factual claims from a note") {
            let note_id = prompt
                .lines()
                .find_map(|line| line.strip_prefix("Note id: "))
                .unwrap_or_default();
            self.extraction_responses
                .get(note_id)
                .cloned()
                .unwrap_or_else(|| "[]".to_string())
        } else if req.json_mode && prompt.contains("\"equivalent\"") {
            let a = prompt
                .lines()
                .find_map(|line| line.strip_prefix("Predicate A: "))
                .unwrap_or_default();
            let b = prompt
                .lines()
                .find_map(|line| line.strip_prefix("Predicate B: "))
                .unwrap_or_default();
            let equiv = self.equivalent_pairs.contains(&canonical_pair(a, b));
            format!(r#"{{"equivalent":{equiv}}}"#)
        } else if req.json_mode && prompt.contains("\"contradicts\"") {
            format!(r#"{{"contradicts":{}}}"#, self.contradiction_yes)
        } else if prompt.contains("Are these two predicates synonymous") {
            let a = prompt
                .lines()
                .find_map(|line| line.strip_prefix("A: "))
                .unwrap_or_default();
            let b = prompt
                .lines()
                .find_map(|line| line.strip_prefix("B: "))
                .unwrap_or_default();
            if self.equivalent_pairs.contains(&canonical_pair(a, b)) {
                "yes".to_string()
            } else {
                "no".to_string()
            }
        } else if prompt.contains("Do these claims contradict each other?") {
            if self.contradiction_yes {
                "yes: conflicting objects".to_string()
            } else {
                "no: compatible".to_string()
            }
        } else {
            "no".to_string()
        };

        Ok(CompletionResponse {
            text,
            model: "mock/claims".to_string(),
            input_tokens: None,
            output_tokens: None,
        })
    }

    fn model_name(&self) -> &str {
        "mock/claims"
    }

    fn destination(&self) -> LlmDestination {
        LlmDestination::Local
    }
}

fn make_claim_json(
    subject: &str,
    predicate: &str,
    object: &str,
    span_start: usize,
    span_end: usize,
) -> String {
    format!(
        r#"[{{"s":"{subject}","p":"{predicate}","o":"{object}","span_start":{span_start},"span_end":{span_end},"valid_from":null,"valid_until":null,"confidence":1.0}}]"#
    )
}

fn build_indexer<'a>(
    vault: &'a Vault,
    index: &'a Index,
    llm: Arc<dyn LlmClient>,
    vectors_root: &Path,
) -> Result<Indexer<'a>> {
    let embedder: Arc<dyn Embedder> = Arc::new(OneDimEmbedder);
    let vector_index = Arc::new(Mutex::new(VectorIndex::open_or_create(vectors_root, 1)?));
    let claim_extractor = ClaimExtractor {
        llm,
        model_label: "test/mock-claims".to_string(),
    };
    Ok(Indexer::new(vault, index, embedder, vector_index).with_claims(claim_extractor))
}

#[tokio::test]
async fn conflicting_claims_mark_older_valid_until_and_relations() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    let note_old_path = vault_root.join("old.md");
    let note_new_path = vault_root.join("new.md");

    let old_body = "Rado role is engineer at ACME.";
    let new_body = "Rado role is manager at ACME.";
    write_note(
        &note_old_path,
        "note-old",
        "2026-01-01T00:00:00Z",
        "2026-01-01T00:00:00Z",
        old_body,
    )?;
    write_note(
        &note_new_path,
        "note-new",
        "2026-02-01T00:00:00Z",
        "2026-02-01T00:00:00Z",
        new_body,
    )?;

    let mut extraction_responses = HashMap::new();
    extraction_responses.insert(
        "note-old".to_string(),
        make_claim_json("Rado", "role_is", "engineer", 0, old_body.len()),
    );
    extraction_responses.insert(
        "note-new".to_string(),
        make_claim_json("Rado", "role_is", "manager", 0, new_body.len()),
    );
    let llm = Arc::new(MockClaimsLlm::new(
        extraction_responses,
        HashSet::new(),
        true,
    ));

    let index = Index::open(&temp.path().join("index").join("memora.db"))?;
    let vault = Vault::new(&vault_root);
    let indexer = build_indexer(
        &vault,
        &index,
        llm,
        &temp.path().join("index").join("vectors"),
    )?;
    indexer.full_rebuild().await?;

    let store = ClaimStore::new(&index);
    let older_id = store
        .list_for_note("note-old")?
        .into_iter()
        .next()
        .expect("older claim id")
        .id;
    let newer_id = store
        .list_for_note("note-new")?
        .into_iter()
        .next()
        .expect("newer claim id")
        .id;
    let older = store.get(&older_id)?.expect("older claim");
    let newer = store.get(&newer_id)?.expect("newer claim");

    assert_eq!(older.valid_until, Some(newer.valid_from));
    assert!(store.has_relation(&newer.id, &older.id, ClaimRelation::Supersedes)?);
    assert!(store.has_relation(&newer.id, &older.id, ClaimRelation::Contradicts)?);
    Ok(())
}

#[tokio::test]
async fn note_edit_marks_derived_claims_as_stale() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    let note_a_path = vault_root.join("a.md");
    let note_d_path = vault_root.join("derived.md");

    let body_a = "Project alpha status is green this week.";
    let body_d = "Synthetic derived note body used for FK support.";
    write_note(
        &note_a_path,
        "note-a",
        "2026-03-01T00:00:00Z",
        "2026-03-01T00:00:00Z",
        body_a,
    )?;
    write_note(
        &note_d_path,
        "note-derived",
        "2026-03-01T00:00:00Z",
        "2026-03-01T00:00:00Z",
        body_d,
    )?;

    let mut extraction_responses = HashMap::new();
    extraction_responses.insert(
        "note-a".to_string(),
        make_claim_json("Project alpha", "status_is", "green", 0, body_a.len()),
    );
    extraction_responses.insert("note-derived".to_string(), "[]".to_string());
    let llm = Arc::new(MockClaimsLlm::new(
        extraction_responses,
        HashSet::new(),
        false,
    ));

    let index = Index::open(&temp.path().join("index").join("memora.db"))?;
    let vault = Vault::new(&vault_root);
    let indexer = build_indexer(
        &vault,
        &index,
        llm,
        &temp.path().join("index").join("vectors"),
    )?;
    indexer.full_rebuild().await?;

    let store = ClaimStore::new(&index);
    let source_claim = store
        .list_for_note("note-a")?
        .into_iter()
        .next()
        .expect("source claim");

    let derived_claim = Claim {
        id: Claim::compute_id(
            "Project alpha",
            "risk_level",
            Some("low"),
            "note-derived",
            0,
        ),
        subject: "Project alpha".to_string(),
        predicate: "risk_level".to_string(),
        object: Some("low".to_string()),
        note_id: "note-derived".to_string(),
        span_start: 0,
        span_end: body_d.len(),
        span_fingerprint: Claim::compute_fingerprint(body_d),
        valid_from: Utc
            .with_ymd_and_hms(2026, 3, 1, 0, 0, 0)
            .single()
            .expect("valid date"),
        valid_until: None,
        confidence: 1.0,
        privacy: Privacy::Private,
        extracted_by: "test".to_string(),
        extracted_at: Utc::now(),
    };
    store.upsert(&derived_claim)?;

    let provenance = Provenance::new(&index);
    provenance.record(&derived_claim.id, &[&source_claim.id])?;

    let edited_body_a = "Project alpha status is amber this week.";
    write_note(
        &note_a_path,
        "note-a",
        "2026-03-01T00:00:00Z",
        "2026-03-02T00:00:00Z",
        edited_body_a,
    )?;
    indexer
        .handle_event(VaultEvent::Modified(note_a_path.clone()))
        .await?;

    let stale = StalenessTracker::new(&index, &provenance);
    let stale_items = stale.list_stale()?;
    assert!(stale_items
        .iter()
        .any(|(id, reason)| id == &derived_claim.id && reason == "source_edited"));
    Ok(())
}

#[tokio::test]
async fn equivalent_predicates_use_llm_synonym_check_for_contradictions() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    let older_path = vault_root.join("eq-old.md");
    let newer_path = vault_root.join("eq-new.md");

    let older_body = "ACME headquarters city is Berlin Germany.";
    let newer_body = "ACME is based in Munich Germany.";
    write_note(
        &older_path,
        "eq-old",
        "2026-01-01T00:00:00Z",
        "2026-01-01T00:00:00Z",
        older_body,
    )?;
    write_note(
        &newer_path,
        "eq-new",
        "2026-04-01T00:00:00Z",
        "2026-04-01T00:00:00Z",
        newer_body,
    )?;

    let mut extraction_responses = HashMap::new();
    extraction_responses.insert(
        "eq-old".to_string(),
        make_claim_json("ACME", "hq_city", "Berlin", 0, older_body.len()),
    );
    extraction_responses.insert(
        "eq-new".to_string(),
        make_claim_json("ACME", "based_in_city", "Munich", 0, newer_body.len()),
    );
    let mut equivalent_pairs = HashSet::new();
    equivalent_pairs.insert(canonical_pair("hq_city", "based_in_city"));
    let llm = Arc::new(MockClaimsLlm::new(
        extraction_responses,
        equivalent_pairs,
        true,
    ));

    let index = Index::open(&temp.path().join("index").join("memora.db"))?;
    let vault = Vault::new(&vault_root);
    let indexer = build_indexer(
        &vault,
        &index,
        llm,
        &temp.path().join("index").join("vectors"),
    )?;
    indexer.full_rebuild().await?;

    let store = ClaimStore::new(&index);
    let older_id = store
        .list_for_note("eq-old")?
        .into_iter()
        .next()
        .expect("older claim id")
        .id;
    let newer_id = store
        .list_for_note("eq-new")?
        .into_iter()
        .next()
        .expect("newer claim id")
        .id;
    let older = store.get(&older_id)?.expect("older claim");
    let newer = store.get(&newer_id)?.expect("newer claim");
    assert_eq!(older.valid_until, Some(newer.valid_from));
    assert!(store.has_relation(&newer.id, &older.id, ClaimRelation::Contradicts)?);
    Ok(())
}

#[tokio::test]
async fn reindex_is_idempotent_for_contradictions_and_stale_rows() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    let note_old_path = vault_root.join("idem-old.md");
    let note_new_path = vault_root.join("idem-new.md");

    let old_body = "Roadmap status is pending approval.";
    let new_body = "Roadmap status is approved.";
    write_note(
        &note_old_path,
        "idem-old",
        "2026-01-01T00:00:00Z",
        "2026-01-01T00:00:00Z",
        old_body,
    )?;
    write_note(
        &note_new_path,
        "idem-new",
        "2026-02-01T00:00:00Z",
        "2026-02-01T00:00:00Z",
        new_body,
    )?;

    let mut extraction_responses = HashMap::new();
    extraction_responses.insert(
        "idem-old".to_string(),
        make_claim_json("Roadmap", "status_is", "pending", 0, old_body.len()),
    );
    extraction_responses.insert(
        "idem-new".to_string(),
        make_claim_json("Roadmap", "status_is", "approved", 0, new_body.len()),
    );
    let llm = Arc::new(MockClaimsLlm::new(
        extraction_responses,
        HashSet::new(),
        true,
    ));

    let index = Index::open(&temp.path().join("index").join("memora.db"))?;
    let vault = Vault::new(&vault_root);
    let indexer = build_indexer(
        &vault,
        &index,
        llm,
        &temp.path().join("index").join("vectors"),
    )?;

    indexer.full_rebuild().await?;
    let store = ClaimStore::new(&index);
    let provenance = Provenance::new(&index);
    let stale = StalenessTracker::new(&index, &provenance);
    let first_contradictions = store.contradictions_unack()?.len();
    let first_stale = stale.list_stale()?.len();

    indexer.full_rebuild().await?;
    let second_contradictions = store.contradictions_unack()?.len();
    let second_stale = stale.list_stale()?.len();

    assert_eq!(first_contradictions, 1);
    assert_eq!(second_contradictions, first_contradictions);
    assert_eq!(first_stale, 0);
    assert_eq!(second_stale, first_stale);
    Ok(())
}

#[tokio::test]
async fn skip_contradiction_detection_skips_contradiction_edges() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    let note_old_path = vault_root.join("skip-old.md");
    let note_new_path = vault_root.join("skip-new.md");

    let old_body = "Rado role is engineer at ACME.";
    let new_body = "Rado role is manager at ACME.";
    write_note(
        &note_old_path,
        "skip-old",
        "2026-01-01T00:00:00Z",
        "2026-01-01T00:00:00Z",
        old_body,
    )?;
    write_note(
        &note_new_path,
        "skip-new",
        "2026-02-01T00:00:00Z",
        "2026-02-01T00:00:00Z",
        new_body,
    )?;

    let mut extraction_responses = HashMap::new();
    extraction_responses.insert(
        "skip-old".to_string(),
        make_claim_json("Rado", "role_is", "engineer", 0, old_body.len()),
    );
    extraction_responses.insert(
        "skip-new".to_string(),
        make_claim_json("Rado", "role_is", "manager", 0, new_body.len()),
    );
    let llm = Arc::new(MockClaimsLlm::new(
        extraction_responses,
        HashSet::new(),
        true,
    ));

    let index = Index::open(&temp.path().join("index").join("memora.db"))?;
    let vault = Vault::new(&vault_root);
    let indexer = build_indexer(
        &vault,
        &index,
        llm,
        &temp.path().join("index").join("vectors"),
    )?
    .with_skip_contradiction_detection(true);
    indexer.full_rebuild().await?;

    let store = ClaimStore::new(&index);
    let newer_id = store
        .list_for_note("skip-new")?
        .into_iter()
        .next()
        .expect("newer claim")
        .id;
    let older_id = store
        .list_for_note("skip-old")?
        .into_iter()
        .next()
        .expect("older claim")
        .id;

    assert!(
        !store.has_relation(&newer_id, &older_id, ClaimRelation::Contradicts)?,
        "contradiction edges should not be written when skip_contradiction_detection is enabled"
    );
    Ok(())
}
