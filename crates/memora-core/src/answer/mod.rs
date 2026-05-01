use std::collections::{HashMap, HashSet};

use anyhow::Result;
use memora_llm::{CompletionRequest, LlmClient, Message, Role};

use crate::cite::{CitationStatus, CitationValidator, CitedAnswer};
use crate::claims::{Claim, ClaimStore};
use crate::config::PrivacyConfig;
use crate::privacy::{PrivacyFilter, RedactedClaim};
use crate::retrieve::HybridRetriever;

const ANSWER_SYSTEM_PROMPT: &str = "You answer using ONLY the verified claims. Every factual sentence must end with [claim:ID] for the supporting claim. If no claim supports a statement, omit it. Quote source text where relevant.";

/// ```compile_fail
/// use memora_core::answer::AnsweringPipeline;
/// use memora_core::{ClaimStore, CitationValidator, HybridRetriever};
/// use memora_llm::LlmClient;
///
/// fn build<'a>(
///     retriever: &'a HybridRetriever<'a>,
///     claim_store: &'a ClaimStore<'a>,
///     validator: &'a CitationValidator<'a>,
///     llm: &'a dyn LlmClient,
/// ) {
///     let _pipeline = AnsweringPipeline {
///         retriever,
///         claim_store,
///         validator,
///         llm,
///     };
/// }
/// ```
pub struct AnsweringPipeline<'a> {
    pub retriever: &'a HybridRetriever<'a>,
    pub claim_store: &'a ClaimStore<'a>,
    pub validator: &'a CitationValidator<'a>,
    pub llm: &'a dyn LlmClient,
    pub privacy_filter: PrivacyFilter,
    pub privacy_config: PrivacyConfig,
}

impl<'a> AnsweringPipeline<'a> {
    pub async fn answer(&self, query: &str, k: usize) -> Result<CitedAnswer> {
        let hits = self.retriever.search(query, k).await?;
        let top_hits = hits.into_iter().take(8).collect::<Vec<_>>();

        let mut candidate_claims = Vec::new();
        for hit in &top_hits {
            candidate_claims.extend(self.claim_store.list_for_note(&hit.id)?);
        }

        let candidate_ids = candidate_claims
            .iter()
            .map(|claim| claim.id.clone())
            .collect::<Vec<_>>();
        let current_claims = self.claim_store.current_only(&candidate_ids)?;
        let note_score = top_hits
            .iter()
            .map(|hit| (hit.id.clone(), hit.score))
            .collect::<HashMap<_, _>>();
        let ranked_claims = rank_claims_by_note_score(current_claims, &note_score, 12);
        if ranked_claims.is_empty() {
            return Ok(CitedAnswer {
                raw_text: "No indexed claims matched this question yet. Try running `memora index --vault <path>` and ensure your notes include valid frontmatter."
                    .to_string(),
                clean_text: "No indexed claims matched this question yet. Try running `memora index --vault <path>` and ensure your notes include valid frontmatter."
                    .to_string(),
                checks: Vec::new(),
                verified_count: 0,
                unverified_count: 0,
                mismatch_count: 0,
                redacted_count: 0,
                degraded: true,
            });
        }
        let (redacted, stats) = self.privacy_filter.filter(&ranked_claims);
        if self.privacy_config.warn_on_secret_query && stats.redacted > 0 {
            tracing::warn!(
                redacted = stats.redacted,
                "query touched secret claims; cloud LLM received redacted versions"
            );
        }

        let prompt_context = format_claim_context(&redacted);
        let response = self
            .llm
            .complete(CompletionRequest {
                model: None,
                system: Some(ANSWER_SYSTEM_PROMPT.to_string()),
                messages: vec![Message {
                    role: Role::User,
                    content: format!("{prompt_context}\n\nQuestion: {query}"),
                }],
                max_tokens: 1_200,
                temperature: 0.1,
                json_mode: false,
            })
            .await?;
        let mut cited = self.validator.validate(&response.text).await?;
        cited.redacted_count = stats.redacted;
        if cited.verified_count > 0 && cited.unverified_count + cited.mismatch_count == 0 {
            return Ok(cited);
        }

        let verified_set = cited
            .checks
            .iter()
            .filter(|check| check.status == CitationStatus::Verified)
            .map(|check| check.claim_id.clone())
            .collect::<HashSet<_>>();
        let mut allowed_ids = verified_set.into_iter().collect::<Vec<_>>();
        allowed_ids.sort();

        let retry_system = format!(
            "{ANSWER_SYSTEM_PROMPT} Use ONLY these claim ids: {}. Do not emit any other [claim:...] marker.",
            allowed_ids.join(", ")
        );
        let retry = self
            .llm
            .complete(CompletionRequest {
                model: None,
                system: Some(retry_system),
                messages: vec![Message {
                    role: Role::User,
                    content: format!("{prompt_context}\n\nQuestion: {query}"),
                }],
                max_tokens: 1_200,
                temperature: 0.0,
                json_mode: false,
            })
            .await?;
        let mut cited_retry = self.validator.validate(&retry.text).await?;
        cited_retry.redacted_count = stats.redacted;
        if cited_retry.verified_count == 0 {
            let fallback = build_extractive_answer(&redacted, 5);
            let mut fallback_cited = self.validator.validate(&fallback).await?;
            fallback_cited.redacted_count = stats.redacted;
            fallback_cited.degraded = true;
            return Ok(fallback_cited);
        }
        if cited_retry.unverified_count + cited_retry.mismatch_count > 0 {
            cited_retry.degraded = true;
        }
        Ok(cited_retry)
    }
}

fn rank_claims_by_note_score(
    mut claims: Vec<Claim>,
    note_score: &HashMap<String, f32>,
    limit: usize,
) -> Vec<Claim> {
    claims.sort_by(|a, b| {
        let score_a = note_score.get(&a.note_id).copied().unwrap_or_default();
        let score_b = note_score.get(&b.note_id).copied().unwrap_or_default();
        score_b
            .partial_cmp(&score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    claims.truncate(limit);
    claims
}

fn format_claim_context(claims: &[RedactedClaim]) -> String {
    let mut out = String::from("Verified claims (cite these with [claim:ID] markers):\n");
    for claim in claims {
        let object = if claim.object.is_empty() {
            "(unary)"
        } else {
            claim.object.as_str()
        };
        out.push_str(&format!(
            "- [claim:{}] {} {} {}\n",
            claim.id, claim.subject, claim.predicate, object
        ));
    }
    out
}

fn build_extractive_answer(claims: &[RedactedClaim], limit: usize) -> String {
    let mut out = String::from("Based on indexed claims:\n");
    for claim in claims.iter().take(limit) {
        let object = if claim.object.is_empty() {
            "(unary)"
        } else {
            claim.object.as_str()
        };
        out.push_str(&format!(
            "- {} {} {} [claim:{}]\n",
            claim.subject, claim.predicate, object, claim.id
        ));
    }
    out
}
