use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use memora_llm::{CompletionRequest, LlmClient, Message, Role};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;

use crate::challenger::{
    detect_contradictions, detect_open_questions, detect_recent_decisions,
    detect_stale_dependencies, Contradiction as ChallengerContradiction, Decision, OpenQuestion,
    StaleDep,
};
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
        let all_claims = self.current_claims_all_regions()?;
        let note_regions = self.note_region_map()?;
        let decisions = region_decisions(
            region,
            &note_ids,
            &all_claims,
            &note_regions,
            detect_recent_decisions(&all_claims),
        );
        let stale = region_stale_dependencies(
            region,
            &note_ids,
            &all_claims,
            &note_regions,
            detect_stale_dependencies(&all_claims),
        );
        let contradictions = region_contradictions(
            region,
            &note_ids,
            &all_claims,
            &note_regions,
            detect_contradictions(&all_claims),
        );
        let open_questions = region_open_questions(
            region,
            &note_ids,
            &all_claims,
            &note_regions,
            &decisions,
            detect_open_questions(&all_claims),
        );
        let atlas = build_atlas_markdown(
            region,
            &overview,
            &subjects,
            &claims_by_subject,
            AtlasFindings {
                decisions: &decisions,
                stale: &stale,
                contradictions: &contradictions,
                open_questions: &open_questions,
            },
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
                    claim.id,
                    claim.subject,
                    claim.predicate,
                    claim.object.as_deref().unwrap_or("")
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

struct AtlasFindings<'a> {
    decisions: &'a [Decision],
    stale: &'a [StaleDep],
    contradictions: &'a [ChallengerContradiction],
    open_questions: &'a [OpenQuestion],
}

fn build_atlas_markdown(
    region: &str,
    overview: &str,
    subjects: &[String],
    claims_by_subject: &BTreeMap<String, Vec<Claim>>,
    findings: AtlasFindings<'_>,
) -> String {
    #[derive(Debug)]
    struct RenderTuple {
        predicate: String,
        object: Option<String>,
        representative_claim_id: String,
        source_notes: Vec<String>,
        privacy: Privacy,
    }

    fn sources_suffix(source_notes: &[String]) -> String {
        if source_notes.len() == 1 {
            return format!("(source note: [[{}]])", source_notes[0]);
        }
        let (visible, remainder) = if source_notes.len() > 12 {
            (&source_notes[..10], source_notes.len() - 10)
        } else {
            (source_notes, 0)
        };
        let mut rendered = visible
            .iter()
            .map(|note| format!("[[{note}]]"))
            .collect::<Vec<_>>()
            .join(", ");
        if remainder > 0 {
            rendered.push_str(&format!(", ... ({remainder} more)"));
        }
        format!("({} sources: {rendered})", source_notes.len())
    }

    fn deduped_claim_tuples(claims: &[Claim]) -> Vec<RenderTuple> {
        let mut grouped = BTreeMap::<(String, Option<String>), Vec<&Claim>>::new();
        for claim in claims {
            grouped
                .entry((claim.predicate.clone(), claim.object.clone()))
                .or_default()
                .push(claim);
        }

        let mut out = grouped
            .into_iter()
            .map(|((predicate, object), group_claims)| {
                let mut claim_ids = group_claims
                    .iter()
                    .map(|claim| claim.id.clone())
                    .collect::<Vec<_>>();
                claim_ids.sort();
                let representative_claim_id = claim_ids
                    .first()
                    .cloned()
                    .expect("grouped claims should be non-empty");

                let mut seen_notes = HashSet::new();
                let mut source_notes = Vec::new();
                for claim in &group_claims {
                    if seen_notes.insert(claim.note_id.clone()) {
                        source_notes.push(claim.note_id.clone());
                    }
                }

                let privacy = group_claims
                    .iter()
                    .map(|claim| claim.privacy)
                    .max()
                    .unwrap_or(Privacy::Private);

                RenderTuple {
                    predicate,
                    object,
                    representative_claim_id,
                    source_notes,
                    privacy,
                }
            })
            .collect::<Vec<_>>();

        out.sort_by(|a, b| {
            b.source_notes
                .len()
                .cmp(&a.source_notes.len())
                .then_with(|| a.predicate.cmp(&b.predicate))
                .then_with(|| {
                    a.object
                        .as_deref()
                        .unwrap_or("")
                        .cmp(b.object.as_deref().unwrap_or(""))
                })
        });
        out
    }

    let mut out = String::new();
    out.push_str(&format!(
        "# Atlas: {region}\n\n_{overview}_\n\n## Subjects\n"
    ));
    for subject in subjects {
        out.push_str(&format!("### {subject}\n"));
        if let Some(claims) = claims_by_subject.get(subject) {
            for tuple in deduped_claim_tuples(claims) {
                if tuple.privacy == Privacy::Secret {
                    out.push_str(&format!(
                        "- [claim:{}] [redacted] [redacted] {}\n",
                        tuple.representative_claim_id,
                        sources_suffix(&tuple.source_notes)
                    ));
                } else {
                    let object_display = tuple.object.as_deref().unwrap_or("(unary)");
                    out.push_str(&format!(
                        "- [claim:{}] {} {} {}\n",
                        tuple.representative_claim_id,
                        tuple.predicate,
                        object_display,
                        sources_suffix(&tuple.source_notes)
                    ));
                }
            }
        }
        out.push('\n');
    }

    out.push_str("## Recent decisions\n");
    if findings.decisions.is_empty() {
        out.push_str("- none\n");
    } else {
        for item in findings.decisions {
            out.push_str(&format!(
                "- **{} -> {}** ({} supporting claims across {})\n",
                humanize_term(&item.subject),
                humanize_term(&item.object),
                item.source_note_ids.len(),
                source_note_list(&item.source_note_ids)
            ));
        }
    }

    out.push_str("\n## Stale dependencies\n");
    if findings.stale.is_empty() {
        out.push_str("- none\n");
    } else {
        for item in findings.stale {
            out.push_str(&format!(
                "- **{}** depends on `{}` which is superseded by `{}` ({} depends_on sources; {} superseded_by sources: {})\n",
                humanize_term(&item.dependent_subject),
                item.stale_subject,
                item.superseded_by,
                item.depends_on_source_note_ids.len(),
                item.superseded_source_note_ids.len(),
                source_note_list(&merged_sources(
                    &item.depends_on_source_note_ids,
                    &item.superseded_source_note_ids
                ))
            ));
        }
    }

    out.push_str("\n## Contradictions\n");
    if findings.contradictions.is_empty() {
        out.push_str("- none\n");
    } else {
        for item in findings.contradictions {
            out.push_str(&format!(
                "- **{}: {} disagreement** - {} ({} sources) vs {} ({} sources) [{}]\n",
                humanize_term(&item.subject),
                item.family,
                item.left_object,
                item.left_source_note_ids.len(),
                item.right_object,
                item.right_source_note_ids.len(),
                source_note_list(&merged_sources(
                    &item.left_source_note_ids,
                    &item.right_source_note_ids
                ))
            ));
        }
    }
    out.push_str("\n## Open questions\n");
    if findings.open_questions.is_empty() {
        out.push_str("- none\n");
    } else {
        for item in findings.open_questions {
            out.push_str(&format!(
                "- **{}: {}** - {} ({} supporting claims across {})\n",
                humanize_term(&item.subject),
                item.object,
                item.family.replace('_', " "),
                item.source_note_ids.len(),
                source_note_list(&item.source_note_ids)
            ));
        }
    }
    out
}

fn merged_sources(left: &[String], right: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for note in left.iter().chain(right.iter()) {
        if seen.insert(note.clone()) {
            out.push(note.clone());
        }
    }
    out
}

fn source_note_list(notes: &[String]) -> String {
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

fn humanize_term(value: &str) -> String {
    value.replace('-', " ")
}

fn region_decisions(
    region: &str,
    note_ids: &HashSet<String>,
    all_claims: &[Claim],
    note_regions: &HashMap<String, String>,
    decisions: Vec<Decision>,
) -> Vec<Decision> {
    let claim_by_id = all_claims
        .iter()
        .map(|claim| (claim.id.clone(), claim))
        .collect::<HashMap<_, _>>();
    decisions
        .into_iter()
        .filter(|decision| {
            decision.supporting_claim_ids.iter().any(|claim_id| {
                claim_by_id
                    .get(claim_id)
                    .is_some_and(|claim| claim_in_region(region, note_ids, note_regions, claim))
            })
        })
        .take(5)
        .collect()
}

fn region_stale_dependencies(
    region: &str,
    note_ids: &HashSet<String>,
    all_claims: &[Claim],
    note_regions: &HashMap<String, String>,
    stale: Vec<StaleDep>,
) -> Vec<StaleDep> {
    let claim_by_id = all_claims
        .iter()
        .map(|claim| (claim.id.clone(), claim))
        .collect::<HashMap<_, _>>();
    stale
        .into_iter()
        .filter(|item| {
            item.depends_on_claim_ids
                .iter()
                .chain(item.superseded_claim_ids.iter())
                .any(|claim_id| {
                    claim_by_id
                        .get(claim_id)
                        .is_some_and(|claim| claim_in_region(region, note_ids, note_regions, claim))
                })
        })
        .take(5)
        .collect()
}

fn region_contradictions(
    region: &str,
    note_ids: &HashSet<String>,
    all_claims: &[Claim],
    note_regions: &HashMap<String, String>,
    contradictions: Vec<ChallengerContradiction>,
) -> Vec<ChallengerContradiction> {
    let claim_by_id = all_claims
        .iter()
        .map(|claim| (claim.id.clone(), claim))
        .collect::<HashMap<_, _>>();
    contradictions
        .into_iter()
        .filter(|item| {
            item.left_claim_ids
                .iter()
                .chain(item.right_claim_ids.iter())
                .any(|claim_id| {
                    claim_by_id
                        .get(claim_id)
                        .is_some_and(|claim| claim_in_region(region, note_ids, note_regions, claim))
                })
        })
        .take(5)
        .collect()
}

fn region_open_questions(
    region: &str,
    note_ids: &HashSet<String>,
    all_claims: &[Claim],
    note_regions: &HashMap<String, String>,
    decisions: &[Decision],
    questions: Vec<OpenQuestion>,
) -> Vec<OpenQuestion> {
    let region_subjects = region_subjects(region, note_ids, all_claims, note_regions);
    let decided_pairs = decided_pairs(decisions);
    questions
        .into_iter()
        .filter(|item| region_subjects.contains(&item.subject))
        .filter(|item| {
            let item_subject = item.subject.trim().to_ascii_lowercase();
            let item_object = normalize_decision_like_object(&item.object);
            !decided_pairs.iter().any(|(subject, object)| {
                subject == &item_subject && decision_objects_equivalent(object, &item_object)
            })
        })
        .take(5)
        .collect()
}

fn region_subjects(
    region: &str,
    note_ids: &HashSet<String>,
    all_claims: &[Claim],
    note_regions: &HashMap<String, String>,
) -> HashSet<String> {
    all_claims
        .iter()
        .filter(|claim| claim_in_region(region, note_ids, note_regions, claim))
        .map(|claim| claim.subject.trim().to_ascii_lowercase())
        .collect()
}

fn decided_pairs(decisions: &[Decision]) -> HashSet<(String, String)> {
    decisions
        .iter()
        .map(|decision| {
            (
                decision.subject.trim().to_ascii_lowercase(),
                normalize_decision_like_object(&decision.object),
            )
        })
        .collect()
}

fn normalize_decision_like_object(value: &str) -> String {
    let mut normalized = value.trim().to_ascii_lowercase().replace([' ', '_'], "-");
    while normalized.contains("--") {
        normalized = normalized.replace("--", "-");
    }
    normalized = normalized.trim_matches('-').to_string();
    for suffix in ["-templates", "-generator", "-default", "-v2"] {
        if normalized.ends_with(suffix) {
            normalized = normalized.trim_end_matches(suffix).to_string();
        }
    }
    normalized
}

fn decision_objects_equivalent(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    if left.len() >= 4 && right.len() >= 4 && (left.contains(right) || right.contains(left)) {
        return true;
    }
    false
}

fn claim_in_region(
    region: &str,
    note_ids: &HashSet<String>,
    note_regions: &HashMap<String, String>,
    claim: &Claim,
) -> bool {
    note_ids.contains(&claim.note_id)
        || note_regions
            .get(&claim.note_id)
            .is_some_and(|claim_region| claim_region == region)
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
            id: Claim::compute_id(subject, "relates_to", Some(&object), note_id, idx),
            subject: subject.to_string(),
            predicate: "relates_to".to_string(),
            object: Some(object),
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

    fn make_claim_for_rendering(
        claim_id: &str,
        note_id: &str,
        subject: &str,
        predicate: &str,
        object: Option<&str>,
    ) -> Claim {
        let valid_from = Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid datetime");
        Claim {
            id: claim_id.to_string(),
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: object.map(ToString::to_string),
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

    #[test]
    fn open_questions_excludes_decided_pairs_after_normalization() {
        let claims = vec![make_claim_for_rendering(
            "seed",
            "akmon-01",
            "akmon",
            "uses_client_generator",
            Some("stainless-templates"),
        )];
        let note_ids = HashSet::from(["akmon-01".to_string()]);
        let note_regions = HashMap::from([(
            "akmon-01".to_string(),
            "semantic/projects/akmon".to_string(),
        )]);
        let decisions = vec![Decision {
            subject: "akmon".to_string(),
            object: "stainless templates".to_string(),
            source_note_ids: vec!["akmon-01".to_string()],
            supporting_claim_ids: vec!["c1".to_string(), "c2".to_string()],
        }];
        let questions = vec![
            OpenQuestion {
                subject: "akmon".to_string(),
                family: "decision_candidate".to_string(),
                object: "stainless-default".to_string(),
                source_note_ids: vec!["ep-daily-2026-04-18".to_string()],
                supporting_claim_ids: vec!["q1".to_string()],
            },
            OpenQuestion {
                subject: "akmon".to_string(),
                family: "decision_candidate".to_string(),
                object: "openapi-generator".to_string(),
                source_note_ids: vec!["ep-daily-2026-04-12".to_string()],
                supporting_claim_ids: vec!["q2".to_string()],
            },
        ];

        let filtered = region_open_questions(
            "semantic/projects/akmon",
            &note_ids,
            &claims,
            &note_regions,
            &decisions,
            questions,
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].object, "openapi-generator");
    }

    #[test]
    fn atlas_deduplicates_verbatim_claims_and_truncates_long_source_lists() {
        let mut claims_by_subject = BTreeMap::<String, Vec<Claim>>::new();
        let mut claims = Vec::new();

        for i in 1..=5 {
            claims.push(make_claim_for_rendering(
                &format!("dup-id-{i:02}"),
                &format!("akmon-{i:02}"),
                "akmon",
                "ingests",
                Some("partner-events"),
            ));
        }

        claims.push(make_claim_for_rendering(
            "singleton-id",
            "akmon-06",
            "akmon",
            "generated_with",
            Some("stainless-templates"),
        ));

        for i in 1..=13 {
            claims.push(make_claim_for_rendering(
                &format!("unary-id-{i:02}"),
                &format!("akmon-u-{i:02}"),
                "akmon",
                "has_field_observation_notes",
                None,
            ));
        }

        claims_by_subject.insert("akmon".to_string(), claims);

        let rendered = build_atlas_markdown(
            "semantic/projects/akmon",
            "overview",
            &["akmon".to_string()],
            &claims_by_subject,
            AtlasFindings {
                decisions: &[],
                stale: &[],
                contradictions: &[],
                open_questions: &[],
            },
        );

        assert!(
            rendered.contains(
                "- [claim:unary-id-01] has_field_observation_notes (unary) (13 sources: [[akmon-u-01]], [[akmon-u-02]], [[akmon-u-03]], [[akmon-u-04]], [[akmon-u-05]], [[akmon-u-06]], [[akmon-u-07]], [[akmon-u-08]], [[akmon-u-09]], [[akmon-u-10]], ... (3 more))"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "- [claim:dup-id-01] ingests partner-events (5 sources: [[akmon-01]], [[akmon-02]], [[akmon-03]], [[akmon-04]], [[akmon-05]])"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "- [claim:singleton-id] generated_with stainless-templates (source note: [[akmon-06]])"
            ),
            "{rendered}"
        );
        assert_eq!(rendered.matches("ingests partner-events").count(), 1);
    }
}
