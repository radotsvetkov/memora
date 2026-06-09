use assert_cmd::Command;
use std::fs;

/// Ingesting a text file produces a valid vault note that the parser accepts,
/// tagged as a reference source and placed under the requested region.
#[test]
fn ingest_text_writes_a_valid_vault_note() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("meeting-notes.txt");
    fs::write(
        &src,
        "Drift uses MessagePack for serialization.\nThe team approved it on Tuesday.",
    )
    .expect("write source");
    let vault = temp.path().join("vault");

    Command::cargo_bin("memora")
        .expect("memora binary")
        .args(["ingest"])
        .arg(&src)
        .args(["--vault"])
        .arg(&vault)
        .args(["--region", "ingested"])
        .assert()
        .success();

    // Exactly one note landed under the region.
    let region_dir = vault.join("ingested");
    let notes: Vec<_> = fs::read_dir(&region_dir)
        .expect("region dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    assert_eq!(notes.len(), 1, "one markdown note written: {notes:?}");

    let content = fs::read_to_string(&notes[0]).expect("read note");
    assert!(
        content.contains("source: reference"),
        "tagged as reference:\n{content}"
    );
    assert!(
        content.contains("region: ingested"),
        "region preserved:\n{content}"
    );
    assert!(
        content.contains("MessagePack"),
        "body carried through:\n{content}"
    );
    assert!(
        content.contains("tags:"),
        "frontmatter has tags:\n{content}"
    );
}

/// Re-ingesting the same file updates the same note (stable id), not a duplicate.
#[test]
fn ingest_is_idempotent_for_the_same_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("doc.md");
    fs::write(&src, "First version of the document.").expect("write");
    let vault = temp.path().join("vault");

    for _ in 0..2 {
        Command::cargo_bin("memora")
            .expect("memora binary")
            .arg("ingest")
            .arg(&src)
            .args(["--vault"])
            .arg(&vault)
            .assert()
            .success();
    }

    let count = fs::read_dir(vault.join("ingested"))
        .expect("region dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .count();
    assert_eq!(
        count, 1,
        "re-ingest updates one note rather than duplicating"
    );
}

/// Without the `pdf` feature, a PDF must fail loudly with guidance, not silently.
/// (Skipped when built with `--features pdf`, where the real extractor runs.)
#[cfg(not(feature = "pdf"))]
#[test]
fn ingest_pdf_without_feature_errors_with_guidance() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("contract.pdf");
    fs::write(&src, b"%PDF-1.4 not really a pdf").expect("write");

    let assert = Command::cargo_bin("memora")
        .expect("memora binary")
        .arg("ingest")
        .arg(&src)
        .args(["--vault"])
        .arg(temp.path().join("vault"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("pdf"),
        "names the pdf feature to enable:\n{stderr}"
    );
}

/// Unsupported extensions are rejected with a clear message.
#[test]
fn ingest_rejects_unsupported_extension() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("image.png");
    fs::write(&src, b"\x89PNG").expect("write");

    let assert = Command::cargo_bin("memora")
        .expect("memora binary")
        .arg("ingest")
        .arg(&src)
        .args(["--vault"])
        .arg(temp.path().join("vault"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("unsupported"),
        "explains the rejection:\n{stderr}"
    );
}
