use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;

#[derive(Debug, Parser)]
#[command(name = "memora", about = "Memora CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Claims {
        #[command(subcommand)]
        command: commands::claims::ClaimsCommand,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Claims { command } => commands::claims::run(command)?,
    }
    Ok(())
}
