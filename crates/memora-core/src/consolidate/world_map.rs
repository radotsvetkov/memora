use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use memora_llm::{CompletionRequest, LlmClient, Message, Role};
use rusqlite::params;

use crate::claims::ClaimStore;
use crate::consolidate::prompts::{REGION_DESCRIPTION_PROMPT, WORLD_MAP_PROMPT};
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

        for region in &regions {
            let region_stats = self.region_stats(region)?;
            let subjects = self.sample_subjects(region)?;
            let description = self.region_description(region, &subjects).await?;
            descriptions.insert(region.clone(), description);
            stats.insert(region.clone(), region_stats);
        }

        let overview = self.overview(&descriptions, &stats).await?;
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
