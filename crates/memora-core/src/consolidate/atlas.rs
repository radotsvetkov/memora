use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use memora_llm::{CompletionRequest, LlmClient, Message, Role};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;

use crate::claims::{Claim, ClaimStore};
use crate::consolidate::prompts::{REGION_OVERVIEW_PROMPT, SUBREGION_PROPOSAL_PROMPT};
use crate::index::Index;
use crate::note::{self, Privacy};

const MIN_CLAIMS_FOR_SYNTHESIS: usize = 5;

const DEFAULT_SUBREGION_THRESHOLD: usize = 200;

const ATLAS_LOW_CONTENT_OVERVIEW: &str =
    "Too few claims for synthesis. Add more notes to this region.";

pub struct AtlasWriter<'a> {
    pub db: &'a Index,
    pub claim_store: &'a ClaimStore<'a>,
    pub llm: &'a dyn LlmClient,
    pub vault: &'a Path,
}

#[derive(Debug, Default)]
pub struct RebuildReport {
    pub rebuilt_regions: Vec<String>,
    pub failed_regions: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct RegionNote {
    id: String,
    path: String,
    qvalue: f64,
}

#[derive(Debug, Clone)]
struct DecisionRow {
    title: String,
    status: String,
    decided_on: String,
    claim_id: String,
}

#[derive(Debug, Clone)]
struct StaleRow {
    claim_id: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct SubregionProposalResponse {
    proposed_subregions: Vec<SubregionProposal>,
}

#[derive(Debug, Deserialize)]
struct SubregionProposal {
    name: String,
    sample_subjects: Vec<String>,
    claim_ids: Vec<String>,
}

impl<'a> AtlasWriter<'a> {
    pub async fn rebuild_region(&self, region: &str) -> Result<()> {
        let notes = self.region_notes(region)?;
        let note_ids = notes
            .iter()
            .map(|note| note.id.clone())
            .collect::<HashSet<_>>();
        let note_qvalue = notes
            .iter()
            .map(|note| (note.id.clone(), note.qvalue))
            .collect::<HashMap<_, _>>();
        let claims = self.current_claims_for_notes(&notes)?;
        let mut claims_by_subject = BTreeMap::<String, Vec<Claim>>::new();
        let mut subject_qvalue = HashMap::<String, f64>::new();
        for claim in &claims {
            claims_by_subject
                .entry(claim.subject.clone())
                .or_default()
                .push(claim.clone());
            let max_q = note_qvalue.get(&claim.note_id).copied().unwrap_or(0.0);
            subject_qvalue
                .entry(claim.subject.clone())
                .and_modify(|value| *value = value.max(max_q))
                .or_insert(max_q);
        }
        let mut subjects = claims_by_subject.keys().cloned().collect::<Vec<_>>();
        subjects.sort_by(|a, b| {
            let qa = subject_qvalue.get(a).copied().unwrap_or_default();
            let qb = subject_qvalue.get(b).copied().unwrap_or_default();
            qb.partial_cmp(&qa)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.cmp(b))
        });

        let overview = if claims.len() < MIN_CLAIMS_FOR_SYNTHESIS {
            ATLAS_LOW_CONTENT_OVERVIEW.to_string()
        } else {
            self.region_overview(region, &claims).await?
        };
        let decisions = self.decisions_for_region(region)?;
        let stale = self.stale_for_region(region)?;
        let contradictions = self.contradictions_for_region(&note_ids)?;
        let atlas = build_atlas_markdown(
            region,
            &overview,
            &subjects,
            &claims_by_subject,
            &decisions,
            &stale,
            &contradictions,
        );
        let atlas_path = self.vault.join(region).join("_atlas.md");
        atomic_write(&atlas_path, &atlas)?;

        let index_path = self.vault.join(region).join("_index.md");
        if claims.len() < MIN_CLAIMS_FOR_SYNTHESIS {
            if index_path.exists() {
                fs::remove_file(&index_path)?;
            }
        } else {
            let index_md = self
                .region_index(region, &subjects, &claims_by_subject)
                .await?;
            atomic_write(&index_path, &index_md)?;
        }

        if claims.len() > DEFAULT_SUBREGION_THRESHOLD {
            self.maybe_split_subregions(region, &notes, &claims).await?;
        }
        Ok(())
    }

    pub async fn rebuild_all_changed(&self) -> RebuildReport {
        let mut report = RebuildReport::default();
        let regions = match self.changed_regions() {
            Ok(regions) => regions,
            Err(err) => {
                report
                    .failed_regions
                    .push(("__changed_region_discovery__".to_string(), err.to_string()));
                return report;
            }
        };
        for region in regions {
            match self.rebuild_region(&region).await {
                Ok(()) => report.rebuilt_regions.push(region),
                Err(err) => report.failed_regions.push((region, err.to_string())),
            }
        }
        if report.failed_regions.is_empty() {
            if let Err(err) = self.record_consolidation_run("atlas") {
                report
                    .failed_regions
                    .push(("__run_record__".to_string(), err.to_string()));
            }
        }
        report
    }

    fn region_notes(&self, region: &str) -> Result<Vec<RegionNote>> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT id, path, qvalue
             FROM notes
             WHERE region = ?
             ORDER BY qvalue DESC, id ASC",
        )?;
        let rows = stmt.query_map(params![region], |row| {
            Ok(RegionNote {
                id: row.get(0)?,
                path: row.get(1)?,
                qvalue: row.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn current_claims_for_notes(&self, notes: &[RegionNote]) -> Result<Vec<Claim>> {
        let mut all = Vec::new();
        for note in notes {
            all.extend(self.claim_store.list_for_note(&note.id)?);
        }
        let now = Utc::now();
        Ok(all
            .into_iter()
            .filter(|claim| match claim.valid_until {
                None => true,
                Some(valid_until) => valid_until > now,
            })
            .collect())
    }

    async fn region_overview(&self, region: &str, claims: &[Claim]) -> Result<String> {
        let compact = claims
            .iter()
            .map(|claim| {
                format!(
                    "[\"{}\",\"{}\",\"{}\",\"{}\"]",
                    claim.id, claim.subject, claim.predicate, claim.object
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let response = self
            .llm
            .complete(CompletionRequest {
                model: None,
                system: Some(REGION_OVERVIEW_PROMPT.to_string()),
                messages: vec![Message {
                    role: Role::User,
                    content: format!("Region: {region}\nClaims:\n{compact}"),
                }],
                max_tokens: 220,
                temperature: 0.1,
                json_mode: false,
            })
            .await?;
        Ok(response.text.trim().replace('\n', " "))
    }

    async fn region_index(
        &self,
        region: &str,
        subjects: &[String],
        claims_by_subject: &BTreeMap<String, Vec<Claim>>,
    ) -> Result<String> {
        let top = subjects
            .iter()
            .take(5)
            .filter_map(|subject| {
                claims_by_subject
                    .get(subject)
                    .and_then(|claims| claims.first())
                    .map(|claim| format!("{subject} [claim:{}]", claim.id))
            })
            .collect::<Vec<_>>();
        let response = self
            .llm
            .complete(CompletionRequest {
                model: None,
                system: Some(REGION_OVERVIEW_PROMPT.to_string()),
                messages: vec![Message {
                    role: Role::User,
                    content: format!(
                        "Write 200-300 words about region '{region}'. Cite top subjects using existing markers.\nTop subjects:\n{}",
                        top.join("\n")
                    ),
                }],
                max_tokens: 520,
                temperature: 0.1,
                json_mode: false,
            })
            .await?;
        Ok(format!(
            "# Index: {region}\n\n_{}_\n\n## Top subjects\n{}\n",
            response.text.trim(),
            top.iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        ))
    }

    fn decisions_for_region(&self, region: &str) -> Result<Vec<DecisionRow>> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT d.title, d.status, d.decided_on, d.claim_id
             FROM decisions d
             JOIN claims c ON c.id = d.claim_id
             JOIN notes n ON n.id = c.note_id
             WHERE n.region = ?
             ORDER BY d.decided_on DESC",
        )?;
        let rows = stmt.query_map(params![region], |row| {
            Ok(DecisionRow {
                title: row.get(0)?,
                status: row.get(1)?,
                decided_on: row.get(2)?,
                claim_id: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn stale_for_region(&self, region: &str) -> Result<Vec<StaleRow>> {
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT sc.claim_id, sc.reason
             FROM stale_claims sc
             JOIN claims c ON c.id = sc.claim_id
             JOIN notes n ON n.id = c.note_id
             WHERE n.region = ?
             ORDER BY sc.marked_at DESC, sc.claim_id ASC",
        )?;
        let rows = stmt.query_map(params![region], |row| {
            Ok(StaleRow {
                claim_id: row.get(0)?,
                reason: row.get(1)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn contradictions_for_region(
        &self,
        note_ids: &HashSet<String>,
    ) -> Result<Vec<(String, String)>> {
        let pairs = self.claim_store.contradictions_unack()?;
        let mut out = Vec::new();
        for (left, right) in pairs {
            if note_ids.contains(&left.note_id) || note_ids.contains(&right.note_id) {
                out.push((left.id, right.id));
            }
        }
        Ok(out)
    }

    fn changed_regions(&self) -> Result<Vec<String>> {
        let last_consolidation = self.last_consolidation_run("atlas")?;
        let mut changed = BTreeSet::new();
        let conn = self.db.pool.get()?;

        let mut updated_stmt = conn.prepare(
            "SELECT DISTINCT region
             FROM notes
             WHERE updated > ?",
        )?;
        let updated_rows =
            updated_stmt.query_map(params![last_consolidation], |row| row.get::<_, String>(0))?;
        for row in updated_rows {
            changed.insert(row?);
        }

        let mut stale_stmt = conn.prepare(
            "SELECT DISTINCT n.region
             FROM stale_claims sc
             JOIN claims c ON c.id = sc.claim_id
             JOIN notes n ON n.id = c.note_id",
        )?;
        let stale_rows = stale_stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in stale_rows {
            changed.insert(row?);
        }

        let mut note_region = HashMap::<String, String>::new();
        let mut note_stmt = conn.prepare("SELECT id, region FROM notes")?;
        let note_rows = note_stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in note_rows {
            let (id, region) = row?;
            note_region.insert(id, region);
        }
        for (left, right) in self.claim_store.contradictions_unack()? {
            if let Some(region) = note_region.get(&left.note_id) {
                changed.insert(region.clone());
            }
            if let Some(region) = note_region.get(&right.note_id) {
                changed.insert(region.clone());
            }
        }

        Ok(changed.into_iter().collect())
    }

    fn last_consolidation_run(&self, scope: &str) -> Result<String> {
        let conn = self.db.pool.get()?;
        let value = conn
            .query_row(
                "SELECT completed_at
                 FROM consolidation_runs
                 WHERE scope = ?
                 ORDER BY completed_at DESC
                 LIMIT 1",
                params![scope],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(value.unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string()))
    }

    fn record_consolidation_run(&self, scope: &str) -> Result<()> {
        let conn = self.db.pool.get()?;
        conn.execute(
            "INSERT INTO consolidation_runs (scope, completed_at) VALUES (?, ?)",
            params![scope, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    async fn maybe_split_subregions(
        &self,
        region: &str,
        notes: &[RegionNote],
        claims: &[Claim],
    ) -> Result<()> {
        let prompt_payload = claims
            .iter()
            .map(|claim| {
                format!(
                    "{{\"claim_id\":\"{}\",\"subject\":\"{}\",\"region\":\"{}\"}}",
                    claim.id, claim.subject, region
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let response = self
            .llm
            .complete(CompletionRequest {
                model: None,
                system: Some(SUBREGION_PROPOSAL_PROMPT.to_string()),
                messages: vec![Message {
                    role: Role::User,
                    content: prompt_payload,
                }],
                max_tokens: 1_024,
                temperature: 0.0,
                json_mode: true,
            })
            .await?;
        let parsed: SubregionProposalResponse =
            serde_json::from_str(response.text.trim()).context("parse SUBREGION proposal JSON")?;
        if parsed.proposed_subregions.is_empty() {
            return Ok(());
        }
        let note_paths = notes
            .iter()
            .map(|note| (note.id.clone(), PathBuf::from(note.path.clone())))
            .collect::<HashMap<_, _>>();
        let mut claim_to_note = HashMap::<String, String>::new();
        for claim in claims {
            claim_to_note.insert(claim.id.clone(), claim.note_id.clone());
        }
        let mut moved_notes = HashSet::new();
        for proposal in parsed.proposed_subregions {
            if proposal.sample_subjects.is_empty() {
                continue;
            }
            let subregion_name = sanitize_segment(&proposal.name);
            if subregion_name.is_empty() {
                continue;
            }
            let subregion = format!("{region}/{subregion_name}");
            let subregion_dir = self.vault.join(&subregion);
            fs::create_dir_all(&subregion_dir)?;

            for claim_id in proposal.claim_ids {
                let Some(note_id) = claim_to_note.get(&claim_id).cloned() else {
                    continue;
                };
                if moved_notes.contains(&note_id) {
                    continue;
                }
                let Some(old_path) = note_paths.get(&note_id).cloned() else {
                    continue;
                };
                let Some(file_name) = old_path.file_name() else {
                    continue;
                };
                let new_path = subregion_dir.join(file_name);
                if new_path.exists() {
                    continue;
                }
                let mut parsed_note = note::parse(&old_path)
                    .with_context(|| format!("parse note before move: {}", old_path.display()))?;
                parsed_note.fm.region = subregion.clone();
                fs::write(&old_path, note::render(&parsed_note))?;
                fs::rename(&old_path, &new_path)?;
                let reparsed = note::parse(&new_path)?;
                self.db.upsert_note(&reparsed, &reparsed.body)?;
                moved_notes.insert(note_id);
            }
        }
        Ok(())
    }
}

fn build_atlas_markdown(
    region: &str,
    overview: &str,
    subjects: &[String],
    claims_by_subject: &BTreeMap<String, Vec<Claim>>,
    decisions: &[DecisionRow],
    stale: &[StaleRow],
    contradictions: &[(String, String)],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Atlas: {region}\n\n_{overview}_\n\n## Subjects\n"
    ));
    for subject in subjects {
        out.push_str(&format!("### {subject}\n"));
        if let Some(claims) = claims_by_subject.get(subject) {
            for claim in claims {
                if claim.privacy == Privacy::Secret {
                    out.push_str(&format!(
                        "- [claim:{}] [redacted] [redacted] (source [[{}]])\n",
                        claim.id, claim.note_id
                    ));
                } else {
                    out.push_str(&format!(
                        "- [claim:{}] {} {} (source note: [[{}]])\n",
                        claim.id, claim.predicate, claim.object, claim.note_id
                    ));
                }
            }
        }
        out.push('\n');
    }

    out.push_str("## Recent decisions\n");
    if decisions.is_empty() {
        out.push_str("- none\n");
    } else {
        for item in decisions {
            out.push_str(&format!(
                "- {} ({}, {}, [claim:{}])\n",
                item.title, item.status, item.decided_on, item.claim_id
            ));
        }
    }

    out.push_str("\n## Stale dependencies\n");
    if stale.is_empty() {
        out.push_str("- none\n");
    } else {
        for item in stale {
            out.push_str(&format!("- [claim:{}] {}\n", item.claim_id, item.reason));
        }
    }

    out.push_str("\n## Contradictions\n");
    if contradictions.is_empty() {
        out.push_str("- none\n");
    } else {
        for (left, right) in contradictions {
            out.push_str(&format!("- [claim:{left}] contradicts [claim:{right}]\n"));
        }
    }
    out
}

fn atomic_write(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("md.tmp");
    fs::write(&tmp_path, content)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

fn sanitize_segment(input: &str) -> String {
    input
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| match ch {
            'a'..='z' | '0'..='9' | '-' | '_' => ch,
            ' ' | '/' => '-',
            _ => '-',
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use memora_llm::{CompletionRequest, CompletionResponse, LlmClient, LlmDestination, LlmError};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::tempdir;

    use crate::claims::{Claim, ClaimStore};
    use crate::note::{self, Frontmatter, Note, NoteSource, Privacy};

    fn write_note(vault: &Path, region: &str, id: &str) -> Result<PathBuf> {
        let region_dir = vault.join(region);
        fs::create_dir_all(&region_dir)?;
        let path = region_dir.join(format!("{id}.md"));
        let body = format!("Body for {id}.");
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
                summary: format!("summary-{id}"),
                tags: vec![],
                refs: vec![],
            },
            body,
            wikilinks: vec![],
        };
        fs::write(&path, note::render(&note))?;
        Ok(path)
    }

    fn make_claim(note_id: &str, subject: &str, idx: usize) -> Claim {
        let valid_from = Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid datetime");
        let object = format!("obj{idx}");
        Claim {
            id: Claim::compute_id(subject, "relates_to", &object, note_id, idx),
            subject: subject.to_string(),
            predicate: "relates_to".to_string(),
            object,
            note_id: note_id.to_string(),
            span_start: 0,
            span_end: 4,
            span_fingerprint: Claim::compute_fingerprint("Body"),
            valid_from,
            valid_until: None,
            confidence: 0.9,
            privacy: Privacy::Private,
            extracted_by: "test/atlas".to_string(),
            extracted_at: valid_from,
        }
    }

    struct CountingLlm {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LlmClient for CountingLlm {
        async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(CompletionResponse {
                text: "unexpected LLM output".to_string(),
                model: "mock/count".to_string(),
                input_tokens: None,
                output_tokens: None,
            })
        }

        fn model_name(&self) -> &str {
            "mock/count"
        }

        fn destination(&self) -> LlmDestination {
            LlmDestination::Local
        }
    }

    #[tokio::test]
    async fn atlas_skips_llm_when_below_min_claims() -> Result<()> {
        let temp = tempdir()?;
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault)?;
        let db_path = temp.path().join("memora.db");
        let index = Index::open(&db_path)?;
        let store = ClaimStore::new(&index);

        for i in 0..2 {
            let id = format!("low-{i}");
            let path = write_note(&vault, "thin", &id)?;
            let parsed = note::parse(&path)?;
            index.upsert_note(&parsed, &parsed.body)?;
            store.upsert(&make_claim(&id, "Topic", i as usize))?;
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let llm = CountingLlm {
            calls: Arc::clone(&calls),
        };
        let writer = AtlasWriter {
            db: &index,
            claim_store: &store,
            llm: &llm,
            vault: &vault,
        };
        writer.rebuild_region("thin").await?;

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let atlas_path = vault.join("thin").join("_atlas.md");
        let index_path = vault.join("thin").join("_index.md");
        assert!(atlas_path.exists());
        assert!(!index_path.exists());
        let atlas_text = fs::read_to_string(&atlas_path)?;
        assert!(atlas_text.contains(ATLAS_LOW_CONTENT_OVERVIEW));
        Ok(())
    }
}
