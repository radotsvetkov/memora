use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use chrono::{TimeZone, Utc};
use tempfile::tempdir;

use memora_core::{Claim, ClaimStore, Frontmatter, Index, Memora, Note, NoteSource, Privacy};

/// Seed a vault on disk (note file + index + one claim), then drive it entirely
/// through the owned `Memora` facade. This is the embeddable API external code
/// would use, so the test deliberately touches no lifetime-borrowed internals.
#[tokio::test]
async fn facade_opens_a_vault_and_verifies_citations() -> Result<()> {
    let temp = tempdir()?;
    let vault = temp.path().join("vault");
    let memora_dir = vault.join(".memora");
    fs::create_dir_all(&memora_dir)?;

    // Pin a local embedder so the facade does not depend on a developer's global
    // config (e.g. an Ollama embedder with a different dimension).
    fs::write(
        memora_dir.join("config.toml"),
        "[embed]\nprovider = \"deterministic\"\nmodel = \"memora/deterministic\"\ndim = 64\n",
    )?;

    let body = "Rado works at HMC and leads Memora.";
    let rel = PathBuf::from("note.md");
    fs::write(
        vault.join(&rel),
        format!(
            "---\nid: note-1\nregion: test/unit\nsource: personal\nprivacy: private\ncreated: 2026-04-01T00:00:00Z\nupdated: 2026-04-02T00:00:00Z\nsummary: \"facade test\"\ntags: []\nrefs: []\n---\n{body}\n"
        ),
    )?;

    // Seed the same on-disk index the facade will open, then drop our handle.
    {
        let index = Index::open(&memora_dir.join("memora.db"))?;
        let note = Note {
            path: rel.clone(),
            fm: Frontmatter {
                id: "note-1".to_string(),
                region: "test/unit".to_string(),
                source: NoteSource::Personal,
                privacy: Privacy::Private,
                created: Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).single().unwrap(),
                updated: Utc.with_ymd_and_hms(2026, 4, 2, 0, 0, 0).single().unwrap(),
                summary: "facade test".to_string(),
                tags: Vec::new(),
                refs: Vec::new(),
            },
            body: body.to_string(),
            wikilinks: Vec::new(),
        };
        index.upsert_note(&note, body)?;
        let store = ClaimStore::new(&index);
        let span = "Rado works at HMC";
        let start = body.find(span).unwrap();
        store.upsert(&Claim {
            id: "0123456789abcdef".to_string(),
            subject: "Rado".to_string(),
            predicate: "works_at".to_string(),
            object: Some("HMC".to_string()),
            note_id: "note-1".to_string(),
            span_start: start,
            span_end: start + span.len(),
            span_fingerprint: Claim::compute_fingerprint(span),
            valid_from: Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).single().unwrap(),
            valid_until: None,
            confidence: 1.0,
            privacy: Privacy::Private,
            extracted_by: "test".to_string(),
            extracted_at: Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).single().unwrap(),
        })?;
    }

    // Everything below uses only the public, owned facade.
    let memora = Memora::open(&vault)?;
    assert_eq!(memora.vault_root(), vault.as_path());
    assert_eq!(memora.config().embed.dim, 64);

    // A faithful citation verifies.
    let ok = memora
        .validate("Rado works at HMC [claim:0123456789abcdef].")
        .await?;
    assert_eq!(ok.verified_count, 1);
    assert_eq!(ok.unverified_count, 0);

    // A hallucinated claim id is rejected.
    let bad = memora
        .validate("Pigs can fly [claim:00000000deadbeef].")
        .await?;
    assert_eq!(bad.verified_count, 0);
    assert_eq!(bad.unverified_count, 1);
    assert!(!bad.clean_text.contains("00000000deadbeef"));

    // Claims are fetchable by id.
    let claim = memora.claim("0123456789abcdef")?.expect("claim exists");
    assert_eq!(claim.subject, "Rado");
    assert!(memora.claim("00000000deadbeef")?.is_none());

    Ok(())
}
