use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use clap::Args;
use memora_core::answer::AnsweringPipeline;
use memora_core::cite::CitationValidator;
use memora_core::claims::ClaimStore;
use memora_core::{Embedder, HybridRetriever, Index, PrivacyConfig, PrivacyFilter, VectorIndex};
use memora_llm::{make_client, LlmProvider};

#[derive(Debug, Args)]
pub struct QueryArgs {
    #[arg(value_name = "text")]
    pub text: String,
    #[arg(long, default_value_t = 8)]
    pub k: usize,
    #[arg(long, default_value = "vault")]
    pub vault_root: PathBuf,
    #[arg(long, default_value = ".memora/memora.db")]
    pub index_db: PathBuf,
    #[arg(long, default_value = ".memora/vectors")]
    pub vector_index: PathBuf,
}

pub async fn run(args: QueryArgs) -> Result<()> {
    let index = Index::open(&args.index_db)?;
    let store = ClaimStore::new(&index);
    let validator = CitationValidator {
        store: &store,
        index: &index,
        vault_root: &args.vault_root,
    };
    let embedder = DeterministicCliEmbedder::new(64);
    let vector_index = VectorIndex::open_or_create(&args.vector_index, embedder.dim())?;
    let retriever = HybridRetriever {
        index: &index,
        vec: &vector_index,
        embedder: &embedder,
    };
    let llm = make_client(LlmProvider::Ollama, None)?;
    let pipeline = AnsweringPipeline {
        retriever: &retriever,
        claim_store: &store,
        validator: &validator,
        llm: llm.as_ref(),
        privacy_filter: PrivacyFilter::new_for(LlmProvider::Ollama),
        privacy_config: PrivacyConfig::default(),
    };
    let answer = pipeline.answer(&args.text, args.k).await?;
    println!("{}", answer.clean_text);
    println!(
        "\nVerified: {} · Unverified: {} · Mismatches: {}",
        answer.verified_count, answer.unverified_count, answer.mismatch_count
    );
    Ok(())
}

struct DeterministicCliEmbedder {
    dim: usize,
}

impl DeterministicCliEmbedder {
    fn new(dim: usize) -> Self {
        Self { dim }
    }
}

#[async_trait]
impl Embedder for DeterministicCliEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for text in texts {
            let mut bytes = blake3::hash(text.as_bytes()).as_bytes().to_vec();
            while bytes.len() < self.dim * 4 {
                let next = blake3::hash(&bytes).as_bytes().to_vec();
                bytes.extend_from_slice(&next);
            }
            let mut vector = Vec::with_capacity(self.dim);
            for chunk in bytes.chunks_exact(4).take(self.dim) {
                let bits = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                vector.push((bits as f32 / u32::MAX as f32) * 2.0 - 1.0);
            }
            out.push(vector);
        }
        Ok(out)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        "cli/deterministic"
    }
}
