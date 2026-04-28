use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StaleAlert {
    pub claim_id: String,
    pub source_note_id: String,
    pub description: String,
    pub proposal_action: String,
    pub proposal_subject: Option<String>,
    pub proposal_predicate: Option<String>,
    pub proposal_object: Option<String>,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContradictionAlert {
    pub left_claim_id: String,
    pub right_claim_id: String,
    pub left_source_note_id: String,
    pub right_source_note_id: String,
    pub description: String,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrossRegionAlert {
    pub subject: String,
    pub regions: Vec<String>,
    pub source_note_ids: Vec<String>,
    pub description: String,
    pub suggested_home_note_id: String,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FrontierAlert {
    pub claim_id: String,
    pub source_note_id: String,
    pub description: String,
    pub confidence: f32,
    pub predicate_occurrences: usize,
    pub clarifying_question: String,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChallengerReport {
    pub generated_at: DateTime<Utc>,
    pub stale_alerts: Vec<StaleAlert>,
    pub contradiction_alerts: Vec<ContradictionAlert>,
    pub cross_region_alerts: Vec<CrossRegionAlert>,
    pub frontier_alerts: Vec<FrontierAlert>,
}
