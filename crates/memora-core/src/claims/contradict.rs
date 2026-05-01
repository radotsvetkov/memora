use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Result;
use memora_llm::LlmClient;
use serde_json::Value;

use crate::claims::{Claim, ClaimRelation, ClaimStore, StalenessTracker};

static PREDICATE_EQUIVALENCE_CACHE: OnceLock<Mutex<HashMap<(String, String), bool>>> =
    OnceLock::new();

pub struct ContradictionDetector<'a> {
    pub store: &'a ClaimStore<'a>,
    pub stale: &'a StalenessTracker<'a>,
    pub llm: Arc<dyn LlmClient>,
}

impl<'a> ContradictionDetector<'a> {
    pub async fn check_new_claim(&self, claim: &Claim) -> Result<Vec<String>> {
        let mut candidates = self
            .store
            .find_by_subject_predicate(&claim.subject, &claim.predicate)?;

        for candidate in self.store.find_by_subject(&claim.subject)? {
            if candidate.predicate == claim.predicate {
                continue;
            }
            if self
                .predicates_equivalent(&claim.subject, &claim.predicate, &candidate.predicate)
                .await?
            {
                candidates.push(candidate);
            }
        }

        let mut seen = HashSet::new();
        let mut superseded_ids = Vec::new();
        for candidate in candidates {
            if candidate.id == claim.id || candidate.object == claim.object {
                continue;
            }
            if !seen.insert(candidate.id.clone()) {
                continue;
            }

            if self.claims_contradict(claim, &candidate).await? {
                let (newer, mut older) = if claim.valid_from >= candidate.valid_from {
                    (claim, candidate)
                } else {
                    (&candidate, claim.clone())
                };

                older.valid_until = Some(newer.valid_from);
                self.store.upsert(&older)?;
                self.store
                    .add_relation(&newer.id, &older.id, ClaimRelation::Supersedes, 1.0)?;
                self.store
                    .add_relation(&newer.id, &older.id, ClaimRelation::Contradicts, 1.0)?;
                self.stale.on_claim_superseded(&older.id)?;

                if !superseded_ids.iter().any(|id| id == &older.id) {
                    superseded_ids.push(older.id);
                }
            }
        }

        Ok(superseded_ids)
    }

    async fn predicates_equivalent(&self, subject: &str, a: &str, b: &str) -> Result<bool> {
        if a == b {
            return Ok(true);
        }
        let key = canonical_predicate_key(a, b);
        let cache = PREDICATE_EQUIVALENCE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(cached) = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&key)
            .copied()
        {
            return Ok(cached);
        }

        let prompt = format!(
            r#"Subject: {subject}
Predicate A: {a}
Predicate B: {b}

Answer with JSON only. Use exactly {{"equivalent":true}} or {{"equivalent":false}}."#
        );
        let text = self
            .llm
            .chat_json(
                &prompt,
                Some("Output must be one JSON object with boolean field \"equivalent\"."),
                128,
                0.0,
            )
            .await?;
        let equivalent =
            parse_bool_field(&text, "equivalent").unwrap_or_else(|| starts_with_yes(&text));
        cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key, equivalent);
        Ok(equivalent)
    }

    async fn claims_contradict(&self, a: &Claim, b: &Claim) -> Result<bool> {
        let prompt = format!(
            r#"Claim A: subject="{}" predicate="{}" object="{}"
Claim B: subject="{}" predicate="{}" object="{}"

Answer with JSON only. Use exactly {{"contradicts":true}} or {{"contradicts":false}}."#,
            a.subject, a.predicate, a.object, b.subject, b.predicate, b.object
        );
        let text = self
            .llm
            .chat_json(
                &prompt,
                Some("Output must be one JSON object with boolean field \"contradicts\"."),
                160,
                0.0,
            )
            .await?;
        Ok(parse_bool_field(&text, "contradicts").unwrap_or_else(|| starts_with_yes(&text)))
    }
}

fn parse_bool_field(text: &str, key: &str) -> Option<bool> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    value.get(key)?.as_bool()
}

fn starts_with_yes(text: &str) -> bool {
    text.trim().to_ascii_lowercase().starts_with("yes")
}

fn canonical_predicate_key(a: &str, b: &str) -> (String, String) {
    let a = a.trim().to_ascii_lowercase();
    let b = b.trim().to_ascii_lowercase();
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}
