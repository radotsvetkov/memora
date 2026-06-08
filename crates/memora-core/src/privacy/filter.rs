use memora_llm::{LlmDestination, LlmProvider, REDACTION_PLACEHOLDER};

use crate::claims::Claim;
use crate::config::PrivacyConfig;
use crate::note::Privacy;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedClaim {
    pub id: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub redacted: bool,
    pub privacy: Privacy,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RedactionStats {
    pub passed: usize,
    pub redacted: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivacyFilter {
    pub destination: LlmDestination,
    pub redact_secret_in_cloud: bool,
}

impl PrivacyFilter {
    pub fn new_for(provider: LlmProvider) -> Self {
        Self::new_for_provider(provider, &PrivacyConfig::default())
    }

    pub fn new_for_provider(provider: LlmProvider, config: &PrivacyConfig) -> Self {
        let destination = match provider {
            LlmProvider::Anthropic | LlmProvider::OpenAi => LlmDestination::CloudKnown,
            LlmProvider::Ollama => LlmDestination::Local,
        };
        Self {
            destination,
            redact_secret_in_cloud: config.redact_secret_in_cloud,
        }
    }

    /// Construct a filter directly from an LLM destination (e.g. the value
    /// returned by [`memora_llm::LlmClient::destination`]).
    pub fn from_destination(destination: LlmDestination, redact_secret_in_cloud: bool) -> Self {
        Self {
            destination,
            redact_secret_in_cloud,
        }
    }

    /// Whether a claim of this privacy level must be redacted before reaching
    /// the configured destination.
    pub fn should_redact(&self, privacy: Privacy) -> bool {
        self.redact_secret_in_cloud
            && self.destination != LlmDestination::Local
            && privacy == Privacy::Secret
    }

    /// Raw token strings (subject / predicate / object) from `claims` that must
    /// not appear in a cloud payload under this filter's policy. Returns an
    /// empty vec for local destinations or when redaction is disabled, so the
    /// result can be passed straight to [`memora_llm::LlmClient::redact`] as a
    /// defense-in-depth scrub list.
    pub fn secret_tokens(&self, claims: &[Claim]) -> Vec<String> {
        let mut out = Vec::new();
        for claim in claims {
            if self.should_redact(claim.privacy) {
                out.push(claim.subject.clone());
                out.push(claim.predicate.clone());
                if let Some(object) = &claim.object {
                    out.push(object.clone());
                }
            }
        }
        out
    }

    pub fn filter(&self, claims: &[Claim]) -> (Vec<RedactedClaim>, RedactionStats) {
        let mut stats = RedactionStats::default();
        let mut out = Vec::with_capacity(claims.len());

        for claim in claims {
            if !self.should_redact(claim.privacy) {
                out.push(RedactedClaim {
                    id: claim.id.clone(),
                    subject: claim.subject.clone(),
                    predicate: claim.predicate.clone(),
                    object: claim.object.clone().unwrap_or_default(),
                    redacted: false,
                    privacy: claim.privacy,
                });
                stats.passed += 1;
            } else {
                // Secret claim bound for the cloud: redact the SUBJECT too. The
                // subject can itself be sensitive (a person, company, or project
                // name), so leaving it verbatim leaks identifying content.
                out.push(RedactedClaim {
                    id: claim.id.clone(),
                    subject: REDACTION_PLACEHOLDER.to_string(),
                    predicate: REDACTION_PLACEHOLDER.to_string(),
                    object: REDACTION_PLACEHOLDER.to_string(),
                    redacted: true,
                    privacy: Privacy::Secret,
                });
                stats.redacted += 1;
            }
        }

        (out, stats)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use memora_llm::LlmProvider;

    use super::*;

    fn claim_with_privacy(privacy: Privacy) -> Claim {
        Claim {
            id: "aaaaaaaaaaaaaaaa".to_string(),
            subject: "Comp".to_string(),
            predicate: "has_salary".to_string(),
            object: Some("95000".to_string()),
            note_id: "note-1".to_string(),
            span_start: 0,
            span_end: 10,
            span_fingerprint: "bbbbbbbbbbbbbbbb".to_string(),
            valid_from: Utc
                .with_ymd_and_hms(2026, 4, 1, 0, 0, 0)
                .single()
                .expect("valid date"),
            valid_until: None,
            confidence: 1.0,
            privacy,
            extracted_by: "test".to_string(),
            extracted_at: Utc::now(),
        }
    }

    #[test]
    fn cloud_destination_redacts_secret_claims() {
        let filter = PrivacyFilter::new_for(LlmProvider::OpenAi);
        let (claims, stats) = filter.filter(&[claim_with_privacy(Privacy::Secret)]);
        assert_eq!(stats.redacted, 1);
        assert_eq!(stats.passed, 0);
        assert_eq!(claims.len(), 1);
        assert!(claims[0].redacted);
        // The subject must be redacted for Secret claims bound for the cloud:
        // an entity name is itself sensitive content.
        assert_eq!(claims[0].subject, "[redacted]");
        assert_eq!(claims[0].predicate, "[redacted]");
        assert_eq!(claims[0].object, "[redacted]");
    }

    #[test]
    fn secret_tokens_collects_raw_triples_for_cloud() {
        let filter = PrivacyFilter::new_for(LlmProvider::OpenAi);
        let tokens = filter.secret_tokens(&[claim_with_privacy(Privacy::Secret)]);
        assert_eq!(tokens, vec!["Comp", "has_salary", "95000"]);
    }

    #[test]
    fn secret_tokens_empty_for_local_destination() {
        let filter = PrivacyFilter::new_for(LlmProvider::Ollama);
        assert!(filter
            .secret_tokens(&[claim_with_privacy(Privacy::Secret)])
            .is_empty());
    }

    #[test]
    fn local_destination_keeps_secret_claims_unredacted() {
        let filter = PrivacyFilter::new_for(LlmProvider::Ollama);
        let (claims, stats) = filter.filter(&[claim_with_privacy(Privacy::Secret)]);
        assert_eq!(stats.redacted, 0);
        assert_eq!(stats.passed, 1);
        assert_eq!(claims.len(), 1);
        assert!(!claims[0].redacted);
        assert_eq!(claims[0].predicate, "has_salary");
        assert_eq!(claims[0].object, "95000");
    }
}
