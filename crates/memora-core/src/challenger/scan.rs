use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use memora_llm::{CompletionRequest, LlmClient, Message, Role};
use rusqlite::params;
use serde::Deserialize;

use crate::challenger::report::{
    ChallengerReport, ContradictionAlert, CrossRegionAlert, FrontierAlert, StaleAlert,
};
use crate::claims::{Claim, ClaimStore};
use crate::index::Index;

pub struct Challenger<'a> {
    pub db: &'a Index,
    pub claim_store: &'a ClaimStore<'a>,
    pub llm: &'a dyn LlmClient,
    pub vault: &'a Path,
    pub config: ChallengerConfig,
}

#[derive(Debug, Clone)]
pub struct ChallengerConfig {
    pub stale_limit: usize,
    pub gap_limit: usize,
    pub cross_region_min: usize,
    pub low_confidence_threshold: f32,
}

impl Default for ChallengerConfig {
    fn default() -> Self {
        Self {
            stale_limit: 50,
            gap_limit: 20,
            cross_region_min: 3,
            low_confidence_threshold: 0.5,
        }
    }
}

#[derive(Debug, Deserialize)]
struct StaleProposal {
    action: String,
    #[serde(default)]
    new_claim: Option<StaleProposalClaim>,
}

#[derive(Debug, Deserialize)]
struct StaleProposalClaim {
    s: String,
    p: String,
    o: String,
}

#[derive(Debug)]
struct CrossRegionSubject {
    subject: String,
    regions: Vec<String>,
    source_note_ids: Vec<String>,
}

#[derive(Debug)]
struct FrontierCandidate {
    claim_id: String,
    source_note_id: String,
    subject: String,
    predicate: String,
    object: String,
    confidence: f32,
    predicate_occurrences: usize,
}

impl<'a> Challenger<'a> {
    pub async fn run_once(&self) -> Result<ChallengerReport> {
        let generated_at = Utc::now();
        let stale_alerts = self.stale_review(generated_at).await?;
        let contradiction_alerts = self.contradictions(generated_at).await?;
        let cross_region_alerts = self.cross_region_patterns(generated_at)?;
        let frontier_alerts = self.frontier_gaps(generated_at).await?;

        Ok(ChallengerReport {
            generated_at,
            stale_alerts,
            contradiction_alerts,
            cross_region_alerts,
            frontier_alerts,
        })
    }

    pub fn persist_report(&self, report: &ChallengerReport) -> Result<()> {
        self.write_world_map_review(report)?;
        self.save_report_json(report)?;
        Ok(())
    }

    async fn stale_review(&self, generated_at: chrono::DateTime<Utc>) -> Result<Vec<StaleAlert>> {
        let stale_ids = self.stale_claim_ids(self.config.stale_limit)?;
        let mut alerts = Vec::new();
        for claim_id in stale_ids {
            let Some(claim) = self.claim_store.get(&claim_id)? else {
                continue;
            };
            let span_text = self.read_span_text(&claim).await.unwrap_or_default();
            let prompt = format!(
                "Given the source note has changed, propose:\n\
                 (1) the updated claim, OR (2) 'archive' if no longer applicable.\n\
                 Source: '{span_text}'. Old claim: '{} {} {}'.\n\
                 Output JSON {{\"action\":\"update\"|\"archive\",\"new_claim\"?:{{\"s\":\"...\",\"p\":\"...\",\"o\":\"...\"}}}}",
                claim.subject, claim.predicate, claim.object
            );
            let response = self
                .llm
                .complete(CompletionRequest {
                    model: None,
                    system: None,
                    messages: vec![Message {
                        role: Role::User,
                        content: prompt,
                    }],
                    max_tokens: 160,
                    temperature: 0.0,
                    json_mode: true,
                })
                .await?;
            let proposal =
                serde_json::from_str::<StaleProposal>(&response.text).unwrap_or(StaleProposal {
                    action: "archive".to_string(),
                    new_claim: None,
                });
            let normalized_action = proposal.action.trim().to_ascii_lowercase();
            let description = if normalized_action == "update" {
                if let Some(new_claim) = &proposal.new_claim {
                    format!(
                        "Stale claim needs update to '{} {} {}'.",
                        new_claim.s, new_claim.p, new_claim.o
                    )
                } else {
                    "Stale claim marked for update.".to_string()
                }
            } else {
                "Stale claim appears obsolete and should be archived.".to_string()
            };
            let (proposal_subject, proposal_predicate, proposal_object) = proposal
                .new_claim
                .map(|c| (Some(c.s), Some(c.p), Some(c.o)))
                .unwrap_or((None, None, None));
            alerts.push(StaleAlert {
                claim_id: claim.id,
                source_note_id: claim.note_id,
                description,
                proposal_action: normalized_action,
                proposal_subject,
                proposal_predicate,
                proposal_object,
                generated_at,
            });
        }
        Ok(alerts)
    }

    async fn contradictions(
        &self,
        generated_at: chrono::DateTime<Utc>,
    ) -> Result<Vec<ContradictionAlert>> {
        let mut alerts = Vec::new();
        for (left, right) in self.claim_store.contradictions_unack()? {
            let summary_prompt = format!(
                "Summarize this contradiction in one sentence.\n\
                 Claim A: '{} {} {}'\n\
                 Claim B: '{} {} {}'",
                left.subject,
                left.predicate,
                left.object,
                right.subject,
                right.predicate,
                right.object
            );
            let summary = self
                .llm
                .complete(CompletionRequest {
                    model: None,
                    system: None,
                    messages: vec![Message {
                        role: Role::User,
                        content: summary_prompt,
                    }],
                    max_tokens: 80,
                    temperature: 0.0,
                    json_mode: false,
                })
                .await?
                .text
                .trim()
                .replace('\n', " ");
            alerts.push(ContradictionAlert {
                left_claim_id: left.id,
                right_claim_id: right.id,
                left_source_note_id: left.note_id,
                right_source_note_id: right.note_id,
                description: summary,
                generated_at,
            });
        }
        Ok(alerts)
    }

    fn cross_region_patterns(
        &self,
        generated_at: chrono::DateTime<Utc>,
    ) -> Result<Vec<CrossRegionAlert>> {
        let subjects = self.cross_region_subjects()?;
        let mut alerts = Vec::new();
        for entry in subjects {
            let suggested_home_note_id = slugify(&entry.subject);
            if self
                .db
                .note_id_for_target(&suggested_home_note_id)?
                .is_some()
            {
                continue;
            }
            let description = format!(
                "Subject '{}' spans {} regions without a home note; consider creating '{}'.",
                entry.subject,
                entry.regions.len(),
                suggested_home_note_id
            );
            alerts.push(CrossRegionAlert {
                subject: entry.subject,
                regions: entry.regions,
                source_note_ids: entry.source_note_ids,
                description,
                suggested_home_note_id,
                generated_at,
            });
        }
        Ok(alerts)
    }

    async fn frontier_gaps(
        &self,
        generated_at: chrono::DateTime<Utc>,
    ) -> Result<Vec<FrontierAlert>> {
        let candidates = self.frontier_candidates()?;
        let mut alerts = Vec::new();
        for candidate in candidates {
            let question_prompt = format!(
                "Generate one short clarifying question for this uncertain claim:\n\
                 '{} {} {}' (confidence {}, predicate occurrences {}).",
                candidate.subject,
                candidate.predicate,
                candidate.object,
                candidate.confidence,
                candidate.predicate_occurrences
            );
            let question = self
                .llm
                .complete(CompletionRequest {
                    model: None,
                    system: None,
                    messages: vec![Message {
                        role: Role::User,
                        content: question_prompt,
                    }],
                    max_tokens: 48,
                    temperature: 0.1,
                    json_mode: false,
                })
                .await?
                .text
                .trim()
                .replace('\n', " ");
            let description = if candidate.confidence < self.config.low_confidence_threshold {
                "Low-confidence claim needs clarification.".to_string()
            } else {
                "Rare predicate appears once and needs corroboration.".to_string()
            };
            alerts.push(FrontierAlert {
                claim_id: candidate.claim_id,
                source_note_id: candidate.source_note_id,
                description,
                confidence: candidate.confidence,
                predicate_occurrences: candidate.predicate_occurrences,
                clarifying_question: question,
                generated_at,
            });
        }
        Ok(alerts)
    }

    fn stale_claim_ids(&self, limit: usize) -> Result<Vec<String>> {
        let limit = i64::try_from(limit)?;
        let conn = self.db.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT claim_id
             FROM stale_claims
             ORDER BY marked_at DESC, claim_id ASC
             LIMIT ?",
        )?;
        let rows = stmt.query_map(params![limit], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn cross_region_subjects(&self) -> Result<Vec<CrossRegionSubject>> {
        let conn = self.db.pool.get()?;
        let min_regions = i64::try_from(self.config.cross_region_min)?;
        let mut stmt = conn.prepare(
            "SELECT c.subject
             FROM claims c
             JOIN notes n ON n.id = c.note_id
             WHERE c.valid_until IS NULL
             GROUP BY c.subject
             HAVING COUNT(DISTINCT n.region) >= ?
             ORDER BY COUNT(DISTINCT n.region) DESC, c.subject ASC
             LIMIT 20",
        )?;
        let subjects = stmt.query_map(params![min_regions], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for subject_row in subjects {
            let subject = subject_row?;
            let mut details_stmt = conn.prepare(
                "SELECT DISTINCT n.region, c.note_id
                 FROM claims c
                 JOIN notes n ON n.id = c.note_id
                 WHERE c.subject = ?
                   AND c.valid_until IS NULL
                 ORDER BY n.region ASC, c.note_id ASC",
            )?;
            let detail_rows = details_stmt.query_map(params![subject.as_str()], |row| {
                let region: String = row.get(0)?;
                let note_id: String = row.get(1)?;
                Ok((region, note_id))
            })?;
            let mut regions = BTreeSet::new();
            let mut source_note_ids = BTreeSet::new();
            for row in detail_rows {
                let (region, note_id) = row?;
                regions.insert(region);
                source_note_ids.insert(note_id);
            }
            out.push(CrossRegionSubject {
                subject,
                regions: regions.into_iter().collect(),
                source_note_ids: source_note_ids.into_iter().collect(),
            });
        }
        Ok(out)
    }

    fn frontier_candidates(&self) -> Result<Vec<FrontierCandidate>> {
        let conn = self.db.pool.get()?;
        let gap_limit = i64::try_from(self.config.gap_limit)?;
        let mut stmt = conn.prepare(
            "SELECT c.id, c.note_id, c.subject, c.predicate, c.object, c.confidence,
                    (
                        SELECT COUNT(*)
                        FROM claims p
                        WHERE p.predicate = c.predicate
                          AND p.valid_until IS NULL
                    ) AS predicate_count
             FROM claims c
             WHERE c.valid_until IS NULL
               AND (
                    c.confidence < ?
                    OR (
                        SELECT COUNT(*)
                        FROM claims p
                        WHERE p.predicate = c.predicate
                          AND p.valid_until IS NULL
                    ) = 1
               )
             ORDER BY c.extracted_at DESC
             LIMIT ?",
        )?;
        let rows = stmt.query_map(
            params![self.config.low_confidence_threshold, gap_limit],
            |row| {
                let predicate_count_raw: i64 = row.get(6)?;
                let predicate_occurrences = usize::try_from(predicate_count_raw).map_err(|_| {
                    rusqlite::Error::IntegralValueOutOfRange(6, predicate_count_raw)
                })?;
                Ok(FrontierCandidate {
                    claim_id: row.get(0)?,
                    source_note_id: row.get(1)?,
                    subject: row.get(2)?,
                    predicate: row.get(3)?,
                    object: row.get(4)?,
                    confidence: row.get(5)?,
                    predicate_occurrences,
                })
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    async fn read_span_text(&self, claim: &Claim) -> Result<String> {
        let Some(note) = self.db.get_note(&claim.note_id)? else {
            return Ok(String::new());
        };
        let path = self.vault.join(&note.path);
        let body = tokio::task::spawn_blocking(move || crate::note::parse(&path))
            .await??
            .body;
        let Some(span_text) = body.get(claim.span_start..claim.span_end) else {
            return Ok(String::new());
        };
        Ok(span_text.to_string())
    }

    fn write_world_map_review(&self, report: &ChallengerReport) -> Result<()> {
        let world_map_path = self.vault.join("world_map.md");
        let mut content = if world_map_path.exists() {
            fs::read_to_string(&world_map_path)?
        } else {
            "# World Map\n".to_string()
        };
        if !content.ends_with('\n') {
            content.push('\n');
        }

        let section = render_review_section(report);
        let replaced = replace_review_section(&content, &section);
        fs::write(world_map_path, replaced)?;
        Ok(())
    }

    fn save_report_json(&self, report: &ChallengerReport) -> Result<()> {
        let memora_dir = self.vault.join(".memora");
        fs::create_dir_all(&memora_dir)?;
        let json = serde_json::to_string_pretty(report)?;
        fs::write(memora_dir.join("last_challenger.json"), &json)?;

        let timestamped = format!(
            "challenger-{}.json",
            report.generated_at.format("%Y%m%dT%H%M%SZ")
        );
        fs::write(memora_dir.join(timestamped), json)?;
        Ok(())
    }
}

fn render_review_section(report: &ChallengerReport) -> String {
    let mut section = String::new();
    section.push_str(&format!(
        "## Today's review (auto-{})\n",
        report.generated_at.format("%Y-%m-%d")
    ));
    section.push_str(&format!(
        "_generated at {}_\n\n",
        report.generated_at.to_rfc3339()
    ));

    section.push_str("### Stale claims\n");
    if report.stale_alerts.is_empty() {
        section.push_str("- none\n");
    } else {
        for alert in &report.stale_alerts {
            section.push_str(&format!(
                "- [{}] {} ({})\n",
                alert.claim_id, alert.description, alert.source_note_id
            ));
        }
    }

    section.push_str("\n### Contradictions\n");
    if report.contradiction_alerts.is_empty() {
        section.push_str("- none\n");
    } else {
        for alert in &report.contradiction_alerts {
            section.push_str(&format!(
                "- [{} vs {}] {}\n",
                alert.left_claim_id, alert.right_claim_id, alert.description
            ));
        }
    }

    section.push_str("\n### Cross-region patterns\n");
    if report.cross_region_alerts.is_empty() {
        section.push_str("- none\n");
    } else {
        for alert in &report.cross_region_alerts {
            section.push_str(&format!(
                "- [{}] {} (regions: {})\n",
                alert.subject,
                alert.description,
                alert.regions.join(", ")
            ));
        }
    }

    section.push_str("\n### Frontier gaps\n");
    if report.frontier_alerts.is_empty() {
        section.push_str("- none\n");
    } else {
        for alert in &report.frontier_alerts {
            section.push_str(&format!(
                "- [{}] {} Question: {}\n",
                alert.claim_id, alert.description, alert.clarifying_question
            ));
        }
    }
    section.push('\n');
    section
}

fn replace_review_section(content: &str, replacement: &str) -> String {
    let mut lines = content.lines().collect::<Vec<_>>();
    let start = lines
        .iter()
        .position(|line| line.starts_with("## Today's review (auto-"));
    if let Some(start_idx) = start {
        let next_heading = lines
            .iter()
            .enumerate()
            .skip(start_idx + 1)
            .find_map(|(idx, line)| line.starts_with("## ").then_some(idx))
            .unwrap_or(lines.len());
        lines.splice(start_idx..next_heading, replacement.lines());
        let mut out = lines.join("\n");
        out.push('\n');
        return out;
    }

    let mut out = content.to_string();
    if !out.ends_with("\n\n") {
        if out.ends_with('\n') {
            out.push('\n');
        } else {
            out.push_str("\n\n");
        }
    }
    out.push_str(replacement);
    out
}

fn slugify(subject: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in subject.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}
