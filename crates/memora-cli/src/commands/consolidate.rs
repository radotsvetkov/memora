use anyhow::Result;
use clap::Args;
use memora_core::{AtlasWriter, ClaimStore, WorldMapWriter};
use memora_llm::{make_client, LlmProvider};
use std::collections::BTreeSet;

use crate::config::AppConfig;
use crate::runtime::open_index;

#[derive(Debug, Args)]
pub struct ConsolidateArgs {
    #[arg(long)]
    pub region: Option<String>,
    #[arg(long, default_value_t = false)]
    pub all: bool,
    #[arg(long, default_value = "vault")]
    pub vault: std::path::PathBuf,
}

pub async fn run(args: ConsolidateArgs) -> Result<()> {
    let cfg = AppConfig::load(&args.vault)?;
    let index = open_index(&args.vault)?;
    let claim_store = ClaimStore::new(&index);
    let provider = match cfg.llm.provider.as_str() {
        "anthropic" => LlmProvider::Anthropic,
        "openai" => LlmProvider::OpenAi,
        _ => LlmProvider::Ollama,
    };
    let llm = make_client(provider, cfg.llm.model.clone())?;
    let atlas = AtlasWriter {
        db: &index,
        claim_store: &claim_store,
        llm: llm.as_ref(),
        vault: &args.vault,
    };
    let world = WorldMapWriter {
        db: &index,
        claim_store: &claim_store,
        llm: llm.as_ref(),
        vault: &args.vault,
    };

    if let Some(region) = args.region.as_deref() {
        atlas.rebuild_region(region).await?;
        world.rebuild().await?;
        println!("Consolidated region: {region}");
        return Ok(());
    }

    if args.all {
        let mut regions = BTreeSet::new();
        for note_id in index.all_ids()? {
            if let Some(note) = index.get_note(&note_id)? {
                regions.insert(note.region);
            }
        }
        for region in regions {
            atlas.rebuild_region(&region).await?;
        }
        world.rebuild().await?;
        println!("Consolidated all regions.");
        return Ok(());
    }

    let report = atlas.rebuild_all_changed().await;
    world.rebuild().await?;
    println!(
        "Consolidated changed regions: {} rebuilt, {} failed.",
        report.rebuilt_regions.len(),
        report.failed_regions.len()
    );
    Ok(())
}
