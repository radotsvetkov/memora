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

    /// Ollama model name used for `/api/embeddings` (for diagnostics and tests).
    pub fn embedding_model_name(&self) -> String {
        self.client.resolved_embedding_model()
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use memora_llm::OllamaClient;

    use super::OllamaEmbedder;

    #[test]
    fn embedder_uses_explicit_embedding_model_not_chat_model() {
        let client = Arc::new(
            OllamaClient::new(
                Some("qwen2.5:14b-instruct-q5_K_M".into()),
                None,
                Some("nomic-embed-text".into()),
            )
            .expect("client"),
        );
        let embedder = OllamaEmbedder::new(client, 768);
        assert_eq!(embedder.embedding_model_name(), "nomic-embed-text");
        assert_ne!(
            embedder.embedding_model_name(),
            "qwen2.5:14b-instruct-q5_K_M"
        );
    }
}
