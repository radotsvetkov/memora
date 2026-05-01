use memora_llm::{LlmDestination, LlmProvider};

use crate::claims::Claim;
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
}

impl PrivacyFilter {
    pub fn new_for(provider: LlmProvider) -> Self {
        let destination = match provider {
            LlmProvider::Anthropic | LlmProvider::OpenAi => LlmDestination::CloudKnown,
            LlmProvider::Ollama => LlmDestination::Local,
        };
        Self { destination }
    }

    pub fn filter(&self, claims: &[Claim]) -> (Vec<RedactedClaim>, RedactionStats) {
        let mut stats = RedactionStats::default();
        let mut out = Vec::with_capacity(claims.len());

        for claim in claims {
            if self.destination == LlmDestination::Local || claim.privacy != Privacy::Secret {
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
                out.push(RedactedClaim {
                    id: claim.id.clone(),
                    subject: claim.subject.clone(),
                    predicate: "[redacted]".to_string(),
                    object: "[redacted]".to_string(),
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
        assert_eq!(claims[0].subject, "Comp");
        assert_eq!(claims[0].predicate, "[redacted]");
        assert_eq!(claims[0].object, "[redacted]");
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
