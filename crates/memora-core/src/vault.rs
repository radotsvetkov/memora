use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use notify::event::ModifyKind;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use walkdir::{DirEntry, WalkDir};

#[derive(Debug, Clone)]
pub struct Vault {
    root: PathBuf,
}

impl Vault {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

pub fn scan(root: &Path) -> impl Iterator<Item = PathBuf> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !should_prune_entry(entry))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(DirEntry::into_path)
        .filter(|path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("md")
                && !is_hidden_file(path)
                && !is_generated_atlas_or_index(path)
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Renamed(PathBuf),
    Deleted(PathBuf),
}

#[derive(Debug, Error)]
pub enum VaultError {
    #[error(transparent)]
    Notify(#[from] notify::Error),
}

pub fn watch(root: &Path) -> Result<(RecommendedWatcher, Receiver<VaultEvent>), VaultError> {
    let (tx, rx) = mpsc::channel();
    let mut watcher =
        notify::recommended_watcher(move |event_result: Result<Event, notify::Error>| {
            let event = match event_result {
                Ok(event) => event,
                Err(err) => {
                    tracing::warn!(error = %err, "watcher event error");
                    return;
                }
            };

            if let EventKind::Modify(ModifyKind::Name(_)) = event.kind {
                for path in event.paths {
                    if is_watchable_markdown(&path) {
                        let _ = tx.send(VaultEvent::Renamed(path));
                    }
                }
                return;
            }

            let event_factory = classify_event_kind(&event.kind);
            if let Some(factory) = event_factory {
                for path in event.paths {
                    if is_watchable_markdown(&path) {
                        let _ = tx.send(factory(path));
                    }
                }
            }
        })?;

    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok((watcher, rx))
}

fn classify_event_kind(kind: &EventKind) -> Option<fn(PathBuf) -> VaultEvent> {
    match kind {
        EventKind::Create(_) => Some(VaultEvent::Created),
        EventKind::Modify(modify_kind) => match modify_kind {
            ModifyKind::Data(_) | ModifyKind::Metadata(_) | ModifyKind::Any | ModifyKind::Other => {
                Some(VaultEvent::Modified)
            }
            ModifyKind::Name(_) => None,
        },
        EventKind::Remove(_) => Some(VaultEvent::Deleted),
        _ => None,
    }
}

fn should_prune_entry(entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
        return false;
    }

    let name = entry.file_name().to_string_lossy();
    if entry.file_type().is_dir() {
        return matches!(name.as_ref(), ".memora" | ".git" | "target") || name.starts_with('.');
    }

    entry.file_type().is_file() && name.starts_with('.')
}

fn is_hidden_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn is_under_hidden_dir(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|segment| segment.starts_with('.'))
    })
}

fn is_watchable_markdown(path: &Path) -> bool {
    let is_markdown = path.extension().and_then(|ext| ext.to_str()) == Some("md");
    let in_memora = path
        .components()
        .any(|component| component.as_os_str() == ".memora");
    is_markdown && !in_memora && !is_under_hidden_dir(path) && !is_generated_atlas_or_index(path)
}

fn is_generated_atlas_or_index(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "_atlas.md" | "_index.md"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn fixture_vault_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/sample-vault")
            .canonicalize()
            .expect("fixture vault should exist")
    }

    #[test]
    fn scan_returns_only_expected_markdown_files() {
        let root = fixture_vault_root();
        let paths: BTreeSet<_> = scan(&root)
            .map(|path| {
                path.strip_prefix(&root)
                    .expect("path under root")
                    .to_path_buf()
            })
            .collect();

        let expected = BTreeSet::from([
            PathBuf::from("world_map.md"),
            PathBuf::from("work/team-sync.md"),
            PathBuf::from("personal/example.md"),
        ]);
        assert_eq!(paths, expected);
    }

    #[test]
    fn scan_skips_dotfiles() {
        let root = tempdir().expect("create temp dir");
        let visible = root.path().join("visible.md");
        let hidden = root.path().join(".hidden.md");

        fs::write(&visible, "# visible").expect("write visible file");
        fs::write(&hidden, "# hidden").expect("write hidden file");

        let scanned: Vec<_> = scan(root.path()).collect();
        assert_eq!(scanned, vec![visible]);
    }

    #[test]
    fn excludes_generated_atlas_and_index_from_scan_and_watch() {
        let root = tempdir().expect("create temp dir");
        let atlas = root.path().join("_atlas.md");
        let index = root.path().join("_index.md");
        let normal = root.path().join("normal.md");

        fs::write(&atlas, "# atlas").expect("write atlas");
        fs::write(&index, "# index").expect("write index");
        fs::write(&normal, "# normal").expect("write normal");

        let scanned: BTreeSet<_> = scan(root.path()).collect();
        assert!(scanned.contains(&normal));
        assert!(!scanned.contains(&atlas));
        assert!(!scanned.contains(&index));

        let watchable_normal = Path::new("/vault/normal.md");
        let watchable_atlas = Path::new("/vault/_atlas.md");
        let watchable_index = Path::new("/vault/_index.md");
        assert!(is_watchable_markdown(watchable_normal));
        assert!(!is_watchable_markdown(watchable_atlas));
        assert!(!is_watchable_markdown(watchable_index));
    }
}
