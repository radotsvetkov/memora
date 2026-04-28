use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::Args;
use memora_core::indexer::Indexer;

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
    let indexer = Indexer::new(&vault, &index, embedder, Arc::new(Mutex::new(vector)));
    let stats = indexer.full_rebuild().await?;
    println!(
        "Indexed: inserted={}, skipped={}, errors={}",
        stats.inserted, stats.skipped, stats.errors
    );
    Ok(())
}
