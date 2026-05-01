use anyhow::Result;
use async_trait::async_trait;

#[cfg(feature = "local-embed")]
pub mod local;
pub mod ollama;
pub mod openai;

pub use ollama::OllamaEmbedder;
pub use openai::OpenAiEmbedder;

#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    fn dim(&self) -> usize;
    fn model_id(&self) -> &str;
}

pub fn normalize_text(s: &str) -> String {
    s.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::normalize_text;

    #[test]
    fn normalize_text_lowercases_and_collapses_whitespace() {
        let normalized = normalize_text("  Hello\tWORLD\n  from   Memora ");
        assert_eq!(normalized, "hello world from memora");
    }
}
