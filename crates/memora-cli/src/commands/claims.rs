use anyhow::Result;
use clap::{Args, Subcommand};

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
}

#[derive(Debug, Args)]
pub struct ClaimsShowArgs {
    #[arg(long = "claim-id")]
    pub claim_id: String,
}

pub fn run(cmd: ClaimsCommand) -> Result<()> {
    match cmd {
        ClaimsCommand::Extract(_args) => {}
        ClaimsCommand::Show(_args) => {}
    }
    Ok(())
}
