use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::{fmt, EnvFilter};

mod commands;
mod config;
mod runtime;

#[derive(Debug, Parser)]
#[command(
    name = "memora",
    about = "Verify AI citations against your sources, structurally.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Zero-config offline demo: watch memora catch a fabricated citation.
    Demo(commands::demo::DemoArgs),
    /// Verify an AI answer's citations against a vault; exit non-zero on failure.
    Verify(commands::verify::VerifyArgs),
    /// Ingest a document (PDF/text/markdown/transcript) into the vault as a note.
    Ingest(commands::ingest::IngestArgs),
    /// Generate a self-contained HTML report of the vault (graph, contradictions, world map).
    Report(commands::report::ReportArgs),
    /// Create a new vault (frontmatter config, sample note, empty world map).
    Init(commands::init::InitArgs),
    /// Parse the vault, extract claims, and build the SQLite + vector index.
    Index(commands::index::IndexArgs),
    /// Watch the vault for changes and keep the index current incrementally.
    Watch(commands::watch::WatchArgs),
    /// Run the MCP server over stdio (same as the `memora-mcp` binary).
    Serve(commands::serve::ServeArgs),
    /// Inspect and manage the claim graph directly.
    Claims {
        #[command(subcommand)]
        command: commands::claims::ClaimsCommand,
    },
    /// Run one challenger pass: surface contradictions, stale claims, and open questions.
    Challenge(commands::challenge::ChallengeArgs),
    /// Rebuild region atlases and the world map from the current claim graph.
    Consolidate(commands::consolidate::ConsolidateArgs),
    /// Diagnose a vault: config, index health, embedder, and LLM connectivity.
    Doctor(commands::doctor::DoctorArgs),
    /// Privacy-related utilities.
    Privacy {
        #[command(subcommand)]
        command: commands::privacy::PrivacyCommand,
    },
    /// Ask a question; get a verified, cited answer from the configured LLM.
    Query(commands::query::QueryArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("warn,memora_core=info,memora_cli=info,memora_llm=info")
    });
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Demo(args) => commands::demo::run(args).await?,
        Commands::Verify(args) => commands::verify::run(args).await?,
        Commands::Ingest(args) => commands::ingest::run(args).await?,
        Commands::Report(args) => commands::report::run(args).await?,
        Commands::Init(args) => commands::init::run(args)?,
        Commands::Index(args) => commands::index::run(args).await?,
        Commands::Watch(args) => commands::watch::run(args).await?,
        Commands::Serve(args) => commands::serve::run(args).await?,
        Commands::Claims { command } => commands::claims::run(command).await?,
        Commands::Challenge(args) => commands::challenge::run(args).await?,
        Commands::Consolidate(args) => commands::consolidate::run(args).await?,
        Commands::Doctor(args) => commands::doctor::run(args)?,
        Commands::Privacy { command } => commands::privacy::run(command)?,
        Commands::Query(args) => commands::query::run(args).await?,
    }
    Ok(())
}
