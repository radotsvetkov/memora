use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Args, Subcommand};
use regex::Regex;

use memora_core::scan;

#[derive(Debug, Subcommand)]
pub enum PrivacyCommand {
    /// Audit notes for potentially sensitive content missing explicit privacy.
    Audit(PrivacyAuditArgs),
}

#[derive(Debug, Args)]
pub struct PrivacyAuditArgs {
    #[arg(long, default_value = "vault")]
    pub vault_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SuspectNote {
    path: PathBuf,
    matched_keyword: String,
}

pub fn run(cmd: PrivacyCommand) -> Result<()> {
    match cmd {
        PrivacyCommand::Audit(args) => run_audit(args),
    }
}

fn run_audit(args: PrivacyAuditArgs) -> Result<()> {
    let suspects = audit_notes(&args.vault_root)?;
    if suspects.is_empty() {
        println!("No suspect notes found.");
        return Ok(());
    }

    println!("Potentially sensitive notes without explicit privacy frontmatter:");
    for suspect in suspects {
        println!(
            "- {} (matched: {})",
            suspect.path.display(),
            suspect.matched_keyword
        );
    }
    println!("Review these notes and add explicit `privacy:` frontmatter where needed.");
    Ok(())
}

fn audit_notes(vault_root: &Path) -> Result<Vec<SuspectNote>> {
    let keyword_re = sensitive_keywords_regex()?;
    let mut suspects = Vec::new();

    for path in scan(vault_root) {
        let source = fs::read_to_string(&path)?;
        if has_explicit_privacy_frontmatter(&source) {
            continue;
        }
        if let Some(matched) = keyword_re.find(&source) {
            let rel_path = path
                .strip_prefix(vault_root)
                .map_or_else(|_| path.clone(), Path::to_path_buf);
            suspects.push(SuspectNote {
                path: rel_path,
                matched_keyword: matched.as_str().to_string(),
            });
        }
    }

    Ok(suspects)
}

fn has_explicit_privacy_frontmatter(source: &str) -> bool {
    let mut lines = source.lines();
    if !matches!(lines.next(), Some(line) if line.trim() == "---") {
        return false;
    }

    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if trimmed.starts_with("privacy:") {
            return true;
        }
    }
    false
}

fn sensitive_keywords_regex() -> Result<Regex> {
    Regex::new(
        r"(?i)\b(salary|bank|account|password|ssn|social security|medical|diagnosis|therapy|iban|routing number|credit card)\b",
    )
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn audit_detects_salary_without_privacy_frontmatter() -> Result<()> {
        let temp = tempdir()?;
        let vault_root = temp.path().join("vault");
        fs::create_dir_all(&vault_root)?;
        let note_path = vault_root.join("finance.md");
        fs::write(
            &note_path,
            r#"---
id: finance-1
region: personal/finance
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Comp details"
---
salary 95000
"#,
        )?;

        let suspects = audit_notes(&vault_root)?;
        assert_eq!(suspects.len(), 1);
        assert_eq!(suspects[0].path, PathBuf::from("finance.md"));
        assert_eq!(suspects[0].matched_keyword.to_lowercase(), "salary");
        Ok(())
    }
}
