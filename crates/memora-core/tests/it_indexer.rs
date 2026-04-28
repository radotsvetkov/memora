use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use memora_core::indexer::Indexer;
use memora_core::{note, Index, Vault};
use tempfile::tempdir;

fn write_note(path: &Path, id: &str, summary: &str, tags: &[&str], body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tags_line = tags
        .iter()
        .map(|tag| format!("\"{tag}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let content = format!(
        r#"---
id: {id}
region: test/integration
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-02T00:00:00Z
summary: "{summary}"
tags: [{tags_line}]
refs: []
---
{body}
"#
    );
    fs::write(path, content)?;
    Ok(())
}

fn setup_vault(root: &Path) -> Result<Vec<PathBuf>> {
    let paths = vec![
        root.join("alpha.md"),
        root.join("work/beta.md"),
        root.join("work/gamma.md"),
        root.join("personal/delta.md"),
        root.join("personal/epsilon.md"),
    ];

    write_note(
        &paths[0],
        "note-alpha",
        "Alpha summary about astronomy",
        &["alpha", "space"],
        "alpha body with comet nebula and star chart",
    )?;
    write_note(
        &paths[1],
        "note-beta",
        "Beta summary",
        &["beta"],
        "beta body references alpha concepts",
    )?;
    write_note(
        &paths[2],
        "note-gamma",
        "Gamma summary",
        &["gamma"],
        "gamma body and project notes",
    )?;
    write_note(
        &paths[3],
        "note-delta",
        "Delta summary",
        &["delta"],
        "delta body with routine updates",
    )?;
    write_note(
        &paths[4],
        "note-epsilon",
        "Epsilon summary with comet focus",
        &["epsilon", "space"],
        "comet comet comet trajectory and observations",
    )?;

    Ok(paths)
}

#[test]
fn full_rebuild_and_reindex_flow() -> Result<()> {
    let temp = tempdir()?;
    let vault_root = temp.path().join("vault");
    let note_paths = setup_vault(&vault_root)?;

    let index_path = temp.path().join("index").join("memora.db");
    let index = Index::open(&index_path)?;
    let vault = Vault::new(&vault_root);
    let indexer = Indexer {
        vault: &vault,
        index: &index,
    };

    let stats = indexer.full_rebuild()?;
    assert_eq!(stats.inserted, 5);
    assert_eq!(stats.errors, 0);

    let before = index
        .get_note("note-epsilon")?
        .expect("epsilon should be indexed before update");
    let updated_content = r#"---
id: note-epsilon
region: test/integration
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-03T00:00:00Z
summary: "Epsilon summary with comet focus"
tags: ["epsilon", "space"]
refs: []
---
comet comet comet comet trajectory and observations updated
"#;
    fs::write(&note_paths[4], updated_content)?;

    let reparsed = note::parse(&note_paths[4])?;
    index.upsert_note(&reparsed, &reparsed.body)?;

    let after = index
        .get_note("note-epsilon")?
        .expect("epsilon should be indexed after update");
    assert_ne!(before.body_hash, after.body_hash);

    let results = index.bm25_search("comet", 5)?;
    assert!(!results.is_empty());
    assert_eq!(results[0].0, "note-epsilon");

    Ok(())
}
