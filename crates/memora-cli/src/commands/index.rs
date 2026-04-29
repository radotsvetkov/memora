use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::Args;
use memora_core::claims::{ClaimExtractor, ClaimStore};
use memora_core::indexer::Indexer;
use memora_llm::{make_client, LlmProvider};

use crate::config::AppConfig;
use crate::runtime::{build_embedder, open_index, open_vault, open_vector};

#[derive(Debug, Args)]
pub struct IndexArgs {
    #[arg(long, default_value = "vault")]
    pub vault: std::path::PathBuf,
}

pub async fn run(args: IndexArgs) -> Result<()> {
    let cfg = AppConfig::load(&args.vault)?;
    let vault = open_vault(&args.vault);
    let index = open_index(&args.vault)?;
    let vector = open_vector(&args.vault, &cfg.embed)?;
    let embedder = build_embedder(&cfg.embed);
    let provider = match cfg.llm.provider.as_str() {
        "anthropic" => LlmProvider::Anthropic,
        "openai" => LlmProvider::OpenAi,
        _ => LlmProvider::Ollama,
    };
    let llm = make_client(provider, cfg.llm.model.clone())?;
    let claim_store = ClaimStore::new(&index);
    let claim_extractor = ClaimExtractor {
        llm: llm.as_ref(),
        model_label: llm.model_name().to_string(),
    };
    let indexer = Indexer::new(&vault, &index, embedder, Arc::new(Mutex::new(vector)))
        .with_claims(claim_extractor, claim_store);
    let stats = indexer.full_rebuild().await?;
    println!(
        "Indexed: inserted={}, skipped={}, errors={}",
        stats.inserted, stats.skipped, stats.errors
    );
    Ok(())
}
