use anyhow::Result;

use crate::index::{Index, RebuildStats};
use crate::note;
use crate::vault::{scan, Vault, VaultEvent};

pub struct Indexer<'a> {
    pub vault: &'a Vault,
    pub index: &'a Index,
}

impl<'a> Indexer<'a> {
    pub fn full_rebuild(&self) -> Result<RebuildStats> {
        let mut stats = RebuildStats::default();
        for path in scan(self.vault.root()) {
            match note::parse(&path) {
                Ok(parsed) => {
                    if let Err(err) = self.index.upsert_note(&parsed, &parsed.body) {
                        stats.errors += 1;
                        tracing::warn!(path = %path.display(), error = %err, "failed to upsert parsed note");
                    } else {
                        stats.inserted += 1;
                    }
                }
                Err(err) => {
                    stats.skipped += 1;
                    stats.errors += 1;
                    tracing::warn!(path = %path.display(), error = %err, "failed to parse note during rebuild");
                }
            }
        }
        Ok(stats)
    }

    pub fn handle_event(&self, ev: VaultEvent) -> Result<()> {
        match ev {
            VaultEvent::Modified(path) | VaultEvent::Created(path) => {
                let parsed = note::parse(&path)?;
                self.index.upsert_note(&parsed, &parsed.body)?;
            }
            VaultEvent::Deleted(path) => {
                if let Some(id) = self.index.id_by_path(&path)? {
                    self.index.delete_note(&id)?;
                }
            }
        }
        Ok(())
    }
}
