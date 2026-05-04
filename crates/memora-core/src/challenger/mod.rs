pub mod report;
pub mod scan;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::claims::Claim;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub subject: String,
    pub object: String,
    pub source_note_ids: Vec<String>,
    pub supporting_claim_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contradiction {
    pub subject: String,
    pub family: String,
    pub left_object: String,
    pub right_object: String,
    pub left_source_note_ids: Vec<String>,
    pub right_source_note_ids: Vec<String>,
    pub left_claim_ids: Vec<String>,
    pub right_claim_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleDep {
    pub dependent_subject: String,
    pub stale_subject: String,
    pub superseded_by: String,
    pub depends_on_source_note_ids: Vec<String>,
    pub superseded_source_note_ids: Vec<String>,
    pub depends_on_claim_ids: Vec<String>,
    pub superseded_claim_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenQuestion {
    pub subject: String,
    pub family: String,
    pub object: String,
    pub source_note_ids: Vec<String>,
    pub supporting_claim_ids: Vec<String>,
}

pub fn detect_recent_decisions(claims: &[Claim]) -> Vec<Decision> {
    let mut grouped = BTreeMap::<(String, String), Vec<&Claim>>::new();
    for claim in claims {
        if !is_recent_decision_predicate(&claim.predicate) {
            continue;
        }
        let Some(object) = claim
            .object
            .as_ref()
            .map(|v| normalize_decision_object(v))
            .filter(|v| !v.is_empty())
        else {
            continue;
        };
        let subject = normalize_term(&claim.subject);
        grouped.entry((subject, object)).or_default().push(claim);
    }

    let mut out = grouped
        .into_iter()
        .filter_map(|((subject, _normalized_object), entries)| {
            let supporting_claim_ids = unique_claim_ids(&entries);
            let strong_predicate_present = entries
                .iter()
                .any(|claim| is_strong_decision_predicate(&claim.predicate));
            if supporting_claim_ids.len() < 2 && !strong_predicate_present {
                return None;
            }
            Some(Decision {
                subject,
                object: canonical_decision_object(&entries),
                source_note_ids: unique_note_ids(&entries),
                supporting_claim_ids,
            })
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| {
        b.source_note_ids
            .len()
            .cmp(&a.source_note_ids.len())
            .then_with(|| a.subject.cmp(&b.subject))
            .then_with(|| a.object.cmp(&b.object))
    });
    out
}

pub fn detect_contradictions(claims: &[Claim]) -> Vec<Contradiction> {
    let mut grouped = BTreeMap::<(String, String), BTreeMap<String, Vec<&Claim>>>::new();
    for claim in claims {
        let Some(family) = contradiction_family(&claim.predicate) else {
            continue;
        };
        let Some(object) = claim
            .object
            .as_ref()
            .map(|v| normalize_decision_object(v))
            .filter(|v| !v.is_empty())
        else {
            continue;
        };
        let subject = normalize_term(&claim.subject);
        grouped
            .entry((subject, family))
            .or_default()
            .entry(object)
            .or_default()
            .push(claim);
    }

    let mut out = Vec::new();
    for ((subject, family), objects) in grouped {
        if objects.len() < 2 {
            continue;
        }
        let object_groups = objects.into_iter().collect::<Vec<_>>();
        for i in 0..object_groups.len() {
            for j in (i + 1)..object_groups.len() {
                let (_left_object, left_entries) = &object_groups[i];
                let (_right_object, right_entries) = &object_groups[j];
                let left_source_note_ids = unique_note_ids(left_entries);
                let right_source_note_ids = unique_note_ids(right_entries);
                if left_source_note_ids.is_empty() || right_source_note_ids.is_empty() {
                    continue;
                }
                out.push(Contradiction {
                    subject: subject.clone(),
                    family: family.clone(),
                    left_object: canonical_decision_object(left_entries),
                    right_object: canonical_decision_object(right_entries),
                    left_source_note_ids,
                    right_source_note_ids,
                    left_claim_ids: unique_claim_ids(left_entries),
                    right_claim_ids: unique_claim_ids(right_entries),
                });
            }
        }
    }

    out.sort_by(|a, b| {
        let a_total = a.left_source_note_ids.len() + a.right_source_note_ids.len();
        let b_total = b.left_source_note_ids.len() + b.right_source_note_ids.len();
        b_total
            .cmp(&a_total)
            .then_with(|| a.subject.cmp(&b.subject))
            .then_with(|| a.family.cmp(&b.family))
    });
    out
}

pub fn detect_stale_dependencies(claims: &[Claim]) -> Vec<StaleDep> {
    let mut superseded = HashMap::<String, HashMap<String, Vec<&Claim>>>::new();
    for claim in claims {
        if claim.predicate != "superseded_by" {
            continue;
        }
        let Some(new_target) = claim.object.as_ref().map(|v| normalize_term(v)) else {
            continue;
        };
        superseded
            .entry(normalize_term(&claim.subject))
            .or_default()
            .entry(new_target)
            .or_default()
            .push(claim);
    }

    let mut depends_on = HashMap::<(String, String), Vec<&Claim>>::new();
    for claim in claims {
        if claim.predicate != "depends_on" {
            continue;
        }
        let Some(dep_target) = claim.object.as_ref().map(|v| normalize_term(v)) else {
            continue;
        };
        let dependent = normalize_term(&claim.subject);
        depends_on.entry((dependent, dep_target)).or_default().push(claim);
    }

    let mut out = Vec::new();
    for ((dependent_subject, stale_subject), depends_entries) in depends_on {
        let Some(replacements) = superseded.get(&stale_subject) else {
            continue;
        };
        for (superseded_by, superseded_entries) in replacements {
            out.push(StaleDep {
                dependent_subject: dependent_subject.clone(),
                stale_subject: stale_subject.clone(),
                superseded_by: superseded_by.clone(),
                depends_on_source_note_ids: unique_note_ids(&depends_entries),
                superseded_source_note_ids: unique_note_ids(superseded_entries),
                depends_on_claim_ids: unique_claim_ids(&depends_entries),
                superseded_claim_ids: unique_claim_ids(superseded_entries),
            });
        }
    }

    out.sort_by(|a, b| {
        let a_total = a.depends_on_source_note_ids.len() + a.superseded_source_note_ids.len();
        let b_total = b.depends_on_source_note_ids.len() + b.superseded_source_note_ids.len();
        b_total
            .cmp(&a_total)
            .then_with(|| a.dependent_subject.cmp(&b.dependent_subject))
    });
    out
}

pub fn detect_open_questions(claims: &[Claim]) -> Vec<OpenQuestion> {
    let mut grouped = BTreeMap::<(String, String, String), Vec<&Claim>>::new();
    for claim in claims {
        let Some(family) = open_question_family(&claim.predicate) else {
            continue;
        };
        let subject = normalize_term(&claim.subject);
        let object = normalize_term(claim.object.as_deref().unwrap_or("(unary)"));
        grouped.entry((subject, family, object)).or_default().push(claim);
    }

    let mut out = grouped
        .into_iter()
        .map(|((subject, family, object), entries)| OpenQuestion {
            subject,
            family,
            object,
            source_note_ids: unique_note_ids(&entries),
            supporting_claim_ids: unique_claim_ids(&entries),
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| {
        b.source_note_ids
            .len()
            .cmp(&a.source_note_ids.len())
            .then_with(|| a.subject.cmp(&b.subject))
            .then_with(|| a.object.cmp(&b.object))
    });
    out
}

fn normalize_term(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn is_recent_decision_predicate(predicate: &str) -> bool {
    let p = predicate.to_ascii_lowercase();
    [
        "switched_to_",
        "switched_",
        "decided_to_",
        "will_switch_",
        "planned_switch_",
        "may_switch_",
        "chose_",
        "adopted_",
        "will_use_",
    ]
    .iter()
    .any(|prefix| p.starts_with(prefix))
        || p.starts_with("uses_")
        || matches!(p.as_str(), "decision_candidate")
}

fn is_strong_decision_predicate(predicate: &str) -> bool {
    let p = predicate.to_ascii_lowercase();
    matches!(p.as_str(), "switched_to" | "decided" | "chose" | "adopted")
        || p.starts_with("switched_to_")
        || p.starts_with("decided_to_")
        || p.starts_with("chose_")
        || p.starts_with("adopted_")
        || p.starts_with("will_switch_")
        || p.starts_with("planned_switch_")
}

fn open_question_family(predicate: &str) -> Option<String> {
    let p = predicate.to_ascii_lowercase();
    if p.starts_with("decision_pending") {
        return Some("decision_pending".to_string());
    }
    if p.starts_with("design_question") {
        return Some("design_question".to_string());
    }
    if p.starts_with("decision_candidate") {
        return Some("decision_candidate".to_string());
    }
    if p.starts_with("under_review") {
        return Some("under_review".to_string());
    }
    if p.starts_with("evaluating_") {
        return Some("evaluating".to_string());
    }
    if p.starts_with("design_tension") {
        return Some("design_tension".to_string());
    }
    None
}

fn contradiction_family(predicate: &str) -> Option<String> {
    let p = predicate.to_ascii_lowercase();
    if is_additive_predicate(&p) {
        return None;
    }
    if !is_exclusive_predicate(&p) {
        return None;
    }
    if matches!(p.as_str(), "uses_language" | "implemented_in" | "language") {
        return Some("language".to_string());
    }
    if matches!(
        p.as_str(),
        "uses_architecture" | "uses_worker_model" | "architecture"
    ) {
        return Some("architecture".to_string());
    }
    Some(p)
}

fn is_exclusive_predicate(predicate: &str) -> bool {
    matches!(
        predicate,
        "uses_language"
            | "implemented_in"
            | "uses_architecture"
            | "uses_worker_model"
            | "uses_client_generator"
            | "uses_default_provider"
            | "runs_queue_workers_in"
            | "language"
            | "architecture"
    ) || predicate.starts_with("switched_to_")
        || predicate.starts_with("decided_")
        || predicate.starts_with("chose_")
}

fn is_additive_predicate(predicate: &str) -> bool {
    matches!(
        predicate,
        "appears_in" | "documented_in" | "affects" | "feeds_into" | "references" | "uses_tool"
    ) || predicate.starts_with("has_field_observation_")
        || predicate.starts_with("appears_in_")
        || predicate.starts_with("documented_in_")
}

fn normalize_decision_object(value: &str) -> String {
    let mut normalized = normalize_term(value).replace([' ', '_'], "-");
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

fn canonical_decision_object(entries: &[&Claim]) -> String {
    let mut counts = HashMap::<String, usize>::new();
    for claim in entries {
        if let Some(object) = claim.object.as_deref() {
            let raw = normalize_term(object);
            *counts.entry(raw).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .max_by(|(obj_a, count_a), (obj_b, count_b)| {
            count_a.cmp(count_b).then_with(|| obj_a.len().cmp(&obj_b.len()))
        })
        .map(|(obj, _)| obj)
        .unwrap_or_else(|| "(unary)".to_string())
}

fn unique_note_ids(entries: &[&Claim]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for claim in entries {
        if seen.insert(claim.note_id.clone()) {
            out.push(claim.note_id.clone());
        }
    }
    out
}

fn unique_claim_ids(entries: &[&Claim]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for claim in entries {
        if seen.insert(claim.id.clone()) {
            out.push(claim.id.clone());
        }
    }
    out
}

pub use report::{
    ChallengerReport, ContradictionAlert, CrossRegionAlert, FrontierAlert, StaleAlert,
};
pub use scan::{Challenger, ChallengerConfig};

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::note::Privacy;

    fn claim(
        id: &str,
        note_id: &str,
        subject: &str,
        predicate: &str,
        object: Option<&str>,
    ) -> Claim {
        let ts = Utc
            .with_ymd_and_hms(2026, 4, 1, 0, 0, 0)
            .single()
            .expect("valid ts");
        Claim {
            id: id.to_string(),
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: object.map(ToString::to_string),
            note_id: note_id.to_string(),
            span_start: 0,
            span_end: 10,
            span_fingerprint: "fp".to_string(),
            valid_from: ts,
            valid_until: None,
            confidence: 0.9,
            privacy: Privacy::Private,
            extracted_by: "test".to_string(),
            extracted_at: ts,
        }
    }

    #[test]
    fn test_detects_simple_decision() {
        let claims = vec![
            claim(
                "c1",
                "akmon-01",
                "akmon",
                "uses_api_client_generator",
                Some("stainless-templates"),
            ),
            claim(
                "c2",
                "akmon-02",
                "akmon",
                "will_switch_default_to",
                Some("stainless-templates"),
            ),
        ];
        let decisions = detect_recent_decisions(&claims);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].subject, "akmon");
        assert_eq!(decisions[0].object, "stainless-templates");
        assert_eq!(decisions[0].source_note_ids.len(), 2);
    }

    #[test]
    fn test_detects_predicate_family_contradiction() {
        let claims = vec![
            claim("c1", "csv-01", "csv-tool", "uses_language", Some("rust")),
            claim("c2", "ep-01", "csv-tool", "implemented_in", Some("python")),
        ];
        let contradictions = detect_contradictions(&claims);
        assert_eq!(contradictions.len(), 1);
        assert_eq!(contradictions[0].family, "language");
    }

    #[test]
    fn test_detects_cross_region_contradiction_appears_in_both() {
        let claims = vec![
            claim("c1", "csv-01", "csv-tool", "uses_language", Some("rust")),
            claim("c2", "ep-01", "csv-tool", "uses_language", Some("python")),
        ];
        let contradictions = detect_contradictions(&claims);
        assert_eq!(contradictions.len(), 1);
        assert_eq!(contradictions[0].left_source_note_ids.len(), 1);
        assert_eq!(contradictions[0].right_source_note_ids.len(), 1);
    }

    #[test]
    fn test_stale_dependency_chain_detection() {
        let claims = vec![
            claim(
                "c1",
                "m-01",
                "retrieval-eval-notes",
                "superseded_by",
                Some("retrieval-eval-notes-v2"),
            ),
            claim(
                "c2",
                "m-02",
                "staleness-case-a-synthesis",
                "depends_on",
                Some("retrieval-eval-notes"),
            ),
        ];
        let stale = detect_stale_dependencies(&claims);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].dependent_subject, "staleness-case-a-synthesis");
    }

    #[test]
    fn test_open_question_grouping() {
        let claims = vec![
            claim(
                "c1",
                "m-01",
                "memora",
                "decision_pending",
                Some("precision-vs-recall"),
            ),
            claim(
                "c2",
                "m-02",
                "memora",
                "decision_pending_followup",
                Some("precision-vs-recall"),
            ),
        ];
        let questions = detect_open_questions(&claims);
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].source_note_ids.len(), 2);
    }

    #[test]
    fn test_object_normalization_merges_stainless_variants() {
        let claims = vec![
            claim(
                "c1",
                "n1",
                "akmon",
                "switched_to_api_client_generator",
                Some("stainless"),
            ),
            claim(
                "c2",
                "n2",
                "akmon",
                "will_switch_default_to",
                Some("stainless-templates"),
            ),
        ];
        let decisions = detect_recent_decisions(&claims);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].source_note_ids.len(), 2);
    }

    #[test]
    fn test_predicate_exclusivity_suppresses_appears_in_contradiction() {
        let claims = vec![
            claim(
                "c1",
                "n1",
                "akmon",
                "appears_in",
                Some("concept-reference-notes"),
            ),
            claim("c2", "n2", "akmon", "appears_in", Some("episodic-daily-logs")),
        ];
        assert!(detect_contradictions(&claims).is_empty());
    }

    #[test]
    fn test_cross_region_scoping_open_questions_subject_aggregation() {
        let claims = vec![
            claim(
                "c1",
                "memora-04",
                "memora",
                "decision_pending",
                Some("precision-vs-recall-tradeoff"),
            ),
            claim(
                "c2",
                "ep-daily-2026-04-10",
                "memora",
                "design_question",
                Some("precision-vs-recall-tradeoff"),
            ),
        ];
        let findings = detect_open_questions(&claims);
        assert_eq!(findings.len(), 2);
        let total_sources = findings
            .iter()
            .flat_map(|f| f.source_note_ids.iter())
            .collect::<BTreeSet<_>>()
            .len();
        assert_eq!(total_sources, 2);
    }

    #[test]
    fn test_empty_input_returns_none() {
        assert!(detect_recent_decisions(&[]).is_empty());
        assert!(detect_contradictions(&[]).is_empty());
        assert!(detect_stale_dependencies(&[]).is_empty());
        assert!(detect_open_questions(&[]).is_empty());
    }

    #[test]
    fn test_findings_sorted_by_source_count() {
        let claims = vec![
            claim("c1", "n1", "akmon", "will_switch_to", Some("stainless")),
            claim("c2", "n2", "akmon", "may_switch_to", Some("stainless")),
            claim("c3", "n3", "csv-tool", "will_switch_to", Some("rust")),
        ];
        let decisions = detect_recent_decisions(&claims);
        assert_eq!(decisions[0].subject, "akmon");
        assert!(decisions[0].source_note_ids.len() > decisions[1].source_note_ids.len());
    }

    #[test]
    fn test_decision_threshold_suppresses_weak_single_claim() {
        let weak_only = vec![claim(
            "c0",
            "n0",
            "akmon",
            "uses_client_generator",
            Some("stainless-templates"),
        )];
        assert!(detect_recent_decisions(&weak_only).is_empty());

        let claims = vec![
            claim(
                "c1",
                "n1",
                "akmon",
                "uses_client_generator",
                Some("stainless-templates"),
            ),
            claim(
                "c2",
                "n2",
                "akmon",
                "switched_to_api_client_generator",
                Some("stainless-templates"),
            ),
        ];
        let decisions = detect_recent_decisions(&claims);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].supporting_claim_ids.len(), 2);
    }
}
