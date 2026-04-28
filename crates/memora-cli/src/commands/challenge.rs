use anyhow::Result;
use clap::Args;
use memora_core::{Challenger, ChallengerConfig, ClaimStore};
use memora_llm::{make_client, LlmProvider};

use crate::config::AppConfig;
use crate::runtime::open_index;

#[derive(Debug, Args)]
pub struct ChallengeArgs {
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
    #[arg(long, default_value = "vault")]
    pub vault: std::path::PathBuf,
}

pub async fn run(args: ChallengeArgs) -> Result<()> {
    let cfg = AppConfig::load(&args.vault)?;
    let index = open_index(&args.vault)?;
    let claim_store = ClaimStore::new(&index);
    let provider = match cfg.llm.provider.as_str() {
        "anthropic" => LlmProvider::Anthropic,
        "openai" => LlmProvider::OpenAi,
        _ => LlmProvider::Ollama,
    };
    let llm = make_client(provider, cfg.llm.model.clone())?;
    let challenger = Challenger {
        db: &index,
        claim_store: &claim_store,
        llm: llm.as_ref(),
        vault: &args.vault,
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
