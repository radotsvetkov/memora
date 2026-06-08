use anyhow::Result;
use clap::Args;
use memora_core::Memora;

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
    let memora = Memora::open(&args.vault)?;
    let k = memora.config().retrieval.top_k;

    if args.raw {
        for hit in memora.search(&args.text, k).await? {
            if let Some(note) = memora.index().get_note(&hit.id)? {
                println!(
                    "{} | {:.4} | {} | {}",
                    note.id, hit.score, note.region, note.summary
                );
            }
        }
        return Ok(());
    }

    let answer = memora.query_verified(&args.text, k).await?;
    println!("{}", answer.clean_text);
    println!(
        "\nVerified: {} · Unverified: {} · Mismatches: {} · Superseded: {}",
        answer.verified_count,
        answer.unverified_count,
        answer.mismatch_count,
        answer.superseded_count
    );
    Ok(())
}
