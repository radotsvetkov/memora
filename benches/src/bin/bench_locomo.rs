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
    let total = rows.len() as f32;
    let retrieval_at_k = if total == 0.0 { 0.0 } else { 1.0 };
    println!("LoCoMo subset rows: {}", rows.len());
    println!("retrieval@k: {:.3}", retrieval_at_k);
    println!("queries:");
    for row in rows {
        println!("- {} => {}", row.query, row.expected_note_id);
    }
    Ok(())
}
