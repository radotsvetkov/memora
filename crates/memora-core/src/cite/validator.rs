use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;

use crate::cite::answer::{rewrite_with_only_verified, CitationStatus, CitedAnswer};
use crate::cite::parser::{extract_quote_before, parse_claim_markers};
use crate::claims::{Claim, ClaimStore};
use crate::index::Index;

#[derive(Debug, Clone)]
pub struct CitationCheck {
    pub claim_id: String,
    pub status: CitationStatus,
    pub source_text: Option<String>,
    pub quote: Option<String>,
}

pub struct CitationValidator<'a> {
    pub store: &'a ClaimStore<'a>,
    pub index: &'a Index,
    pub vault_root: &'a Path,
}

impl<'a> CitationValidator<'a> {
    pub async fn validate(&self, llm_text: &str) -> Result<CitedAnswer> {
        let markers = parse_claim_markers(llm_text);
        let mut checks = Vec::with_capacity(markers.len());
        let mut verified_ids = HashSet::new();

        for (start, _end, claim_id) in markers {
            let quote = extract_quote_before(llm_text, start);
            let Some(claim) = self.store.get(&claim_id)? else {
                checks.push(CitationCheck {
                    claim_id,
                    status: CitationStatus::Unverified,
                    source_text: None,
                    quote,
                });
                continue;
            };

            match self.verify_claim(llm_text, &claim, quote.clone()).await? {
                CitationStatus::Verified => {
                    verified_ids.insert(claim_id.clone());
                    let source_text = self.read_span_text(&claim).await?;
                    checks.push(CitationCheck {
                        claim_id,
                        status: CitationStatus::Verified,
                        source_text,
                        quote,
                    });
                }
                status => {
                    let source_text = self.read_span_text(&claim).await?;
                    checks.push(CitationCheck {
                        claim_id,
                        status,
                        source_text,
                        quote,
                    });
                }
            }
        }

        let verified_count = checks
            .iter()
            .filter(|check| check.status == CitationStatus::Verified)
            .count();
        let unverified_count = checks
            .iter()
            .filter(|check| check.status == CitationStatus::Unverified)
            .count();
        let mismatch_count = checks
            .iter()
            .filter(|check| {
                check.status == CitationStatus::FingerprintMismatch
                    || check.status == CitationStatus::QuoteMismatch
            })
            .count();
        let clean_text = rewrite_with_only_verified(llm_text, &verified_ids);

        Ok(CitedAnswer {
            raw_text: llm_text.to_string(),
            clean_text,
            checks,
            verified_count,
            unverified_count,
            mismatch_count,
            redacted_count: 0,
            degraded: false,
        })
    }

    async fn verify_claim(
        &self,
        _llm_text: &str,
        claim: &Claim,
        quote: Option<String>,
    ) -> Result<CitationStatus> {
        let Some(span_text) = self.read_span_text(claim).await? else {
            return Ok(CitationStatus::Unverified);
        };

        let fingerprint = Claim::compute_fingerprint(&span_text);
        if fingerprint != claim.span_fingerprint {
            return Ok(CitationStatus::FingerprintMismatch);
        }

        if let Some(quote_text) = quote {
            let normalized_quote = collapse_ws_lower(&quote_text);
            let normalized_span = collapse_ws_lower(&span_text);
            if !normalized_quote.is_empty() && !normalized_span.contains(&normalized_quote) {
                return Ok(CitationStatus::QuoteMismatch);
            }
        }

        Ok(CitationStatus::Verified)
    }

    async fn read_span_text(&self, claim: &Claim) -> Result<Option<String>> {
        let Some(note) = self.index.get_note(&claim.note_id)? else {
            return Ok(None);
        };
        let path = self.vault_root.join(&note.path);
        let body = tokio::task::spawn_blocking(move || crate::note::parse(&path))
            .await??
            .body;
        let Some(span_text) = body.get(claim.span_start..claim.span_end) else {
            return Ok(None);
        };
        Ok(Some(span_text.to_string()))
    }
}

fn collapse_ws_lower(input: &str) -> String {
    input
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use anyhow::Result;
    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    use super::CitationValidator;
    use crate::cite::CitationStatus;
    use crate::claims::{Claim, ClaimStore};
    use crate::index::Index;
    use crate::note::{Frontmatter, Note, NoteSource, Privacy};

    fn make_note(id: &str, path: PathBuf, body: &str) -> Note {
        Note {
            path,
            fm: Frontmatter {
                id: id.to_string(),
                region: "test/unit".to_string(),
                source: NoteSource::Personal,
                privacy: Privacy::Private,
                created: Utc
                    .with_ymd_and_hms(2026, 4, 1, 0, 0, 0)
                    .single()
                    .expect("valid created date"),
                updated: Utc
                    .with_ymd_and_hms(2026, 4, 2, 0, 0, 0)
                    .single()
                    .expect("valid updated date"),
                summary: "validator test".to_string(),
                tags: Vec::new(),
                refs: Vec::new(),
            },
            body: body.to_string(),
            wikilinks: Vec::new(),
        }
    }

    fn write_note_file(path: &Path, id: &str, body: &str) -> Result<()> {
        let content = format!(
            r#"---
id: {id}
region: test/unit
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-02T00:00:00Z
summary: "validator test"
tags: []
refs: []
---
{body}
"#
        );
        fs::write(path, content)?;
        Ok(())
    }

    #[tokio::test]
    async fn validator_marks_known_claim_as_verified() -> Result<()> {
        let temp = tempdir()?;
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault)?;
        let note_rel_path = PathBuf::from("note.md");
        let body = "Rado works at HMC and leads Memora.";
        let note = make_note("note-1", note_rel_path.clone(), body);
        let full_note_path = vault.join(&note_rel_path);
        write_note_file(&full_note_path, &note.fm.id, body)?;

        let index = Index::open(&temp.path().join("index.db"))?;
        index.upsert_note(&note, body)?;
        let store = ClaimStore::new(&index);
        let span_start = body.find("Rado works at HMC").expect("span start");
        let span_end = span_start + "Rado works at HMC".len();
        let span = body
            .get(span_start..span_end)
            .expect("span should be valid for test body");
        let claim = Claim {
            id: "aaaaaaaaaaaaaaaa".to_string(),
            subject: "Rado".to_string(),
            predicate: "works_at".to_string(),
            object: "HMC".to_string(),
            note_id: note.fm.id.clone(),
            span_start,
            span_end,
            span_fingerprint: Claim::compute_fingerprint(span),
            valid_from: note.fm.created,
            valid_until: None,
            confidence: 1.0,
            privacy: Privacy::Private,
            extracted_by: "test".to_string(),
            extracted_at: Utc::now(),
        };
        store.upsert(&claim)?;

        let validator = CitationValidator {
            store: &store,
            index: &index,
            vault_root: &vault,
        };

        let answer = validator
            .validate("Rado works at HMC [claim:aaaaaaaaaaaaaaaa].")
            .await?;
        assert_eq!(answer.verified_count, 1);
        assert_eq!(answer.checks[0].status, CitationStatus::Verified);
        Ok(())
    }

    #[tokio::test]
    async fn validator_marks_quote_mismatch() -> Result<()> {
        let temp = tempdir()?;
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault)?;
        let note_rel_path = PathBuf::from("note.md");
        let body = "Rado works at HMC and leads Memora.";
        let note = make_note("note-1", note_rel_path.clone(), body);
        let full_note_path = vault.join(&note_rel_path);
        write_note_file(&full_note_path, &note.fm.id, body)?;

        let index = Index::open(&temp.path().join("index.db"))?;
        index.upsert_note(&note, body)?;
        let store = ClaimStore::new(&index);
        let span_start = body.find("Rado works at HMC").expect("span start");
        let span_end = span_start + "Rado works at HMC".len();
        let span = body
            .get(span_start..span_end)
            .expect("span should be valid for test body");
        let claim = Claim {
            id: "bbbbbbbbbbbbbbbb".to_string(),
            subject: "Rado".to_string(),
            predicate: "works_at".to_string(),
            object: "HMC".to_string(),
            note_id: note.fm.id.clone(),
            span_start,
            span_end,
            span_fingerprint: Claim::compute_fingerprint(span),
            valid_from: note.fm.created,
            valid_until: None,
            confidence: 1.0,
            privacy: Privacy::Private,
            extracted_by: "test".to_string(),
            extracted_at: Utc::now(),
        };
        store.upsert(&claim)?;

        let validator = CitationValidator {
            store: &store,
            index: &index,
            vault_root: &vault,
        };

        let answer = validator
            .validate("\"Completely unrelated quote\" [claim:bbbbbbbbbbbbbbbb].")
            .await?;
        assert_eq!(answer.checks[0].status, CitationStatus::QuoteMismatch);
        Ok(())
    }
}
