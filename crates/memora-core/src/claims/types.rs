use std::fmt::{Display, Formatter};
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::note::Privacy;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub id: String,
    pub subject: String,
    pub predicate: String,
    /// `None` for unary predicates (no object / "in early stages" style facts).
    pub object: Option<String>,
    pub note_id: String,
    pub span_start: usize,
    pub span_end: usize,
    pub span_fingerprint: String,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
    pub confidence: f32,
    pub privacy: Privacy,
    pub extracted_by: String,
    pub extracted_at: DateTime<Utc>,
}

impl Claim {
    pub fn compute_id(
        s: &str,
        p: &str,
        o: Option<&str>,
        note_id: &str,
        span_start: usize,
    ) -> String {
        let o = o.unwrap_or("");
        let payload = format!("{s}|{p}|{o}|{note_id}|{span_start}");
        let hash = blake3::hash(payload.as_bytes());
        hash.to_hex().to_string().chars().take(16).collect()
    }

    /// Full-width blake3 (256-bit) fingerprint of a source span, hex-encoded.
    ///
    /// This is the cryptographic basis of citation verification, so it uses the
    /// full digest. Earlier versions truncated it to 64 bits (16 hex chars);
    /// [`Claim::fingerprint_matches`] still accepts those legacy fingerprints so
    /// existing vaults keep verifying until they are re-indexed (which upgrades
    /// them to full width). Note this is distinct from [`Claim::compute_id`],
    /// whose 16-char output is a collision-tolerant *identifier*, not an
    /// integrity hash.
    pub fn compute_fingerprint(span_text: &str) -> String {
        blake3::hash(span_text.as_bytes()).to_hex().to_string()
    }

    /// Does `span_text` match a `stored` fingerprint? Tolerant of the legacy
    /// 64-bit (16 hex char) format so claims written by older versions still
    /// verify; full-width (256-bit) fingerprints are compared in full.
    pub fn fingerprint_matches(span_text: &str, stored: &str) -> bool {
        let full = Self::compute_fingerprint(span_text);
        match stored.len() {
            len if len == full.len() => full == stored,
            16 => full
                .get(..16)
                .map(|prefix| prefix == stored)
                .unwrap_or(false),
            _ => false,
        }
    }

    /// Human-readable object for prompts and logs; unary claims show a placeholder.
    pub fn object_display(&self) -> &str {
        self.object.as_deref().unwrap_or("(unary)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimRelation {
    Entails,
    Contradicts,
    Supersedes,
    Derives,
    CoOccurs,
}

impl Display for ClaimRelation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Entails => "entails",
            Self::Contradicts => "contradicts",
            Self::Supersedes => "supersedes",
            Self::Derives => "derives",
            Self::CoOccurs => "co_occurs",
        };
        f.write_str(value)
    }
}

impl FromStr for ClaimRelation {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "entails" => Ok(Self::Entails),
            "contradicts" => Ok(Self::Contradicts),
            "supersedes" => Ok(Self::Supersedes),
            "derives" => Ok(Self::Derives),
            "co_occurs" => Ok(Self::CoOccurs),
            _ => Err(format!("invalid claim relation: {s}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn claim_id_is_stable_and_truncated() {
        let id = Claim::compute_id("rado", "works_at", Some("hmc"), "note-1", 12);
        assert_eq!(id.len(), 16);
        assert_eq!(
            id,
            Claim::compute_id("rado", "works_at", Some("hmc"), "note-1", 12)
        );
    }

    #[test]
    fn fingerprint_is_full_width_blake3() {
        let fp = Claim::compute_fingerprint("Rado works at HMC");
        // 256-bit blake3 = 64 hex chars (not the legacy 16).
        assert_eq!(fp.len(), 64);
        assert!(Claim::fingerprint_matches("Rado works at HMC", &fp));
        assert!(!Claim::fingerprint_matches("Rado works at Google", &fp));
    }

    #[test]
    fn fingerprint_matches_accepts_legacy_truncated_fingerprints() {
        // A pre-existing claim stored only the first 16 hex chars; it must still
        // verify against the same span so old vaults keep working.
        let span = "Rado works at HMC";
        let legacy: String = Claim::compute_fingerprint(span).chars().take(16).collect();
        assert_eq!(legacy.len(), 16);
        assert!(Claim::fingerprint_matches(span, &legacy));
        assert!(!Claim::fingerprint_matches("tampered text", &legacy));
    }

    #[test]
    fn relation_round_trip() {
        let relation = ClaimRelation::from_str("co_occurs").expect("parse relation");
        assert_eq!(relation.to_string(), "co_occurs");
    }
}
