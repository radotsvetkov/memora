use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use anyhow::Result;
use uuid::Uuid;

use crate::embed::Embedder;
use crate::index::{Index, VectorIndex};
use crate::retrieve::hebbian::HebbianLearner;
use crate::retrieve::spread::spread;

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalHit {
    pub id: String,
    pub score: f32,
    pub source: HitSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitSource {
    Bm25,
    Vector,
    Both,
}

pub struct HybridRetriever<'a> {
    pub index: &'a Index,
    pub vec: &'a VectorIndex,
    pub embedder: &'a dyn Embedder,
}

impl<'a> HybridRetriever<'a> {
    pub async fn search(&self, query: &str, k: usize) -> Result<Vec<RetrievalHit>> {
        if k == 0 {
            return Ok(Vec::new());
        }

        let bm25 = self.index.bm25_search(query, 50)?;
        let q_vec = self.embedder.embed(&[query.to_string()]).await?[0].clone();
        let vec_hits = self.vec.search(&q_vec, 50)?;

        let mut scores = HashMap::<String, f32>::new();
        let mut bm25_seen = HashSet::<String>::new();
        let mut vec_seen = HashSet::<String>::new();

        for (rank, (id, _)) in bm25.iter().enumerate() {
            let rank_1 = (rank + 1) as f32;
            *scores.entry(id.clone()).or_insert(0.0) += 1.0 / (60.0 + rank_1);
            bm25_seen.insert(id.clone());
        }
        for (rank, (id, _)) in vec_hits.iter().enumerate() {
            let rank_1 = (rank + 1) as f32;
            *scores.entry(id.clone()).or_insert(0.0) += 1.0 / (60.0 + rank_1);
            vec_seen.insert(id.clone());
        }

        let mut fused = scores
            .into_iter()
            .map(|(id, score)| RetrievalHit {
                source: match (bm25_seen.contains(&id), vec_seen.contains(&id)) {
                    (true, true) => HitSource::Both,
                    (true, false) => HitSource::Bm25,
                    (false, true) => HitSource::Vector,
                    (false, false) => HitSource::Both,
                },
                id,
                score,
            })
            .collect::<Vec<_>>();

        fused.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        fused.truncate(k);
        Ok(fused)
    }

    pub async fn search_with_spread_and_record(
        &self,
        query: &str,
        k: usize,
        max_extra: usize,
    ) -> Result<(String, Vec<RetrievalHit>)> {
        let seeds = self.search(query, k).await?;
        let mut expanded = spread(&seeds, self.index, max_extra)?;

        for hit in &mut expanded {
            let qvalue = self.index.qvalue(&hit.id)?.unwrap_or(0.0);
            hit.score += 0.1 * qvalue;
        }
        expanded.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });

        let final_ids = expanded
            .iter()
            .map(|hit| hit.id.clone())
            .collect::<Vec<_>>();
        let final_refs = final_ids.iter().map(String::as_str).collect::<Vec<_>>();
        HebbianLearner::new(self.index).record_coactivation(&final_refs)?;

        let query_id = Uuid::new_v4().to_string();
        self.index.record_retrieval(&query_id, query, &final_ids)?;

        Ok((query_id, expanded))
    }
}
