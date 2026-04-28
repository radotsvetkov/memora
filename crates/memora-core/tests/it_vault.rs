use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use memora_core::note::parse;
use memora_core::scan;

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/sample-vault")
        .canonicalize()
        .expect("fixture vault should exist")
}

#[test]
fn scan_finds_expected_fixture_paths() {
    let root = fixture_root();
    let found: BTreeSet<_> = scan(&root)
        .map(|path| {
            path.strip_prefix(&root)
                .expect("path should be inside fixture root")
                .to_path_buf()
        })
        .collect();

    let expected = BTreeSet::from([
        PathBuf::from("world_map.md"),
        PathBuf::from("work/_atlas.md"),
        PathBuf::from("work/internorga.md"),
        PathBuf::from("personal/example.md"),
    ]);
    assert_eq!(found, expected);
}

#[test]
fn parsing_all_scanned_notes_succeeds_and_aggregates_wikilinks() {
    let root = fixture_root();
    let mut links = BTreeSet::new();

    for path in scan(&root) {
        let parsed = parse(&path).expect("scanned fixture note should parse");
        for link in parsed.wikilinks {
            links.insert(link);
        }
    }

    let expected_links = BTreeSet::from([
        "_atlas".to_string(),
        "example".to_string(),
        "internorga".to_string(),
        "world-map".to_string(),
    ]);
    assert_eq!(links, expected_links);
}
