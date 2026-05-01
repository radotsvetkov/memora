use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::{fmt, EnvFilter};

mod commands;
mod config;
mod runtime;

#[derive(Debug, Parser)]
#[command(name = "memora", about = "Memora CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Init(commands::init::InitArgs),
    Index(commands::index::IndexArgs),
    Watch(commands::watch::WatchArgs),
    Serve(commands::serve::ServeArgs),
    Claims {
        #[command(subcommand)]
        command: commands::claims::ClaimsCommand,
    },
    Challenge(commands::challenge::ChallengeArgs),
    Consolidate(commands::consolidate::ConsolidateArgs),
    Doctor(commands::doctor::DoctorArgs),
    Privacy {
        #[command(subcommand)]
        command: commands::privacy::PrivacyCommand,
    },
    Query(commands::query::QueryArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
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
