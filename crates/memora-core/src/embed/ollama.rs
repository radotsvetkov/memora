use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use memora_llm::OllamaClient;

use super::Embedder;

/// Embeddings via Ollama `/api/embeddings` using [`OllamaClient::resolved_embedding_model`].
pub struct OllamaEmbedder {
    client: Arc<OllamaClient>,
    dim: usize,
}

impl OllamaEmbedder {
    pub fn new(client: Arc<OllamaClient>, dim: usize) -> Self {
        Self { client, dim }
    }
}

#[async_trait]
impl Embedder for OllamaEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for text in texts {
            let vec = self
                .client
                .embed_one(text)
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string()))
                .context("ollama embedding request failed")?;
            if vec.len() != self.dim {
                anyhow::bail!(
                    "embedding dimension mismatch: got {} floats but [embed].dim is {}; adjust embed.dim to match {}",
                    vec.len(),
                    self.dim,
                    self.client.resolved_embedding_model()
                );
            }
            out.push(vec);
        }
        Ok(out)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        "ollama/embeddings"
    }
}
