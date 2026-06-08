use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use chrono::{TimeZone, Utc};
use memora_core::{Claim, ClaimStore, Frontmatter, Index, Note, NoteSource, Privacy};
use tempfile::tempdir;

/// Seed a vault with one verifiable claim so the `memora verify` binary has
/// something real to check.
fn seed_vault() -> (tempfile::TempDir, PathBuf) {
    let temp = tempdir().expect("tempdir");
    let vault = temp.path().join("vault");
    let memora_dir = vault.join(".memora");
    fs::create_dir_all(&memora_dir).expect("mkdir");
    fs::write(
        memora_dir.join("config.toml"),
        "[embed]\nprovider = \"deterministic\"\nmodel = \"m\"\ndim = 64\n",
    )
    .expect("config");

    let body = "Rado works at HMC and leads Memora.";
    let rel = PathBuf::from("note.md");
    fs::write(
        vault.join(&rel),
        format!(
            "---\nid: note-1\nregion: test/unit\nsource: personal\nprivacy: private\ncreated: 2026-04-01T00:00:00Z\nupdated: 2026-04-02T00:00:00Z\nsummary: \"verify test\"\ntags: []\nrefs: []\n---\n{body}\n"
        ),
    )
    .expect("note");

    let index = Index::open(&memora_dir.join("memora.db")).expect("index");
    let note = Note {
        path: rel,
        fm: Frontmatter {
            id: "note-1".to_string(),
            region: "test/unit".to_string(),
            source: NoteSource::Personal,
            privacy: Privacy::Private,
            created: Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).single().unwrap(),
            updated: Utc.with_ymd_and_hms(2026, 4, 2, 0, 0, 0).single().unwrap(),
            summary: "verify test".to_string(),
            tags: Vec::new(),
            refs: Vec::new(),
        },
        body: body.to_string(),
        wikilinks: Vec::new(),
    };
    index.upsert_note(&note, body).expect("upsert note");
    let store = ClaimStore::new(&index);
    let span = "Rado works at HMC";
    let start = body.find(span).unwrap();
    store
        .upsert(&Claim {
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
        })
        .expect("upsert claim");

    let vault_path = vault.clone();
    (temp, vault_path)
}

fn write_answer(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, contents).expect("write answer");
    p
}

#[test]
fn verify_passes_a_faithful_citation() {
    let (temp, vault) = seed_vault();
    let answer = write_answer(
        temp.path(),
        "ok.txt",
        "Rado works at HMC [claim:0123456789abcdef].",
    );
    let assert = Command::cargo_bin("memora")
        .unwrap()
        .args(["verify", "--vault"])
        .arg(&vault)
        .arg(&answer)
        .env("NO_COLOR", "1")
        .assert()
        .success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        out.contains("VERIFIED"),
        "expected a verified citation: {out}"
    );
}

#[test]
fn verify_fails_a_fabricated_citation() {
    let (temp, vault) = seed_vault();
    let answer = write_answer(
        temp.path(),
        "bad.txt",
        "Pigs can fly [claim:00000000deadbeef].",
    );
    let assert = Command::cargo_bin("memora")
        .unwrap()
        .args(["verify", "--vault"])
        .arg(&vault)
        .arg(&answer)
        .env("NO_COLOR", "1")
        .assert()
        .failure();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(out.contains("REJECTED"), "expected a rejection: {out}");
}

#[test]
fn verify_json_reports_problem_count() {
    let (temp, vault) = seed_vault();
    let answer = write_answer(temp.path(), "bad.txt", "Nope [claim:00000000deadbeef].");
    let assert = Command::cargo_bin("memora")
        .unwrap()
        .args(["verify", "--json", "--vault"])
        .arg(&vault)
        .arg(&answer)
        .assert()
        .failure();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(v["problems"], 1);
    assert_eq!(v["unverified"], 1);
}
