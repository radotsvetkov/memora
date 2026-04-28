use std::path::PathBuf;

use anyhow::Result;
use clap::Args;
use memora_core::{Challenger, ChallengerConfig, ClaimStore, Index};
use memora_llm::{make_client, LlmProvider};

#[derive(Debug, Args)]
pub struct ChallengeArgs {
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
    #[arg(long, default_value = "vault")]
    pub vault_root: PathBuf,
    #[arg(long, default_value = ".memora/memora.db")]
    pub index_db: PathBuf,
}

pub async fn run(args: ChallengeArgs) -> Result<()> {
    let index = Index::open(&args.index_db)?;
    let claim_store = ClaimStore::new(&index);
    let llm = make_client(LlmProvider::Ollama, None)?;
    let challenger = Challenger {
        db: &index,
        claim_store: &claim_store,
        llm: llm.as_ref(),
        vault: &args.vault_root,
        config: ChallengerConfig::default(),
    };

    let report = challenger.run_once().await?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !args.dry_run {
        challenger.persist_report(&report)?;
        println!("Persisted report to world_map.md and .memora/last_challenger.json");
    }
    Ok(())
}
