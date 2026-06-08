//! Deterministic citation-rejection evaluation.
//!
//! This is the one metric memora is built to win: when an answer cites a claim
//! that does not exist, points at a source span whose bytes have changed, or
//! quotes text that is not in the cited span, memora's validator must REJECT it
//! before the answer is returned. A naive RAG / prompt-cite pipeline performs no
//! post-generation verification, so by construction it rejects 0% of fabricated
//! citations — it passes them straight through to the user.
//!
//! Every case here is checked against an expected outcome. The binary exits
//! non-zero if any expectation fails, so it doubles as a CI regression gate for
//! the core contract. No network, no API key, fully reproducible:
//!
//!   cargo run -p memora-bench --release --bin bench_citation_rejection
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use tempfile::tempdir;

use memora_core::{
    CitationStatus, CitationValidator, Claim, ClaimStore, Frontmatter, Index, Note, NoteSource,
    Privacy,
};

/// Whether a citation is legitimate (should be preserved) or fabricated
/// (should be rejected by the validator).
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Valid,
    Fabricated,
}

struct Case {
    name: &'static str,
    /// The model's answer text, containing exactly one `[claim:...]` marker.
    answer: String,
    kind: Kind,
    expected: CitationStatus,
}

#[tokio::main]
async fn main() -> Result<()> {
    let temp = tempdir()?;
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault)?;
    let index = Index::open(&temp.path().join("index.db"))?;
    let store = ClaimStore::new(&index);

    // --- Note A: a normal personal note with two extractable facts. ---
    let body_a = "Rado works at HMC and leads Memora.";
    seed_note(&vault, &index, "a.md", "note-a", body_a)?;
    // Claim A1: a faithfully extracted fact (correct span + correct fingerprint).
    let a1 = upsert_claim(
        &store,
        "0000000000000a01",
        "note-a",
        body_a,
        "Rado works at HMC",
        true,
    )?;
    // Claim A2: a claim whose stored fingerprint no longer matches the source
    // span (simulates the note being edited after extraction).
    let a2 = upsert_claim(
        &store,
        "0000000000000a02",
        "note-a",
        body_a,
        "leads Memora",
        false,
    )?;

    // --- Note B: a decision note. ---
    let body_b = "The launch deadline is March 15 for the EU region.";
    seed_note(&vault, &index, "b.md", "note-b", body_b)?;
    let b1 = upsert_claim(
        &store,
        "0000000000000b01",
        "note-b",
        body_b,
        "The launch deadline is March 15",
        true,
    )?;

    let validator = CitationValidator {
        store: &store,
        index: &index,
        vault_root: &vault,
    };

    // Build the evaluation set: legitimate citations that must survive, and
    // every class of fabricated citation that must be rejected.
    let cases = vec![
        Case {
            name: "valid: faithful citation, no quote",
            answer: format!("Rado works at HMC [claim:{a1}]."),
            kind: Kind::Valid,
            expected: CitationStatus::Verified,
        },
        Case {
            name: "valid: faithful citation with matching quote",
            answer: format!("It says \"Rado works at HMC\" [claim:{a1}]."),
            kind: Kind::Valid,
            expected: CitationStatus::Verified,
        },
        Case {
            name: "valid: second faithful citation",
            answer: format!("The launch deadline is March 15 [claim:{b1}]."),
            kind: Kind::Valid,
            expected: CitationStatus::Verified,
        },
        Case {
            name: "fabricated: hallucinated claim id (does not exist)",
            answer: "The earth is flat [claim:00000000deadbe01].".to_string(),
            kind: Kind::Fabricated,
            expected: CitationStatus::Unverified,
        },
        Case {
            name: "fabricated: second hallucinated claim id",
            answer: "Revenue tripled last quarter [claim:00000000deadbe02].".to_string(),
            kind: Kind::Fabricated,
            expected: CitationStatus::Unverified,
        },
        Case {
            name: "fabricated: quote not present in cited span",
            answer: format!("It says \"Rado works at Google\" [claim:{a1}]."),
            kind: Kind::Fabricated,
            expected: CitationStatus::QuoteMismatch,
        },
        Case {
            name: "fabricated: source span changed since extraction (fingerprint)",
            answer: format!("Rado leads Memora [claim:{a2}]."),
            kind: Kind::Fabricated,
            expected: CitationStatus::FingerprintMismatch,
        },
    ];

    let mut total_valid = 0usize;
    let mut preserved = 0usize;
    let mut total_fabricated = 0usize;
    let mut rejected = 0usize;
    let mut failures: Vec<String> = Vec::new();

    println!("memora citation-rejection evaluation");
    println!("-----------------------------------");
    for case in &cases {
        let answer = validator.validate(&case.answer).await?;
        let status = answer
            .checks
            .first()
            .map(|c| c.status)
            .unwrap_or(CitationStatus::Unverified);

        let expectation_met = status == case.expected;
        if !expectation_met {
            failures.push(format!(
                "{}: expected {:?}, got {:?}",
                case.name, case.expected, status
            ));
        }

        match case.kind {
            Kind::Valid => {
                total_valid += 1;
                let kept = status == CitationStatus::Verified;
                if kept {
                    preserved += 1;
                }
                // A valid citation should survive into the cleaned answer.
                let still_present = answer.clean_text.contains(claim_id_of(&case.answer));
                println!(
                    "  [VALID]      {:<55} -> {:?} {}",
                    case.name,
                    status,
                    mark(kept && still_present && expectation_met)
                );
            }
            Kind::Fabricated => {
                total_fabricated += 1;
                let dropped = status != CitationStatus::Verified;
                if dropped {
                    rejected += 1;
                }
                // A fabricated citation must NOT survive into the cleaned answer.
                let scrubbed = !answer.clean_text.contains(claim_id_of(&case.answer));
                println!(
                    "  [FABRICATED] {:<55} -> {:?} {}",
                    case.name,
                    status,
                    mark(dropped && scrubbed && expectation_met)
                );
            }
        }
    }

    let rejection_rate = ratio(rejected, total_fabricated);
    let preservation_rate = ratio(preserved, total_valid);

    println!();
    println!("RESULTS (measured, deterministic)");
    println!(
        "  fabricated_citation_rejection_rate : {:.3}  ({}/{} fabricated citations rejected)",
        rejection_rate, rejected, total_fabricated
    );
    println!(
        "  valid_citation_preservation_rate   : {:.3}  ({}/{} valid citations preserved)",
        preservation_rate, preserved, total_valid
    );
    println!(
        "  naive_rag_rejection_rate           : 0.000  (by construction: no post-generation verification)"
    );
    println!();

    if failures.is_empty() {
        println!("PASS: every citation was classified exactly as expected.");
        Ok(())
    } else {
        eprintln!("FAIL: {} expectation(s) not met:", failures.len());
        for f in &failures {
            eprintln!("  - {f}");
        }
        std::process::exit(1);
    }
}

/// Write a note file into the vault and register it in the index.
fn seed_note(vault: &Path, index: &Index, rel: &str, id: &str, body: &str) -> Result<()> {
    let rel_path = PathBuf::from(rel);
    let note = Note {
        path: rel_path.clone(),
        fm: Frontmatter {
            id: id.to_string(),
            region: "bench/citation".to_string(),
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
            summary: "citation rejection bench".to_string(),
            tags: Vec::new(),
            refs: Vec::new(),
        },
        body: body.to_string(),
        wikilinks: Vec::new(),
    };
    let content = format!(
        "---\nid: {id}\nregion: bench/citation\nsource: personal\nprivacy: private\ncreated: 2026-04-01T00:00:00Z\nupdated: 2026-04-02T00:00:00Z\nsummary: \"citation rejection bench\"\ntags: []\nrefs: []\n---\n{body}\n"
    );
    fs::write(vault.join(&rel_path), content)?;
    index.upsert_note(&note, body)?;
    Ok(())
}

/// Insert a claim over `span_text` within `body`. When `faithful` is false, the
/// stored fingerprint is deliberately computed from different text, simulating a
/// source edit after extraction (the validator must then reject the citation).
fn upsert_claim(
    store: &ClaimStore,
    id: &str,
    note_id: &str,
    body: &str,
    span_text: &str,
    faithful: bool,
) -> Result<String> {
    let span_start = body
        .find(span_text)
        .with_context(|| format!("span '{span_text}' not found in body"))?;
    let span_end = span_start + span_text.len();
    let fingerprint = if faithful {
        Claim::compute_fingerprint(span_text)
    } else {
        Claim::compute_fingerprint(&format!("{span_text} (edited)"))
    };
    let claim = Claim {
        id: id.to_string(),
        subject: "subject".to_string(),
        predicate: "predicate".to_string(),
        object: Some("object".to_string()),
        note_id: note_id.to_string(),
        span_start,
        span_end,
        span_fingerprint: fingerprint,
        valid_from: Utc
            .with_ymd_and_hms(2026, 4, 1, 0, 0, 0)
            .single()
            .expect("valid date"),
        valid_until: None,
        confidence: 1.0,
        privacy: Privacy::Private,
        extracted_by: "bench".to_string(),
        extracted_at: Utc
            .with_ymd_and_hms(2026, 4, 1, 0, 0, 0)
            .single()
            .expect("valid date"),
    };
    store.upsert(&claim)?;
    Ok(id.to_string())
}

fn claim_id_of(answer: &str) -> &str {
    let start = answer
        .find("[claim:")
        .map(|i| i + "[claim:".len())
        .unwrap_or(0);
    &answer[start..start + 16]
}

fn ratio(num: usize, den: usize) -> f32 {
    if den == 0 {
        0.0
    } else {
        num as f32 / den as f32
    }
}

fn mark(ok: bool) -> &'static str {
    if ok {
        "OK"
    } else {
        "<-- UNEXPECTED"
    }
}
