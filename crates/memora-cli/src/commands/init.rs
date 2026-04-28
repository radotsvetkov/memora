use std::fs;

use anyhow::Result;
use chrono::Utc;
use clap::Args;
use memora_core::note::{self, Frontmatter, Note, NoteSource, Privacy};

use crate::config::AppConfig;

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(long, default_value = "vault")]
    pub vault: std::path::PathBuf,
}

pub fn run(args: InitArgs) -> Result<()> {
    fs::create_dir_all(&args.vault)?;
    fs::create_dir_all(args.vault.join(".memora"))?;
    let _ = AppConfig::write_default(&args.vault)?;

    let world_map = args.vault.join("world_map.md");
    if !world_map.exists() {
        fs::write(
            &world_map,
            "# World Map\n\nYour regions appear here after indexing and consolidation.\n",
        )?;
    }

    let sample_dir = args.vault.join("sample");
    fs::create_dir_all(&sample_dir)?;
    let sample_note_path = sample_dir.join("hello-memora.md");
    if !sample_note_path.exists() {
        let now = Utc::now();
        let note = Note {
            path: sample_note_path.clone(),
            fm: Frontmatter {
                id: "hello-memora".to_string(),
                region: "sample".to_string(),
                source: NoteSource::Personal,
                privacy: Privacy::Private,
                created: now,
                updated: now,
                summary: "Sample note for first index.".to_string(),
                tags: vec!["sample".to_string()],
                refs: vec![],
            },
            body: "Memora is initialized.\n".to_string(),
            wikilinks: vec![],
        };
        fs::write(&sample_note_path, note::render(&note))?;
    }

    println!("Initialized vault at {}", args.vault.display());
    Ok(())
}
