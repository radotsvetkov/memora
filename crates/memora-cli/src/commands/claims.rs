use anyhow::Result;
use clap::{Args, Subcommand};
use memora_core::claims::{ClaimExtractor, ClaimStore};
use memora_core::note;
use memora_llm::{make_client, LlmProvider};

use crate::config::AppConfig;
use crate::runtime::open_index;

#[derive(Debug, Subcommand)]
pub enum ClaimsCommand {
    /// Extract claims for one note id.
    Extract(ClaimsExtractArgs),
    /// Show one claim id and its source quote.
    Show(ClaimsShowArgs),
}

#[derive(Debug, Args)]
pub struct ClaimsExtractArgs {
    #[arg(long)]
    pub note: String,
    #[arg(long, default_value = "vault")]
    pub vault: std::path::PathBuf,
}

#[derive(Debug, Args)]
pub struct ClaimsShowArgs {
    #[arg(value_name = "ID")]
    pub claim_id: String,
    #[arg(long, default_value = "vault")]
    pub vault: std::path::PathBuf,
}

pub async fn run(cmd: ClaimsCommand) -> Result<()> {
    match cmd {
        ClaimsCommand::Extract(args) => run_extract(args).await?,
        ClaimsCommand::Show(args) => run_show(args)?,
    }
    Ok(())
}

async fn run_extract(args: ClaimsExtractArgs) -> Result<()> {
    let cfg = AppConfig::load(&args.vault)?;
    let provider = match cfg.llm.provider.as_str() {
        "anthropic" => LlmProvider::Anthropic,
        "openai" => LlmProvider::OpenAi,
        _ => LlmProvider::Ollama,
    };
    let llm = make_client(provider, cfg.llm.model.clone())?;
    let index = open_index(&args.vault)?;
    let store = ClaimStore::new(&index);
    let row = index
        .get_note(&args.note)?
        .ok_or_else(|| anyhow::anyhow!("note not found: {}", args.note))?;
    let path = if std::path::PathBuf::from(&row.path).is_absolute() {
        std::path::PathBuf::from(&row.path)
    } else {
        args.vault.join(&row.path)
    };
    let parsed = note::parse(&path)?;
    let extractor = ClaimExtractor {
        llm: llm.as_ref(),
        model_label: llm.model_name().to_string(),
    };
    let claims = extractor.extract(&parsed, &parsed.body).await?;
    for claim in &claims {
        store.upsert(claim)?;
    }
    println!("{}", serde_json::to_string_pretty(&claims)?);
    Ok(())
}

fn run_show(args: ClaimsShowArgs) -> Result<()> {
    let index = open_index(&args.vault)?;
    let store = ClaimStore::new(&index);
    let claim = store
        .get(&args.claim_id)?
        .ok_or_else(|| anyhow::anyhow!("claim not found: {}", args.claim_id))?;
    let row = index
        .get_note(&claim.note_id)?
        .ok_or_else(|| anyhow::anyhow!("note missing for claim: {}", claim.note_id))?;
    let note_path = if std::path::PathBuf::from(&row.path).is_absolute() {
        std::path::PathBuf::from(&row.path)
    } else {
        args.vault.join(&row.path)
    };
    let body = note::parse(&note_path)?.body;
    let quote = body
        .get(claim.span_start..claim.span_end)
        .unwrap_or("")
        .to_string();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "id": claim.id,
            "subject": claim.subject,
            "predicate": claim.predicate,
            "object": claim.object,
            "note_id": claim.note_id,
            "span_start": claim.span_start,
            "span_end": claim.span_end,
            "quote": quote
        }))?
    );
    Ok(())
}
