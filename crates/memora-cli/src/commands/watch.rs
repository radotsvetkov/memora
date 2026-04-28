use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use clap::Args;
use memora_core::vault::watch as watch_vault;
use memora_core::{Scheduler, SchedulerConfig};
use memora_llm::{make_client, LlmProvider};

use crate::config::AppConfig;
use crate::runtime::{build_embedder, open_index, open_vault, open_vector};

#[derive(Debug, Args)]
pub struct WatchArgs {
    #[arg(long, default_value = "vault")]
    pub vault: std::path::PathBuf,
}

pub async fn run(args: WatchArgs) -> Result<()> {
    let cfg = AppConfig::load(&args.vault)?;
    let vault = open_vault(&args.vault);
    let index = Arc::new(open_index(&args.vault)?);
    let vector = open_vector(&args.vault, &cfg.embed)?;
    let embedder = build_embedder(&cfg.embed);
    let indexer = memora_core::indexer::Indexer::new(
        &vault,
        index.as_ref(),
        embedder,
        Arc::new(Mutex::new(vector)),
    );
    indexer.full_rebuild().await?;

    let provider = match cfg.llm.provider.as_str() {
        "anthropic" => LlmProvider::Anthropic,
        "openai" => LlmProvider::OpenAi,
        _ => LlmProvider::Ollama,
    };
    let llm = make_client(provider, cfg.llm.model.clone())?;
    let scheduler = Scheduler::spawn(
        SchedulerConfig::default(),
        index.clone(),
        Arc::from(llm),
        args.vault.clone(),
    );

    let (_watcher, rx) = watch_vault(&args.vault)?;
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_signal = stop.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        stop_for_signal.store(true, Ordering::SeqCst);
    });

    let handle = tokio::runtime::Handle::current();
    while !stop.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => {
                handle.block_on(indexer.handle_event(event))?;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    scheduler.abort();
    Ok(())
}
