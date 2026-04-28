use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;
use memora_core::note;
use memora_core::{Embedder, HebbianLearner, HybridRetriever, Index, QValueLearner, VectorIndex};
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

struct KeywordEmbedder {
    model_id: String,
    token_to_dim: HashMap<String, usize>,
}

impl KeywordEmbedder {
    fn new() -> Self {
        let dims = [
            "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "shared", "topic",
        ];
        let token_to_dim = dims
            .iter()
            .enumerate()
            .map(|(idx, token)| (token.to_string(), idx))
            .collect::<HashMap<_, _>>();
        Self {
            model_id: "test/keyword-embedder".to_string(),
            token_to_dim,
        }
    }

    fn embed_one(&self, text: &str) -> Vec<f32> {
        let mut out = vec![0.0_f32; self.token_to_dim.len()];
        for raw in text.split_whitespace() {
            let token = raw.to_lowercase();
            if let Some(idx) = self.token_to_dim.get(&token) {
                out[*idx] += 1.0;
            }
        }
        let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for value in &mut out {
                *value /= norm;
            }
        }
        out
    }
}

#[async_trait]
impl Embedder for KeywordEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|text| self.embed_one(text)).collect())
    }

    fn dim(&self) -> usize {
        self.token_to_dim.len()
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

#[tokio::test]
async fn hybrid_search_records_hebbian_and_reinforces_qvalues() -> Result<()> {
    let temp = tempdir()?;
    let root = temp.path().join("vault");
    let notes = vec![
        ("note-alpha", "alpha topic", "alpha shared topic details"),
        ("note-beta", "beta topic", "beta shared topic details"),
        ("note-gamma", "gamma topic", "gamma exclusive details"),
        ("note-delta", "delta topic", "delta exclusive details"),
        ("note-epsilon", "epsilon topic", "epsilon exclusive details"),
        ("note-zeta", "zeta topic", "zeta exclusive details"),
    ];
    for (id, summary, body) in &notes {
        write_note(&root.join(format!("{id}.md")), id, summary, body)?;
    }

    let index = Index::open(&temp.path().join("index").join("memora.db"))?;
    let embedder = KeywordEmbedder::new();
    let mut vector_index =
        VectorIndex::open_or_create(&temp.path().join("index").join("vectors"), embedder.dim())?;

    for (id, _, _) in &notes {
        let parsed = note::parse(&root.join(format!("{id}.md")))?;
        index.upsert_note(&parsed, &parsed.body)?;
        let text = format!("{}\n{}", parsed.fm.summary, parsed.body);
        let vec = embedder
            .embed(&[text])
            .await?
            .into_iter()
            .next()
            .unwrap_or_default();
        vector_index.upsert(id, &vec)?;
    }
    vector_index.save()?;

    let retriever = HybridRetriever {
        index: &index,
        vec: &vector_index,
        embedder: &embedder,
    };

    let distinct_queries = vec![
        ("alpha", "note-alpha"),
        ("beta", "note-beta"),
        ("gamma", "note-gamma"),
        ("delta", "note-delta"),
        ("epsilon", "note-epsilon"),
    ];
    for (query, expected) in distinct_queries {
        let (_, hits) = retriever.search_with_spread_and_record(query, 1, 0).await?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, expected);
    }

    let (_, overlap_hits) = retriever
        .search_with_spread_and_record("alpha beta shared", 2, 0)
        .await?;
    let overlap_ids = overlap_hits
        .iter()
        .map(|hit| hit.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert!(overlap_ids.contains("note-alpha"));
    assert!(overlap_ids.contains("note-beta"));

    let neighbors = HebbianLearner::new(&index).neighbors("note-alpha", 10)?;
    let beta_edge = neighbors
        .into_iter()
        .find(|(id, _)| id == "note-beta")
        .expect("expected hebbian edge between alpha and beta");
    assert!(beta_edge.1 > 0.0);

    QValueLearner::new(&index).reinforce(&["note-alpha"], &["note-alpha", "note-beta"])?;
    let q_alpha = index.qvalue("note-alpha")?.unwrap_or(0.0);
    let q_beta = index.qvalue("note-beta")?.unwrap_or(0.0);
    assert!(q_alpha > 0.0);
    assert!(q_beta < 0.0);

    let (_, reinforced_hits) = retriever
        .search_with_spread_and_record("alpha beta shared", 2, 0)
        .await?;
    assert_eq!(reinforced_hits[0].id, "note-alpha");

    Ok(())
}
