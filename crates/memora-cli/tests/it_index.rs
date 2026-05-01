use std::fs;

use assert_cmd::Command;
use rusqlite::Connection;
use tempfile::tempdir;

#[test]
fn index_emits_per_note_errors_via_tracing() {
    let temp = tempdir().expect("create tempdir");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let valid_note = vault.join("valid.md");
    fs::write(
        &valid_note,
        r#"---
id: valid
region: default
source: reference
privacy: private
created: 2026-04-29T08:19:00Z
updated: 2026-04-29T08:20:00Z
summary: "Valid reference note"
---
Body
"#,
    )
    .expect("write valid note");

    let broken_note = vault.join("broken.md");
    fs::write(
        &broken_note,
        r#"---
id: broken
region: default
source: reference
privacy: private
created: 2026-04-29T08:19:00Z
updated: 2026-04-29T08:20:00Z
summary: "Broken note":
---
Body
"#,
    )
    .expect("write broken note");

    let assert = Command::cargo_bin("memora")
        .expect("build memora binary")
        .env("RUST_LOG", "warn")
        .arg("index")
        .arg("--vault")
        .arg(&vault)
        .arg("--no-auto-fix-frontmatter")
        .assert()
        .success();
    let output = assert.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stdout.contains("errors=1"),
        "expected one indexing error in summary, got stdout: {stdout}"
    );
    assert!(
        stderr.contains("failed to index note"),
        "expected per-note error log in stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("broken.md"),
        "expected broken note path in stderr, got: {stderr}"
    );

    let db_path = vault.join(".memora").join("memora.db");
    let conn = Connection::open(db_path).expect("open index db");
    let note_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))
        .expect("count notes");
    assert_eq!(note_count, 1, "expected only valid note to be indexed");
}
