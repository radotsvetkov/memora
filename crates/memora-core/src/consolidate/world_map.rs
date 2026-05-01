use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use memora_llm::{CompletionRequest, LlmClient, Message, Role};
use rusqlite::params;

use crate::claims::ClaimStore;
use crate::consolidate::prompts::{
    REGION_DESCRIPTION_PROMPT, WORLD_MAP_EARLY_STAGES_FALLBACK, WORLD_MAP_PROMPT,
};
use crate::index::Index;

pub struct WorldMapWriter<'a> {
    pub db: &'a Index,
    pub claim_store: &'a ClaimStore<'a>,
    pub llm: &'a dyn LlmClient,
    pub vault: &'a Path,
}

#[derive(Debug, Clone)]
struct RegionStats {
    notes_count: usize,
    claim_count: usize,
    decision_count: usize,
    stale_count: usize,
}

impl<'a> WorldMapWriter<'a> {
    pub async fn rebuild(&self) -> Result<()> {
        let regions = self.regions()?;
        let mut descriptions = BTreeMap::<String, String>::new();
        let mut stats = BTreeMap::<String, RegionStats>::new();
        let mut total_claims = 0usize;

        for region in &regions {
            let region_stats = self.region_stats(region)?;
            total_claims += region_stats.claim_count;
            stats.insert(region.clone(), region_stats.clone());
            let subjects = self.sample_subjects(region)?;
            let description = if region_stats.claim_count < 3 || region == "default" {
                "General notes (uncategorized)".to_string()
            } else {
                self.region_description(region, &subjects).await?
            };
            descriptions.insert(region.clone(), description);
        }

        let overview = if total_claims < 10 {
            WORLD_MAP_EARLY_STAGES_FALLBACK.to_string()
        } else {
            self.overview(&descriptions, &stats).await?
        };
        let mut markdown = String::from("# World Map\n\n");
        markdown.push_str(&format!("_{}_\n\n## Regions\n", overview));

        for region in regions {
            let description = descriptions.get(&region).cloned().unwrap_or_default();
            let data = stats.get(&region).cloned().unwrap_or(RegionStats {
                notes_count: 0,
                claim_count: 0,
                decision_count: 0,
                stale_count: 0,
            });
            markdown.push_str(&format!(
                "- **{region}** — {description} ({} notes, {} claims, {} decisions, {} stale)\n",
                data.notes_count, data.claim_count, data.decision_count, data.stale_count
            ));
        }
        markdown.push_str(&format!(
            "\n## Today's review (auto-{})\n(challenger placeholder)\n",
            Utc::now().format("%Y-%m-%d")
        ));

        let output = self.vault.join("world_map.md");
        atomic_write(&output, &markdown)?;
        Ok(())
    }

    fn regions(&self) -> Result<Vec<String>> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare("SELECT DISTINCT region FROM notes ORDER BY region ASC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn region_stats(&self, region: &str) -> Result<RegionStats> {
        let conn = self.db.pool.get()?;
        let notes_count = conn.query_row(
            "SELECT COUNT(*) FROM notes WHERE region = ?",
            params![region],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let claim_count = conn.query_row(
            "SELECT COUNT(*)
             FROM claims c
             JOIN notes n ON n.id = c.note_id
             WHERE n.region = ?
               AND (c.valid_until IS NULL OR c.valid_until > ?)",
            params![region, Utc::now().to_rfc3339()],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let decision_count = conn.query_row(
            "SELECT COUNT(*)
             FROM decisions d
             JOIN claims c ON c.id = d.claim_id
             JOIN notes n ON n.id = c.note_id
             WHERE n.region = ?",
            params![region],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let stale_count = conn.query_row(
            "SELECT COUNT(*)
             FROM stale_claims s
             JOIN claims c ON c.id = s.claim_id
             JOIN notes n ON n.id = c.note_id
             WHERE n.region = ?",
            params![region],
            |row| row.get::<_, i64>(0),
        )? as usize;

        Ok(RegionStats {
            notes_count,
            claim_count,
            decision_count,
            stale_count,
        })
    }

    fn sample_subjects(&self, region: &str) -> Result<Vec<String>> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT c.subject
             FROM claims c
             JOIN notes n ON n.id = c.note_id
             WHERE n.region = ?
             GROUP BY c.subject
             ORDER BY COUNT(*) DESC, c.subject ASC
             LIMIT 5",
        )?;
        let rows = stmt.query_map(params![region], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    async fn region_description(&self, region: &str, subjects: &[String]) -> Result<String> {
        let response = self
            .llm
            .complete(CompletionRequest {
                model: None,
                system: Some(REGION_DESCRIPTION_PROMPT.to_string()),
                messages: vec![Message {
                    role: Role::User,
                    content: format!("Region: {region}\nSample subjects: {}", subjects.join(", ")),
                }],
                max_tokens: 80,
                temperature: 0.1,
                json_mode: false,
            })
            .await?;
        Ok(response.text.trim().replace('\n', " "))
    }

    async fn overview(
        &self,
        descriptions: &BTreeMap<String, String>,
        stats: &BTreeMap<String, RegionStats>,
    ) -> Result<String> {
        let lines = descriptions
            .iter()
            .map(|(region, description)| {
                let data = stats.get(region).expect("stats key should exist");
                format!(
                    "- {region}: {description} (notes={}, claims={}, decisions={}, stale={})",
                    data.notes_count, data.claim_count, data.decision_count, data.stale_count
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let response = self
            .llm
            .complete(CompletionRequest {
                model: None,
                system: Some(WORLD_MAP_PROMPT.to_string()),
                messages: vec![Message {
                    role: Role::User,
                    content: lines,
                }],
                max_tokens: 300,
                temperature: 0.1,
                json_mode: false,
            })
            .await?;
        Ok(response.text.trim().replace('\n', " "))
    }
}

fn atomic_write(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("md.tmp");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use memora_llm::{CompletionRequest, CompletionResponse, LlmClient, LlmDestination, LlmError};
    use tempfile::tempdir;

    use crate::claims::{Claim, ClaimStore};
    use crate::note::{self, Frontmatter, Note, NoteSource, Privacy};

    fn seed_note(vault: &Path, region: &str, id: &str) -> Result<()> {
        let dir = vault.join(region);
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{id}.md"));
        let body = format!("Body {id}");
        let note = Note {
            path: path.clone(),
            fm: Frontmatter {
                id: id.to_string(),
                region: region.to_string(),
                source: NoteSource::Personal,
                privacy: Privacy::Private,
                created: Utc
                    .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                    .single()
                    .expect("valid datetime"),
                updated: Utc
                    .with_ymd_and_hms(2026, 1, 2, 0, 0, 0)
                    .single()
                    .expect("valid datetime"),
                summary: format!("s-{id}"),
                tags: vec![],
                refs: vec![],
            },
            body,
            wikilinks: vec![],
        };
        fs::write(&path, note::render(&note))?;
        Ok(())
    }

    fn make_claim(note_id: &str, subject: &str, object: &str, span_start: usize) -> Claim {
        let valid_from = Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid datetime");
        Claim {
            id: Claim::compute_id(subject, "has", object, note_id, span_start),
            subject: subject.to_string(),
            predicate: "has".to_string(),
            object: object.to_string(),
            note_id: note_id.to_string(),
            span_start,
            span_end: span_start.saturating_add(4),
            span_fingerprint: Claim::compute_fingerprint("Body"),
            valid_from,
            valid_until: None,
            confidence: 0.9,
            privacy: Privacy::Private,
            extracted_by: "test/world-map".to_string(),
            extracted_at: valid_from,
        }
    }

    struct PanicWorldOverviewLlm;

    #[async_trait]
    impl LlmClient for PanicWorldOverviewLlm {
        async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
            if req
                .system
                .as_deref()
                .unwrap_or("")
                .contains("vault's shape")
            {
                panic!("world map overview LLM must be skipped below 10 total claims");
            }
            Ok(CompletionResponse {
                text: "Synthetic one-line region summary.".to_string(),
                model: "mock/world-map".to_string(),
                input_tokens: None,
                output_tokens: None,
            })
        }

        fn model_name(&self) -> &str {
            "mock/world-map"
        }

        fn destination(&self) -> LlmDestination {
            LlmDestination::Local
        }
    }

    #[tokio::test]
    async fn world_map_uses_static_fallback_below_threshold() -> Result<()> {
        let temp = tempdir()?;
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault)?;
        let db_path = temp.path().join("wm.sqlite");
        let index = Index::open(&db_path)?;
        let store = ClaimStore::new(&index);

        for id in ["w1", "w2", "w3"] {
            seed_note(&vault, "work", id)?;
            let path = vault.join("work").join(format!("{id}.md"));
            let parsed = note::parse(&path)?;
            index.upsert_note(&parsed, &parsed.body)?;
        }
        for id in ["m1", "m2"] {
            seed_note(&vault, "misc", id)?;
            let path = vault.join("misc").join(format!("{id}.md"));
            let parsed = note::parse(&path)?;
            index.upsert_note(&parsed, &parsed.body)?;
        }

        store.upsert(&make_claim("w1", "S1", "a", 0))?;
        store.upsert(&make_claim("w2", "S2", "b", 1))?;
        store.upsert(&make_claim("w3", "S3", "c", 2))?;
        store.upsert(&make_claim("m1", "T1", "d", 0))?;
        store.upsert(&make_claim("m2", "T2", "e", 1))?;

        let writer = WorldMapWriter {
            db: &index,
            claim_store: &store,
            llm: &PanicWorldOverviewLlm,
            vault: &vault,
        };
        writer.rebuild().await?;

        let md = fs::read_to_string(vault.join("world_map.md"))?;
        assert!(md.contains(WORLD_MAP_EARLY_STAGES_FALLBACK));
        assert!(md.contains("General notes (uncategorized)"));
        Ok(())
    }
}
