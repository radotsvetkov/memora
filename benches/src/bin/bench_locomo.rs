use std::fs;
use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct LocomoSample {
    query: String,
    expected_note_id: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("locomo_subset.jsonl");
    let content = fs::read_to_string(&fixture_path)?;
    let mut rows = Vec::new();
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        rows.push(serde_json::from_str::<LocomoSample>(line)?);
    }
    // NOTE: this binary only lists the fixture queries. It does NOT score
    // retrieval — there is no indexed corpus to retrieve against here, so any
    // retrieval@k value would be fabricated. (A previous version hardcoded
    // retrieval@k = 1.0 for any non-empty fixture; that has been removed.)
    // To actually score LoCoMo, build an index from the source corpus and run
    // the real retriever over each query, comparing against expected_note_id.
    println!("LoCoMo subset rows: {} (fixture listing only; not scored)", rows.len());
    println!("queries:");
    for row in rows {
        println!("- {} => {}", row.query, row.expected_note_id);
    }
    Ok(())
}
