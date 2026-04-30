use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use memora_core::indexer::FrontmatterFixMode;
use memora_core::vault::watch as watch_vault;
use memora_core::{Scheduler, SchedulerConfig, VaultEvent};
use memora_llm::{make_client, LlmProvider};

use crate::config::AppConfig;
use crate::runtime::{build_embedder, open_index, open_vault, open_vector};

#[derive(Debug, Args)]
pub struct WatchArgs {
    #[arg(long, default_value = "vault")]
    pub vault: std::path::PathBuf,
    #[arg(long, conflicts_with = "no_auto_fix_frontmatter")]
    pub auto_fix_frontmatter: bool,
    #[arg(long, conflicts_with = "auto_fix_frontmatter")]
    pub no_auto_fix_frontmatter: bool,
}

pub async fn run(args: WatchArgs) -> Result<()> {
    let cfg = AppConfig::load(&args.vault)?;
    let _lock = acquire_watch_lock(&args.vault)?;
    let vault = open_vault(&args.vault);
    let index = Arc::new(open_index(&args.vault)?);
    let vector = open_vector(&args.vault, &cfg.embed)?;
    let embedder = build_embedder(&cfg.embed);
    let refs_sync_mode = cfg.frontmatter.refs_sync_mode()?;
    let debounce = Duration::from_millis(cfg.watch.debounce_ms);
    let indexer = memora_core::indexer::Indexer::new(
        &vault,
        index.as_ref(),
        embedder,
        Arc::new(Mutex::new(vector)),
    )
    .with_frontmatter_fix_mode(resolve_watch_fix_mode(&args))
    .with_refs_sync_mode(refs_sync_mode);
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

    while !stop.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => {
                let events = coalesce_events(&rx, event, debounce);
                for event in events {
                    if let Err(err) = indexer.handle_event(event).await {
                        tracing::warn!(error = %err, "watch event processing failed; continuing");
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    scheduler.abort();
    Ok(())
}

fn resolve_watch_fix_mode(args: &WatchArgs) -> FrontmatterFixMode {
    if args.no_auto_fix_frontmatter {
        FrontmatterFixMode::Strict
    } else {
        FrontmatterFixMode::RewriteMissing
    }
}

fn coalesce_events(
    rx: &std::sync::mpsc::Receiver<VaultEvent>,
    first: VaultEvent,
    debounce: Duration,
) -> Vec<VaultEvent> {
    let mut latest_by_path: HashMap<PathBuf, VaultEvent> = HashMap::new();
    latest_by_path.insert(event_path(&first).to_path_buf(), first);

    loop {
        match rx.recv_timeout(debounce) {
            Ok(event) => {
                latest_by_path.insert(event_path(&event).to_path_buf(), event);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    latest_by_path.into_values().collect()
}

fn event_path(event: &VaultEvent) -> &Path {
    match event {
        VaultEvent::Created(path)
        | VaultEvent::Modified(path)
        | VaultEvent::Renamed(path)
        | VaultEvent::Deleted(path) => path.as_path(),
    }
}

struct WatchLock {
    path: PathBuf,
}

impl Drop for WatchLock {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_file(&self.path) {
            tracing::warn!(path = %self.path.display(), error = %err, "failed to remove watch lock");
        }
    }
}

fn acquire_watch_lock(vault_root: &Path) -> Result<WatchLock> {
    let lock_path = vault_root.join(".memora").join("watch.lock");
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "create lock parent directory for watcher at {}",
                parent.display()
            )
        })?;
    }

    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(mut file) => {
            let pid = std::process::id();
            writeln!(file, "{pid}")?;
            file.flush()?;
            Ok(WatchLock { path: lock_path })
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Err(anyhow!(
            "watcher lock already exists at {}. Another watcher may be running for this vault. \
             If no watcher is active, remove this file and retry.",
            lock_path.display()
        )),
        Err(err) => Err(err.into()),
    }
}
