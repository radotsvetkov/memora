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
    pub object: String,
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
    pub fn compute_id(s: &str, p: &str, o: &str, note_id: &str, span_start: usize) -> String {
        let payload = format!("{s}|{p}|{o}|{note_id}|{span_start}");
        let hash = blake3::hash(payload.as_bytes());
        hash.to_hex().to_string().chars().take(16).collect()
    }

    pub fn compute_fingerprint(span_text: &str) -> String {
        let hash = blake3::hash(span_text.as_bytes());
        hash.to_hex().to_string().chars().take(16).collect()
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
        let id = Claim::compute_id("rado", "works_at", "hmc", "note-1", 12);
        assert_eq!(id.len(), 16);
        assert_eq!(
            id,
            Claim::compute_id("rado", "works_at", "hmc", "note-1", 12)
        );
    }

    #[test]
    fn relation_round_trip() {
        let relation = ClaimRelation::from_str("co_occurs").expect("parse relation");
        assert_eq!(relation.to_string(), "co_occurs");
    }
}
