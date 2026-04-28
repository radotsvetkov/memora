use anyhow::{anyhow, bail, Context, Result};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use super::Embedder;

const API_KEY_ENV: &str = "OPENAI_API_KEY";
const MODEL_ENV: &str = "OPENAI_EMBED_MODEL";
const DEFAULT_MODEL: &str = "text-embedding-3-small";
const DEFAULT_DIM: usize = 1_536;
const MAX_BATCH_SIZE: usize = 100;
const DEFAULT_BASE_URL: &str = "https://api.openai.com";

pub struct OpenAiEmbedder {
    http: reqwest::Client,
    api_key: SecretString,
    model: String,
}

impl OpenAiEmbedder {
    pub fn new() -> Result<Self> {
        let api_key = std::env::var(API_KEY_ENV)
            .with_context(|| format!("missing required environment variable {API_KEY_ENV}"))?;
        let model = std::env::var(MODEL_ENV).unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        Ok(Self {
            http: reqwest::Client::new(),
            api_key: SecretString::new(api_key),
            model,
        })
    }

    fn endpoint() -> String {
        let base =
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        format!("{base}/v1/embeddings")
    }
}

#[async_trait::async_trait]
impl Embedder for OpenAiEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(MAX_BATCH_SIZE) {
            let payload = EmbeddingRequest {
                model: self.model.clone(),
                input: batch.to_vec(),
            };

            let response = self
                .http
                .post(Self::endpoint())
                .bearer_auth(self.api_key.expose_secret())
                .json(&payload)
                .send()
                .await
                .context("send embeddings request")?;
            let status = response.status();
            let raw = response
                .text()
                .await
                .context("read embeddings response body")?;

            if status.as_u16() == 401 || status.as_u16() == 403 {
                bail!("openai embeddings auth failed: {raw}");
            }
            if !status.is_success() {
                bail!(
                    "openai embeddings request failed ({}): {raw}",
                    status.as_u16()
                );
            }

            let parsed: EmbeddingResponse =
                serde_json::from_str(&raw).context("parse embeddings response json")?;
            let mut rows = parsed.data;
            rows.sort_by_key(|row| row.index);
            if rows.len() != batch.len() {
                return Err(anyhow!(
                    "openai embeddings length mismatch: expected {}, got {}",
                    batch.len(),
                    rows.len()
                ));
            }
            out.extend(rows.into_iter().map(|row| row.embedding));
        }

        Ok(out)
    }

    fn dim(&self) -> usize {
        DEFAULT_DIM
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingDataRow>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingDataRow {
    index: usize,
    embedding: Vec<f32>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::await_holding_lock)]

    use std::sync::{Mutex, OnceLock};

    use mockito::{Matcher, Server};

    use super::*;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[tokio::test]
    async fn embed_happy_path_parses_vectors() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut server = Server::new_async().await;
        std::env::set_var("OPENAI_BASE_URL", server.url());
        std::env::set_var("OPENAI_API_KEY", "test-key");
        std::env::set_var("OPENAI_EMBED_MODEL", "text-embedding-3-small");

        let _mock = server
            .mock("POST", "/v1/embeddings")
            .match_header("authorization", "Bearer test-key")
            .match_body(Matcher::AllOf(vec![
                Matcher::Regex(r#""model":"text-embedding-3-small""#.to_string()),
                Matcher::Regex(r#""input":\["alpha","beta"\]"#.to_string()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                    "data": [
                        {"index": 0, "embedding": [1.0, 0.0]},
                        {"index": 1, "embedding": [0.0, 1.0]}
                    ]
                }"#,
            )
            .create();

        let embedder = OpenAiEmbedder::new().expect("construct openai embedder");
        let vectors = embedder
            .embed(&["alpha".to_string(), "beta".to_string()])
            .await
            .expect("embed request should succeed");
        assert_eq!(vectors.len(), 2);
        assert_eq!(vectors[0], vec![1.0, 0.0]);
        assert_eq!(vectors[1], vec![0.0, 1.0]);
    }

    #[tokio::test]
    async fn embed_returns_error_on_401() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut server = Server::new_async().await;
        std::env::set_var("OPENAI_BASE_URL", server.url());
        std::env::set_var("OPENAI_API_KEY", "bad-key");

        let _mock = server
            .mock("POST", "/v1/embeddings")
            .with_status(401)
            .with_body("unauthorized")
            .create();

        let embedder = OpenAiEmbedder::new().expect("construct openai embedder");
        let err = embedder
            .embed(&["alpha".to_string()])
            .await
            .expect_err("401 should be reported");
        assert!(err.to_string().contains("auth failed"));
    }
}
