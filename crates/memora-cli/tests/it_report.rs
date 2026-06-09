use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use chrono::{TimeZone, Utc};
use memora_core::{
    Claim, ClaimRelation, ClaimStore, Frontmatter, Index, Note, NoteSource, Privacy, Provenance,
};
use tempfile::tempdir;

/// Seed a vault with one note + one claim whose object carries an HTML/JS
/// injection payload, so the report's escaping is exercised.
fn seed_vault(payload_object: &str) -> (tempfile::TempDir, PathBuf) {
    let temp = tempdir().expect("tempdir");
    let vault = temp.path().join("vault");
    let memora_dir = vault.join(".memora");
    fs::create_dir_all(&memora_dir).expect("mkdir");
    fs::write(
        memora_dir.join("config.toml"),
        "[embed]\nprovider = \"deterministic\"\nmodel = \"m\"\ndim = 64\n",
    )
    .expect("config");

    let body = "Widget status recorded here.";
    let rel = PathBuf::from("note.md");
    fs::write(
        vault.join(&rel),
        format!(
            "---\nid: note-1\nregion: test/unit\nsource: personal\nprivacy: private\ncreated: 2026-04-01T00:00:00Z\nupdated: 2026-04-02T00:00:00Z\nsummary: \"report test\"\ntags: []\nrefs: []\n---\n{body}\n"
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
            summary: "report test".to_string(),
            tags: Vec::new(),
            refs: Vec::new(),
        },
        body: body.to_string(),
        wikilinks: Vec::new(),
    };
    index.upsert_note(&note, body).expect("upsert note");
    let store = ClaimStore::new(&index);
    store
        .upsert(&Claim {
            id: "0123456789abcdef".to_string(),
            subject: "Widget".to_string(),
            predicate: "status_is".to_string(),
            object: Some(payload_object.to_string()),
            note_id: "note-1".to_string(),
            span_start: 0,
            span_end: body.len(),
            span_fingerprint: Claim::compute_fingerprint(body),
            valid_from: Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).single().unwrap(),
            valid_until: None,
            confidence: 1.0,
            privacy: Privacy::Private,
            extracted_by: "test".to_string(),
            extracted_at: Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).single().unwrap(),
        })
        .expect("upsert claim");

    (temp, vault)
}

// Dev helper: write a richer report to the temp dir for visual inspection.
// Run with: cargo test -p memora-cli --test it_report preview -- --ignored --nocapture
#[test]
#[ignore]
fn preview_report() {
    let temp = tempdir().expect("tempdir");
    let vault = temp.path().join("vault");
    let memora_dir = vault.join(".memora");
    fs::create_dir_all(&memora_dir).expect("mkdir");
    fs::write(
        memora_dir.join("config.toml"),
        "[embed]\nprovider = \"deterministic\"\nmodel = \"m\"\ndim = 64\n",
    )
    .expect("config");
    fs::write(
        vault.join("world_map.md"),
        "# World Map\n\n## projects/drift\n\n- Decision: serialization format is MessagePack\n- Open question: migration path for legacy blobs\n\n## projects/atlas\n\n- Contradiction: roadmap status changed pending -> approved\n",
    )
    .expect("world map");

    let index = Index::open(&memora_dir.join("memora.db")).expect("index");
    let mk_note = |id: &str, region: &str| {
        let rel = PathBuf::from(format!("{id}.md"));
        fs::write(vault.join(&rel), format!("---\nid: {id}\nregion: {region}\nsource: personal\nprivacy: private\ncreated: 2026-01-01T00:00:00Z\nupdated: 2026-02-01T00:00:00Z\nsummary: \"{id}\"\ntags: []\nrefs: []\n---\nbody for {id}\n")).unwrap();
        index
            .upsert_note(
                &Note {
                    path: rel,
                    fm: Frontmatter {
                        id: id.to_string(),
                        region: region.to_string(),
                        source: NoteSource::Personal,
                        privacy: Privacy::Private,
                        created: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap(),
                        updated: Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).single().unwrap(),
                        summary: id.to_string(),
                        tags: Vec::new(),
                        refs: Vec::new(),
                    },
                    body: format!("body for {id}"),
                    wikilinks: Vec::new(),
                },
                &format!("body for {id}"),
            )
            .unwrap();
    };
    mk_note("drift-spec", "projects/drift");
    mk_note("atlas-notes", "projects/atlas");

    let store = ClaimStore::new(&index);
    let mk_claim = |id: &str, s: &str, p: &str, o: &str, note: &str, until: Option<i32>| {
        store
            .upsert(&Claim {
                id: id.to_string(),
                subject: s.to_string(),
                predicate: p.to_string(),
                object: Some(o.to_string()),
                note_id: note.to_string(),
                span_start: 0,
                span_end: 4,
                span_fingerprint: Claim::compute_fingerprint("body"),
                valid_from: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap(),
                valid_until: until.map(|m| {
                    Utc.with_ymd_and_hms(2026, m as u32, 1, 0, 0, 0)
                        .single()
                        .unwrap()
                }),
                confidence: 1.0,
                privacy: Privacy::Private,
                extracted_by: "seed".to_string(),
                extracted_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap(),
            })
            .unwrap();
    };
    mk_claim(
        "1111111111111111",
        "Drift",
        "serialization_is",
        "MessagePack",
        "drift-spec",
        None,
    );
    mk_claim(
        "2222222222222222",
        "Drift",
        "status_is",
        "approved",
        "drift-spec",
        None,
    );
    mk_claim(
        "3333333333333333",
        "Drift",
        "status_is",
        "pending",
        "drift-spec",
        Some(2),
    );
    mk_claim(
        "4444444444444444",
        "Atlas",
        "depends_on",
        "Drift",
        "atlas-notes",
        None,
    );
    mk_claim(
        "5555555555555555",
        "Team",
        "decided",
        "ship in Q2",
        "atlas-notes",
        None,
    );
    mk_claim(
        "6666666666666666",
        "Roadmap",
        "summary_is",
        "on track",
        "atlas-notes",
        None,
    );

    store
        .add_relation(
            "2222222222222222",
            "3333333333333333",
            ClaimRelation::Contradicts,
            1.0,
        )
        .unwrap();
    store
        .add_relation(
            "2222222222222222",
            "3333333333333333",
            ClaimRelation::Supersedes,
            1.0,
        )
        .unwrap();
    let prov = Provenance::new(&index);
    prov.record(
        "6666666666666666",
        &["2222222222222222", "5555555555555555"],
    )
    .unwrap();
    prov.record("4444444444444444", &["1111111111111111"])
        .unwrap();

    let out = std::env::temp_dir().join("memora-report-preview.html");
    Command::cargo_bin("memora")
        .expect("memora binary")
        .args(["report", "--vault"])
        .arg(&vault)
        .args(["--output"])
        .arg(&out)
        .assert()
        .success();
    println!("PREVIEW_REPORT_PATH={}", out.display());
}

#[test]
fn report_generates_self_contained_html_with_sections() {
    let (temp, vault) = seed_vault("operational");
    let out = temp.path().join("report.html");

    Command::cargo_bin("memora")
        .expect("memora binary")
        .args(["report", "--vault"])
        .arg(&vault)
        .args(["--output"])
        .arg(&out)
        .assert()
        .success();

    let html = fs::read_to_string(&out).expect("read report");
    assert!(html.starts_with("<!DOCTYPE html>"), "is an HTML document");
    assert!(html.contains("memora"), "branded");
    assert!(html.contains("Claim graph"), "graph section");
    assert!(html.contains("Contradictions"), "contradictions section");
    assert!(html.contains("Stale dependencies"), "stale section");
    assert!(html.contains("World map"), "world map section");
    // Offline: no external resource fetches.
    assert!(!html.contains("http://"), "no external http resources");
    assert!(!html.contains("https://"), "no external https resources");
    assert!(!html.contains("googleapis"), "no CDN fonts");
}

#[test]
fn report_escapes_vault_content_no_injection() {
    let payload = "<script>alert('xss')</script>";
    let (temp, vault) = seed_vault(payload);
    let out = temp.path().join("report.html");

    Command::cargo_bin("memora")
        .expect("memora binary")
        .args(["report", "--vault"])
        .arg(&vault)
        .args(["--output"])
        .arg(&out)
        .assert()
        .success();

    let html = fs::read_to_string(&out).expect("read report");
    // The raw payload must never appear as live markup, neither in the body nor
    // inside the embedded graph JSON (which `\u`-escapes `<`).
    assert!(
        !html.contains("<script>alert('xss')</script>"),
        "injection payload must be neutralized"
    );
    // The escaped forms are expected instead.
    assert!(
        html.contains("&lt;script&gt;") || html.contains("\\u003cscript"),
        "payload should appear only in escaped form"
    );
    // Sanity: only our two intended <script ...> tags exist (graph-data + the JS).
    let script_opens = html.matches("<script").count();
    assert_eq!(script_opens, 2, "exactly the two intended script tags");
}
