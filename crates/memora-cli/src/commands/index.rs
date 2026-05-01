use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use clap::Args;
use memora_core::claims::ClaimExtractor;
use memora_core::indexer::{FrontmatterFixMode, Indexer};
use memora_core::note::ParseError;
use memora_llm::{make_client, LlmProvider};

use crate::config::AppConfig;
use crate::runtime::{build_embedder, open_index, open_vault, open_vector};

#[derive(Debug, Args)]
pub struct IndexArgs {
    #[arg(long, default_value = "vault")]
    pub vault: std::path::PathBuf,
    #[arg(long, conflicts_with = "no_auto_fix_frontmatter")]
    pub auto_fix_frontmatter: bool,
    #[arg(long, conflicts_with = "auto_fix_frontmatter")]
    pub no_auto_fix_frontmatter: bool,
    /// Skip contradiction detection during indexing. Recommended for first-time bulk import —
    /// contradictions among newly-imported notes don't have meaningful temporal order. Run
    /// dedicated contradiction workflows separately after import if needed.
    #[arg(long)]
    pub no_contradict: bool,
}

pub async fn run(args: IndexArgs) -> Result<()> {
    let (total_notes, needs_fixing) = prescan_frontmatter(&args.vault)?;
    let fix_mode = choose_fix_mode(&args, total_notes, needs_fixing)?;

    let cfg = AppConfig::load(&args.vault)?;
    let vault = open_vault(&args.vault);
    let index = open_index(&args.vault)?;
    let vector = open_vector(&args.vault, &cfg.embed)?;
    let embedder = build_embedder(&cfg.embed, &cfg.llm)?;
    let provider = match cfg.llm.provider.as_str() {
        "anthropic" => LlmProvider::Anthropic,
        "openai" => LlmProvider::OpenAi,
        _ => LlmProvider::Ollama,
    };
    let llm = make_client(
        provider,
        cfg.llm.model.clone(),
        cfg.llm.endpoint.clone(),
        cfg.llm.embedding_model.clone(),
    )?;
    let claim_extractor = ClaimExtractor {
        llm: Arc::clone(&llm),
        model_label: llm.model_name().to_string(),
    };
    let refs_sync_mode = cfg.frontmatter.refs_sync_mode()?;
    let indexer = Indexer::new(&vault, &index, embedder, Arc::new(Mutex::new(vector)))
        .with_frontmatter_fix_mode(fix_mode)
        .with_refs_sync_mode(refs_sync_mode)
        .with_claims(claim_extractor)
        .with_parallel_notes(cfg.indexing.parallelism)
        .with_skip_contradiction_detection(args.no_contradict);
    let stats = indexer.full_rebuild().await?;
    println!(
        "Indexed: inserted={}, skipped={}, errors={}",
        stats.inserted, stats.skipped, stats.errors
    );
    if stats.errors > 0 {
        eprintln!(
            "\n{} notes failed. Re-run with RUST_LOG=warn for details.",
            stats.errors
        );
    }
    if stats.inserted > 50 && !args.no_contradict {
        eprintln!(
            "\nTip: For large vaults, --no-contradict speeds up first-time indexing significantly. \
             Contradiction detection runs continuously in `memora watch`."
        );
    }
    Ok(())
}

fn prescan_frontmatter(vault_root: &std::path::Path) -> Result<(usize, usize)> {
    let mut total = 0usize;
    let mut needs_fix = 0usize;
    for path in memora_core::scan(vault_root) {
        total += 1;
        match memora_core::note::parse(&path) {
            Ok(_) => {}
            Err(ParseError::MissingFrontmatter) | Err(ParseError::MissingField(_)) => {
                needs_fix += 1;
            }
            Err(_) => {}
        }
    }
    Ok((total, needs_fix))
}

fn choose_fix_mode(
    args: &IndexArgs,
    total_notes: usize,
    needs_fixing: usize,
) -> Result<FrontmatterFixMode> {
    if args.no_auto_fix_frontmatter {
        return Ok(FrontmatterFixMode::Strict);
    }
    if args.auto_fix_frontmatter || needs_fixing == 0 {
        return Ok(FrontmatterFixMode::RewriteMissing);
    }

    println!(
        "Found {total_notes} notes. {needs_fixing} need frontmatter added.\n\
         Memora will prepend YAML frontmatter to these files (id, region,\n\
         created, updated, summary inferred from filename and content).\n\
         Your existing content is preserved.\n\n\
         Proceed? [y/N]"
    );

    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let proceed = matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes");
    if !proceed {
        return Err(anyhow!(
            "Re-run with --auto-fix-frontmatter or add frontmatter manually."
        ));
    }
    Ok(FrontmatterFixMode::RewriteMissing)
}
