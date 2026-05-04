use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use memora_llm::{CompletionRequest, LlmClient, Message, Role};
use rusqlite::params;

use crate::challenger::{
    detect_contradictions, detect_recent_decisions, detect_stale_dependencies,
};
use crate::claims::ClaimStore;
use crate::consolidate::prompts::{
    REGION_DESCRIPTION_PROMPT, WORLD_MAP_EARLY_STAGES_FALLBACK, WORLD_MAP_PROMPT,
};
use crate::index::Index;
use crate::Claim;

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
        let all_claims = self.current_claims_all_regions()?;
        let note_regions = self.note_region_map()?;
        let todays_review = todays_review_findings(&all_claims, &note_regions);
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
            "\n## Today's review (auto-{})\n",
            Utc::now().format("%Y-%m-%d")
        ));
        if todays_review.is_empty() {
            markdown.push_str("- none\n");
        } else {
            for line in todays_review {
                markdown.push_str(&format!("- {line}\n"));
            }
        }

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

    fn current_claims_all_regions(&self) -> Result<Vec<Claim>> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, subject, predicate, object, note_id, span_start, span_end,
                    span_fingerprint, valid_from, valid_until, confidence, privacy,
                    extracted_by, extracted_at
             FROM claims
             WHERE valid_until IS NULL OR valid_until > ?",
        )?;
        let rows = stmt.query_map(params![Utc::now().to_rfc3339()], |row| {
            let valid_from: String = row.get(8)?;
            let valid_until: Option<String> = row.get(9)?;
            let privacy_raw: String = row.get(11)?;
            let extracted_at: String = row.get(13)?;
            Ok(Claim {
                id: row.get(0)?,
                subject: row.get(1)?,
                predicate: row.get(2)?,
                object: row.get(3)?,
                note_id: row.get(4)?,
                span_start: row.get(5)?,
                span_end: row.get(6)?,
                span_fingerprint: row.get(7)?,
                valid_from: chrono::DateTime::parse_from_rfc3339(&valid_from)
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            8,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?
                    .with_timezone(&Utc),
                valid_until: valid_until
                    .as_deref()
                    .map(chrono::DateTime::parse_from_rfc3339)
                    .transpose()
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            9,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?
                    .map(|v| v.with_timezone(&Utc)),
                confidence: row.get(10)?,
                privacy: privacy_raw.parse().map_err(|_| {
                    rusqlite::Error::FromSqlConversionFailure(
                        11,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "invalid privacy",
                        )),
                    )
                })?,
                extracted_by: row.get(12)?,
                extracted_at: chrono::DateTime::parse_from_rfc3339(&extracted_at)
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            13,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?
                    .with_timezone(&Utc),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn note_region_map(&self) -> Result<HashMap<String, String>> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare("SELECT id, region FROM notes")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, region) = row?;
            out.insert(id, region);
        }
        Ok(out)
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

fn todays_review_findings(claims: &[Claim], note_regions: &HashMap<String, String>) -> Vec<String> {
    #[derive(Clone)]
    struct ReviewLine {
        priority: u8,
        support: usize,
        text: String,
    }
    let mut lines = Vec::<ReviewLine>::new();

    for item in detect_contradictions(claims) {
        let left_regions = regions_for_notes(&item.left_source_note_ids, note_regions);
        let right_regions = regions_for_notes(&item.right_source_note_ids, note_regions);
        let cross_region = left_regions != right_regions;
        let merged = merge_notes(&item.left_source_note_ids, &item.right_source_note_ids);
        let support = item.left_source_note_ids.len() + item.right_source_note_ids.len();
        let priority = if cross_region { 1 } else { 3 };
        lines.push(ReviewLine {
            priority,
            support,
            text: format!(
                "**{}: {} disagreement** - {} ({} sources) vs {} ({} sources) [{}]",
                humanize(&item.subject),
                item.family,
                item.left_object,
                item.left_source_note_ids.len(),
                item.right_object,
                item.right_source_note_ids.len(),
                render_notes(&merged)
            ),
        });
    }

    for item in detect_stale_dependencies(claims) {
        let merged = merge_notes(
            &item.depends_on_source_note_ids,
            &item.superseded_source_note_ids,
        );
        let support = item.depends_on_source_note_ids.len() + item.superseded_source_note_ids.len();
        lines.push(ReviewLine {
            priority: 2,
            support,
            text: format!(
                "**{}** depends on `{}` which is superseded by `{}` [{}]",
                humanize(&item.dependent_subject),
                item.stale_subject,
                item.superseded_by,
                render_notes(&merged)
            ),
        });
    }

    for item in detect_recent_decisions(claims) {
        lines.push(ReviewLine {
            priority: 4,
            support: item.source_note_ids.len(),
            text: format!(
                "**{} -> {}** ({} supporting claims across {})",
                humanize(&item.subject),
                humanize(&item.object),
                item.source_note_ids.len(),
                render_notes(&item.source_note_ids)
            ),
        });
    }

    lines.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| b.support.cmp(&a.support))
            .then_with(|| a.text.cmp(&b.text))
    });
    lines.into_iter().take(3).map(|line| line.text).collect()
}

fn regions_for_notes(notes: &[String], note_regions: &HashMap<String, String>) -> BTreeSet<String> {
    notes
        .iter()
        .filter_map(|note| note_regions.get(note).cloned())
        .collect()
}

fn merge_notes(left: &[String], right: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for note in left.iter().chain(right.iter()) {
        if seen.insert(note.clone()) {
            out.push(note.clone());
        }
    }
    out
}

fn render_notes(notes: &[String]) -> String {
    let shown = notes
        .iter()
        .take(3)
        .map(|note| format!("[[{note}]]"))
        .collect::<Vec<_>>();
    if notes.len() <= 3 {
        return shown.join(", ");
    }
    format!("{}, and {} more", shown.join(", "), notes.len() - 3)
}

fn humanize(value: &str) -> String {
    value.replace('-', " ")
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
            id: Claim::compute_id(subject, "has", Some(object), note_id, span_start),
            subject: subject.to_string(),
            predicate: "has".to_string(),
            object: Some(object.to_string()),
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
