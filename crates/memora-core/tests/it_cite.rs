use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use memora_core::answer::AnsweringPipeline;
use memora_core::cite::{CitationStatus, CitationValidator};
use memora_core::claims::{Claim, ClaimStore};
use memora_core::note::{Frontmatter, Note, NoteSource, Privacy};
use memora_core::{Embedder, HybridRetriever, Index, PrivacyConfig, PrivacyFilter, VectorIndex};
use memora_llm::{
    CompletionRequest, CompletionResponse, LlmClient, LlmDestination, LlmError, LlmProvider,
};
use tempfile::tempdir;

fn write_note_file(path: &Path, id: &str, summary: &str, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = format!(
        r#"---
id: {id}
region: test/integration
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-02T00:00:00Z
summary: "{summary}"
tags: []
refs: []
---
{body}
"#
    );
    fs::write(path, content)?;
    Ok(())
}

fn build_note(rel_path: PathBuf, id: &str, summary: &str, body: &str) -> Note {
    Note {
        path: rel_path,
        fm: Frontmatter {
            id: id.to_string(),
            region: "test/integration".to_string(),
            source: NoteSource::Personal,
            privacy: Privacy::Private,
            created: Utc
                .with_ymd_and_hms(2026, 4, 1, 0, 0, 0)
                .single()
                .expect("valid created date"),
            updated: Utc
                .with_ymd_and_hms(2026, 4, 2, 0, 0, 0)
                .single()
                .expect("valid updated date"),
            summary: summary.to_string(),
            tags: Vec::new(),
            refs: Vec::new(),
        },
        body: body.to_string(),
        wikilinks: Vec::new(),
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

struct MockLlm {
    responses: Mutex<VecDeque<String>>,
    requests: Mutex<Vec<CompletionRequest>>,
}

impl MockLlm {
    fn new(responses: Vec<String>) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from(responses)),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LlmClient for MockLlm {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(req);
        let text = self
            .responses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
            .unwrap_or_default();
        Ok(CompletionResponse {
            text,
            model: "mock/llm".to_string(),
            input_tokens: None,
            output_tokens: None,
        })
    }

    fn model_name(&self) -> &str {
        "mock/llm"
    }

    fn destination(&self) -> LlmDestination {
        LlmDestination::Local
    }
}

struct Fixture {
    vault_root: PathBuf,
    index: Index,
    vector_index: VectorIndex,
    note: Note,
    body: String,
}

fn setup_fixture() -> Result<Fixture> {
    let temp = tempdir()?;
    let root = temp.keep();
    let vault_root = root.join("vault");
    fs::create_dir_all(&vault_root)?;
    let rel_path = PathBuf::from("project/note.md");
    let body = "Rado works at HMC. INTERNORGA had 3500 exhibitors.";
    write_note_file(&vault_root.join(&rel_path), "note-1", "Memora note", body)?;
    let note = build_note(rel_path, "note-1", "Memora note", body);

    let index = Index::open(&root.join("index").join("memora.db"))?;
    index.upsert_note(&note, body)?;
    let mut vector_index = VectorIndex::open_or_create(&root.join("index").join("vectors"), 1)?;
    vector_index.upsert(&note.fm.id, &[1.0])?;
    vector_index.save()?;

    Ok(Fixture {
        vault_root,
        index,
        vector_index,
        note,
        body: body.to_string(),
    })
}

fn upsert_test_claim(
    fixture: &Fixture,
    claim_id: &str,
    span_start: usize,
    span_end: usize,
) -> Result<Claim> {
    let span = fixture
        .body
        .get(span_start..span_end)
        .expect("test span must be valid UTF-8 slice");
    let claim = Claim {
        id: claim_id.to_string(),
        subject: "Rado".to_string(),
        predicate: "works_at".to_string(),
        object: "HMC".to_string(),
        note_id: fixture.note.fm.id.clone(),
        span_start,
        span_end,
        span_fingerprint: Claim::compute_fingerprint(span),
        valid_from: fixture.note.fm.created,
        valid_until: None,
        confidence: 1.0,
        privacy: Privacy::Private,
        extracted_by: "test/mock-extractor".to_string(),
        extracted_at: Utc::now(),
    };
    ClaimStore::new(&fixture.index).upsert(&claim)?;
    Ok(claim)
}

#[tokio::test]
async fn validator_verifies_known_claim_marker() -> Result<()> {
    let fixture = setup_fixture()?;
    let start = fixture.body.find("Rado works at HMC").expect("span start");
    let end = start + "Rado works at HMC".len();
    let claim = upsert_test_claim(&fixture, "aaaaaaaaaaaaaaaa", start, end)?;
    let store = ClaimStore::new(&fixture.index);
    let validator = CitationValidator {
        store: &store,
        index: &fixture.index,
        vault_root: &fixture.vault_root,
    };

    let response = format!("Rado works at HMC [claim:{}].", claim.id);
    let cited = validator.validate(&response).await?;
    assert_eq!(cited.verified_count, 1);
    assert_eq!(cited.unverified_count, 0);
    assert_eq!(cited.checks[0].status, CitationStatus::Verified);
    Ok(())
}

#[tokio::test]
async fn validator_marks_hallucinated_claim_id_unverified() -> Result<()> {
    let fixture = setup_fixture()?;
    let store = ClaimStore::new(&fixture.index);
    let validator = CitationValidator {
        store: &store,
        index: &fixture.index,
        vault_root: &fixture.vault_root,
    };

    let cited = validator
        .validate("Invented fact [claim:ffffffffffffffff].")
        .await?;
    assert_eq!(cited.verified_count, 0);
    assert_eq!(cited.unverified_count, 1);
    assert_eq!(cited.checks[0].status, CitationStatus::Unverified);
    Ok(())
}

#[tokio::test]
async fn validator_detects_fingerprint_mismatch_after_note_edit() -> Result<()> {
    let fixture = setup_fixture()?;
    let start = fixture.body.find("Rado works at HMC").expect("span start");
    let end = start + "Rado works at HMC".len();
    let claim = upsert_test_claim(&fixture, "bbbbbbbbbbbbbbbb", start, end)?;
    let store = ClaimStore::new(&fixture.index);
    let validator = CitationValidator {
        store: &store,
        index: &fixture.index,
        vault_root: &fixture.vault_root,
    };

    let changed = "Rado works remotely now. INTERNORGA had 3500 exhibitors.";
    write_note_file(
        &fixture.vault_root.join("project/note.md"),
        "note-1",
        "Memora note",
        changed,
    )?;

    let cited = validator
        .validate(&format!("Rado works at HMC [claim:{}].", claim.id))
        .await?;
    assert_eq!(cited.checks[0].status, CitationStatus::FingerprintMismatch);
    Ok(())
}

#[tokio::test]
async fn validator_detects_quote_mismatch() -> Result<()> {
    let fixture = setup_fixture()?;
    let start = fixture.body.find("Rado works at HMC").expect("span start");
    let end = start + "Rado works at HMC".len();
    let claim = upsert_test_claim(&fixture, "cccccccccccccccc", start, end)?;
    let store = ClaimStore::new(&fixture.index);
    let validator = CitationValidator {
        store: &store,
        index: &fixture.index,
        vault_root: &fixture.vault_root,
    };

    let cited = validator
        .validate(&format!(
            "\"Different quote entirely\" [claim:{}].",
            claim.id
        ))
        .await?;
    assert_eq!(cited.checks[0].status, CitationStatus::QuoteMismatch);
    Ok(())
}

#[tokio::test]
async fn answering_pipeline_retries_and_keeps_only_verified_markers() -> Result<()> {
    let fixture = setup_fixture()?;
    let start = fixture.body.find("Rado works at HMC").expect("span start");
    let end = start + "Rado works at HMC".len();
    let claim = upsert_test_claim(&fixture, "dddddddddddddddd", start, end)?;
    let store = ClaimStore::new(&fixture.index);
    let validator = CitationValidator {
        store: &store,
        index: &fixture.index,
        vault_root: &fixture.vault_root,
    };
    let embedder = OneDimEmbedder;
    let retriever = HybridRetriever {
        index: &fixture.index,
        vec: &fixture.vector_index,
        embedder: &embedder,
    };
    let llm = MockLlm::new(vec![
        format!(
            "Rado works at HMC [claim:{}]. Extra hallucination [claim:eeeeeeeeeeeeeeee].",
            claim.id
        ),
        format!("Rado works at HMC [claim:{}].", claim.id),
    ]);
    let pipeline = AnsweringPipeline {
        retriever: &retriever,
        claim_store: &store,
        validator: &validator,
        llm: &llm,
        privacy_filter: PrivacyFilter::new_for(LlmProvider::Ollama),
        privacy_config: PrivacyConfig::default(),
    };

    let cited = pipeline.answer("Rado", 5).await?;
    assert!(!cited.degraded);
    assert_eq!(cited.unverified_count, 0);
    assert_eq!(cited.mismatch_count, 0);
    assert!(cited.raw_text.contains(&claim.id));
    assert!(!cited.raw_text.contains("eeeeeeeeeeeeeeee"));
    assert!(!cited.clean_text.contains("eeeeeeeeeeeeeeee"));

    let requests = llm
        .requests
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(requests.len(), 2);
    let retry_system = requests[1]
        .system
        .as_deref()
        .expect("retry request should set system prompt");
    assert!(retry_system.contains(&claim.id));
    assert!(retry_system.contains("Use ONLY these claim ids"));
    Ok(())
}
