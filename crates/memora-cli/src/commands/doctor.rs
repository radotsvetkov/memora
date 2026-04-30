use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use clap::Args;
use rusqlite::Connection;

#[derive(Debug, Args)]
pub struct DoctorArgs {
    #[arg(long, default_value = "vault")]
    pub vault: PathBuf,
}

pub fn run(args: DoctorArgs) -> Result<()> {
    let vault_root = args.vault;
    let memora_dir = vault_root.join(".memora");
    let config_path = memora_dir.join("config.toml");
    let db_path = memora_dir.join("memora.db");
    let vectors_bin = memora_dir.join("vectors.bin");
    let vectors_data = memora_dir.join("vectors.hnsw.data");
    let vectors_graph = memora_dir.join("vectors.hnsw.graph");
    let lock_path = memora_dir.join("watch.lock");

    println!("vault: {}", vault_root.display());
    println!(
        "vault_exists: {}",
        status_bool(vault_root.exists() && vault_root.is_dir())
    );
    println!(
        ".memora_exists: {}",
        status_bool(memora_dir.exists() && memora_dir.is_dir())
    );
    println!(
        "config_exists: {} ({})",
        status_bool(config_path.exists()),
        config_path.display()
    );
    println!(
        "db_exists: {} ({})",
        status_bool(db_path.exists()),
        db_path.display()
    );
    println!(
        "vectors_files_exist: {}",
        status_bool(vectors_bin.exists() && vectors_data.exists() && vectors_graph.exists())
    );
    println!("watch_lock_present: {}", status_bool(lock_path.exists()));

    if db_path.exists() {
        let conn = Connection::open(&db_path)?;
        let notes_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))?;
        let fts_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM notes_fts", [], |row| row.get(0))?;
        let claims_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM claims", [], |row| row.get(0))?;
        println!("notes_count: {notes_count}");
        println!("notes_fts_count: {fts_count}");
        println!("claims_count: {claims_count}");
    }

    println!(
        "vectors_sizes_bytes: bin={} data={} graph={}",
        file_size_or_zero(&vectors_bin),
        file_size_or_zero(&vectors_data),
        file_size_or_zero(&vectors_graph)
    );
    Ok(())
}

fn status_bool(value: bool) -> &'static str {
    if value {
        "ok"
    } else {
        "missing"
    }
}

fn file_size_or_zero(path: &PathBuf) -> u64 {
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}
