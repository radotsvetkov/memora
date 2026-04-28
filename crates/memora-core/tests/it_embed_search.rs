use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use memora_core::indexer::Indexer;
use memora_core::{Embedder, Index, Vault, VectorIndex};
use tempfile::tempdir;

fn write_note(path: &Path, id: &str, summary: &str, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = format!(
        r#"---
id: {id}
region: test/integration
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-02T00:00:00Z
summary: "{summary}"
tags: []
refs: []
---
{body}
"#
    );
    fs::write(path, content)?;
    Ok(())
}

struct HashEmbedder {
    dim: usize,
    model_id: String,
}

impl HashEmbedder {
    fn new(dim: usize) -> Self {
        Self {
            dim,
            model_id: "test/hash-embedder".to_string(),
        }
    }

    fn embed_one(&self, text: &str) -> Vec<f32> {
        let mut vec = vec![0.0f32; self.dim];
        for token in text.split_whitespace() {
            let hash = blake3::hash(token.as_bytes());
            let bytes = hash.as_bytes();
            let idx = (u16::from_le_bytes([bytes[0], bytes[1]]) as usize) % self.dim;
            let sign = if bytes[2] % 2 == 0 { 1.0 } else { -1.0 };
            let weight = (bytes[3] as f32 / 255.0) + 0.5;
            vec[idx] += sign * weight;
        }
        let norm = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut vec {
                *v /= norm;
            }
        }
        vec
    }
}

#[async_trait]
impl Embedder for HashEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|text| self.embed_one(text)).collect())
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

#[tokio::test]
async fn vector_search_returns_note_in_top3() -> Result<()> {
    let temp = tempdir()?;
    let root = temp.path().join("vault");
    write_note(
        &root.join("alpha.md"),
        "note-alpha",
        "Astronomy notes about nebula clusters",
        "nebula star chart observations and stellar catalog",
    )?;
    write_note(
        &root.join("beta.md"),
        "note-beta",
        "Cooking recipe checklist",
        "bread flour water fermentation notes",
    )?;
    write_note(
        &root.join("gamma.md"),
        "note-gamma",
        "Travel itinerary planning",
        "airports trains hotels and local maps",
    )?;
    write_note(
        &root.join("delta.md"),
        "note-delta",
        "Astronomy instrument setup",
        "telescope focus lens cleaning and nebula timing",
    )?;

    let index = Index::open(&temp.path().join("index").join("memora.db"))?;
    let vault = Vault::new(&root);
    let embedder: Arc<dyn Embedder> = Arc::new(HashEmbedder::new(128));
    let vector_index = Arc::new(Mutex::new(VectorIndex::open_or_create(
        &temp.path().join("index").join("vectors"),
        embedder.dim(),
    )?));
    let indexer = Indexer::new(&vault, &index, embedder, vector_index);
    indexer.full_rebuild().await?;

    let results = indexer
        .vector_search("astronomy notes about nebula clusters", 3)
        .await?;
    assert_eq!(results.len(), 3);
    assert!(results.iter().any(|(id, _)| id == "note-alpha"));
    Ok(())
}
