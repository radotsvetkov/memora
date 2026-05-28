use std::path::{Component, Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VaultPathError {
    #[error("path escapes vault root: {0}")]
    OutsideVault(String),
    #[error("invalid path component in {0}")]
    InvalidComponent(String),
}

/// Reject `..`, absolute roots, and prefix components that escape the vault.
pub fn validate_relative_path(rel: &Path) -> Result<(), VaultPathError> {
    for component in rel.components() {
        match component {
            Component::ParentDir => {
                return Err(VaultPathError::InvalidComponent(rel.display().to_string()));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(VaultPathError::InvalidComponent(rel.display().to_string()));
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(())
}

/// Join a vault-relative path after validating components.
pub fn join_vault_relative(vault_root: &Path, rel: &Path) -> Result<PathBuf, VaultPathError> {
    validate_relative_path(rel)?;
    Ok(vault_root.join(rel))
}

/// Resolve an indexed note path for read access, always constraining reads to the vault.
pub fn resolve_note_path(vault_root: &Path, indexed_path: &str) -> Result<PathBuf, VaultPathError> {
    let raw = Path::new(indexed_path);
    if raw.is_absolute() {
        let vault_canon = canonicalize_or_identity(vault_root);
        let raw_canon = canonicalize_or_identity(raw);
        if let Ok(rel) = raw_canon.strip_prefix(&vault_canon) {
            return join_vault_relative(vault_root, rel);
        }
        return Err(VaultPathError::OutsideVault(indexed_path.to_string()));
    }
    join_vault_relative(vault_root, raw)
}

fn canonicalize_or_identity(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Validate a capture/consolidation region string before writing under the vault.
pub fn validate_region(vault_root: &Path, region: &str) -> Result<PathBuf, VaultPathError> {
    if region.is_empty() {
        return Err(VaultPathError::InvalidComponent("empty region".to_string()));
    }
    join_vault_relative(vault_root, Path::new(region))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn rejects_parent_dir_in_region() {
        let temp = tempdir().expect("tempdir");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("mkdir");
        let err = validate_region(&vault, "../escape").expect_err("should reject");
        assert!(matches!(err, VaultPathError::InvalidComponent(_)));
    }

    #[test]
    fn resolve_stays_inside_vault() {
        let temp = tempdir().expect("tempdir");
        let vault = temp.path().join("vault");
        fs::create_dir_all(vault.join("notes")).expect("mkdir");
        let note = vault.join("notes/a.md");
        fs::write(&note, "hello").expect("write");

        let resolved = resolve_note_path(&vault, "notes/a.md").expect("resolve");
        assert_eq!(resolved, vault.join("notes/a.md"));
    }

    #[test]
    fn resolve_absolute_under_vault() {
        let temp = tempdir().expect("tempdir");
        let vault = temp.path().join("vault");
        fs::create_dir_all(vault.join("notes")).expect("mkdir");
        let note = vault.join("notes/a.md");
        fs::write(&note, "hello").expect("write");
        let resolved = resolve_note_path(&vault, note.to_string_lossy().as_ref()).expect("resolve");
        assert_eq!(resolved, note);
    }
}
