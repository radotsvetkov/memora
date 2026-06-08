use anyhow::Result;
use clap::Args;
use memora_core::answer::AnsweringPipeline;
use memora_core::cite::CitationValidator;
use memora_core::claims::ClaimStore;
use memora_core::{HybridRetriever, PrivacyFilter};

use crate::config::AppConfig;
use crate::runtime::{
    build_embedder_from_app, make_gated_client, open_index, open_vector, privacy_config_from_app,
    provider_from_config,
};

#[derive(Debug, Args)]
pub struct QueryArgs {
    #[arg(value_name = "text")]
    pub text: String,
    #[arg(long, default_value_t = false)]
    pub raw: bool,
    #[arg(long, default_value = "vault")]
    pub vault: std::path::PathBuf,
}

pub async fn run(args: QueryArgs) -> Result<()> {
    let cfg = AppConfig::load(&args.vault)?;
    let index = open_index(&args.vault)?;
    let vector_index = open_vector(&args.vault, &cfg.embed)?;
    let embedder = build_embedder_from_app(&cfg.embed, &cfg.llm)?;
    let store = ClaimStore::new(&index);
    let validator = CitationValidator {
        store: &store,
        index: &index,
        vault_root: &args.vault,
    };
    let retriever = HybridRetriever {
        index: &index,
        vec: &vector_index,
        embedder: embedder.as_ref(),
    };

    let k = cfg.retrieval.top_k;
    if args.raw {
        let hits = retriever.search(&args.text, k).await?;
        for hit in hits {
            if let Some(note) = index.get_note(&hit.id)? {
                println!(
                    "{} | {:.4} | {} | {}",
                    note.id, hit.score, note.region, note.summary
                );
            }
        }
        return Ok(());
    }

    let llm = make_gated_client(&cfg.llm)?;
    let privacy_config = privacy_config_from_app(&cfg);
    let provider = provider_from_config(&cfg.llm);
    let pipeline = AnsweringPipeline {
        retriever: &retriever,
        claim_store: &store,
        validator: &validator,
        llm: Some(llm.as_ref()),
        privacy_filter: PrivacyFilter::new_for_provider(provider, &privacy_config),
        privacy_config,
    };
    let answer = pipeline.answer(&args.text, k).await?;
    println!("{}", answer.clean_text);
    println!(
        "\nVerified: {} · Unverified: {} · Mismatches: {}",
        answer.verified_count, answer.unverified_count, answer.mismatch_count
    );
    Ok(())
}
