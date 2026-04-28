use std::path::PathBuf;

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use super::Embedder;

const DEFAULT_DIM: usize = 384;
const DEFAULT_MODEL_ID: &str = "BAAI/bge-small-en-v1.5";

pub struct LocalEmbedder {
    model: TextEmbedding,
    model_id: String,
}

impl LocalEmbedder {
    pub fn new() -> Result<Self> {
        let cache_dir = cache_dir()?;
        std::fs::create_dir_all(&cache_dir)
            .with_context(|| format!("create local embedding cache dir {}", cache_dir.display()))?;

        let options = InitOptions::new(EmbeddingModel::BGESmallENV15).with_cache_dir(cache_dir);
        let model = TextEmbedding::try_new(options).context("initialize local embedding model")?;
        Ok(Self {
            model,
            model_id: DEFAULT_MODEL_ID.to_string(),
        })
    }
}

#[async_trait::async_trait]
impl Embedder for LocalEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        self.model
            .embed(texts.to_vec(), None)
            .context("generate local embeddings")
    }

    fn dim(&self) -> usize {
        DEFAULT_DIM
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

fn cache_dir() -> Result<PathBuf> {
    if let Ok(xdg_cache) = std::env::var("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(xdg_cache).join("memora"));
    }
    let home = std::env::var("HOME").context("HOME is not set for local embedder cache")?;
    Ok(PathBuf::from(home).join(".cache").join("memora"))
}

#[cfg(all(test, feature = "local-embed"))]
mod tests {
    use super::LocalEmbedder;
    use crate::embed::Embedder;

    #[tokio::test]
    async fn local_embedder_produces_vectors() {
        let embedder = LocalEmbedder::new().expect("initialize local embedder");
        let vectors = embedder
            .embed(&["hello world".to_string()])
            .await
            .expect("embed with local model");
        assert_eq!(vectors.len(), 1);
        assert_eq!(vectors[0].len(), embedder.dim());
    }
}
