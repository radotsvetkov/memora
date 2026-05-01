use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::Value;

use memora_llm::LlmClient;

use crate::claims::privacy_markers::{parse_privacy_spans, privacy_for_span};
use crate::claims::Claim;
use crate::note::Note;

pub const EXTRACTION_PROMPT_TEMPLATE: &str = r#"You extract atomic factual CLAIMS from a personal note. Output a JSON array only.
No prose. No markdown fences. No commentary.

A claim is one minimal statement of fact about a subject. Break compound
sentences into individual claims.

For each claim return these fields:
- s: subject (entity, project, person, concept) — short noun phrase
- p: predicate (relation between subject and object) — snake_case verb phrase
- o: object (value, description, or another entity) — short
- span_start: byte offset where the supporting text begins, in the note body
- span_end: byte offset where the supporting text ends, in the note body
- valid_from: ISO datetime if explicitly stated for this claim, otherwise null
- valid_until: ISO datetime if explicitly stated as ended/changed, otherwise null
- confidence: 0.0–1.0. Definite fact = 1.0. "I think/maybe" = 0.5. Speculation = 0.3.

DO NOT:
- Invent claims not literally supported by the text.
- Combine multiple facts into one claim.
- Paraphrase in ways that change meaning.
- Include sentences as claims; extract the proposition only.
- Use "..." or "[redacted]" as field values.

The byte offsets must point to the smallest contiguous span of the body that
supports the claim.

Note id: {{NOTE_ID}}
Note source: {{NOTE_SOURCE}}
Note created: {{NOTE_CREATED}}
Note body (with byte offsets visible as comments at every line start):

{{NOTE_BODY_WITH_OFFSETS}}

OUTPUT JSON ARRAY ONLY."#;

#[derive(Clone)]
pub struct ClaimExtractor {
    pub llm: Arc<dyn LlmClient>,
    pub model_label: String,
}

impl ClaimExtractor {
    pub async fn extract(&self, note: &Note, body: &str) -> Result<Vec<Claim>> {
        let marker_spans = parse_privacy_spans(body);
        let prompt = self.render_prompt(note, body);
        let items = match self.llm.chat_json(&prompt, None, 2_000, 0.1).await {
            Ok(text) => match parse_llm_items(&text) {
                Ok(items) => items,
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "claim extractor returned malformed JSON; using heuristic extraction fallback"
                    );
                    heuristic_items(body)
                }
            },
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "claim extractor LLM call failed; using heuristic extraction fallback"
                );
                heuristic_items(body)
            }
        };
        let mut claims = Vec::new();

        for item in items {
            match self.build_claim_item(note, body, &marker_spans, &item) {
                Ok(claim) => claims.push(claim),
                Err(err) => {
                    tracing::warn!(error = %err, item = %item, "discarding invalid extracted claim");
                }
            }
        }

        Ok(claims)
    }

    fn render_prompt(&self, note: &Note, body: &str) -> String {
        EXTRACTION_PROMPT_TEMPLATE
            .replace("{{NOTE_ID}}", &note.fm.id)
            .replace("{{NOTE_SOURCE}}", &note.fm.source.to_string())
            .replace("{{NOTE_CREATED}}", &note.fm.created.to_rfc3339())
            .replace(
                "{{NOTE_BODY_WITH_OFFSETS}}",
                &render_body_with_offsets(body),
            )
    }

    fn build_claim_item(
        &self,
        note: &Note,
        body: &str,
        marker_spans: &[(usize, usize, crate::note::Privacy)],
        item: &Value,
    ) -> Result<Claim> {
        let subject = read_non_empty_field(item, "s")?;
        let predicate = read_non_empty_field(item, "p")?;
        let object = read_non_empty_field(item, "o")?;
        let span_start = read_usize_field(item, "span_start")?;
        let span_end = read_usize_field(item, "span_end")?;

        if span_end <= span_start {
            return Err(anyhow!("invalid span: span_end must be > span_start"));
        }
        if span_end > body.len() {
            return Err(anyhow!("invalid span: span_end out of bounds"));
        }
        let Some(span_text) = body.get(span_start..span_end) else {
            return Err(anyhow!("invalid span: not on UTF-8 boundary"));
        };
        let trimmed_len = span_text.trim().chars().count();
        if !(5..=300).contains(&trimmed_len) {
            return Err(anyhow!("invalid span text length: {trimmed_len}"));
        }

        let valid_from = parse_or_default(item.get("valid_from"), note.fm.created);
        let valid_until = parse_optional(item.get("valid_until"));
        let confidence = item
            .get("confidence")
            .and_then(Value::as_f64)
            .map(|value| value as f32)
            .unwrap_or(0.7);
        let privacy = privacy_for_span(span_start, span_end, marker_spans, note.fm.privacy);

        Ok(Claim {
            id: Claim::compute_id(&subject, &predicate, &object, &note.fm.id, span_start),
            subject,
            predicate,
            object,
            note_id: note.fm.id.clone(),
            span_start,
            span_end,
            span_fingerprint: Claim::compute_fingerprint(span_text),
            valid_from,
            valid_until,
            confidence,
            privacy,
            extracted_by: self.model_label.clone(),
            extracted_at: Utc::now(),
        })
    }
}

pub fn render_body_with_offsets(body: &str) -> String {
    let mut out = String::new();
    let mut offset = 0usize;
    for line in body.split_inclusive('\n') {
        out.push_str(&format!("[byte:{offset:06}] {line}"));
        offset += line.len();
    }
    if !body.ends_with('\n') && !body.is_empty() {
        return out;
    }
    if body.is_empty() {
        out.push_str("[byte:000000] ");
    }
    out
}

fn parse_llm_items(text: &str) -> Result<Vec<Value>> {
    let trimmed = text.trim();
    if let Ok(items) = serde_json::from_str::<Vec<Value>>(trimmed) {
        return Ok(items);
    }

    let re = Regex::new(r"(?s)\[.*\]").expect("array regex should compile");
    if let Some(matched) = re.find(trimmed) {
        if let Ok(items) = serde_json::from_str::<Vec<Value>>(matched.as_str()) {
            return Ok(items);
        }
    }

    tracing::debug!(
        raw_response = text,
        "failed to parse claim extractor response"
    );
    Err(anyhow!("claim extractor returned malformed JSON"))
}

fn read_non_empty_field(item: &Value, key: &str) -> Result<String> {
    let value = item
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| anyhow!("missing or empty field: {key}"))?;
    if value == "..." || value == "[redacted]" {
        return Err(anyhow!("invalid placeholder field value for: {key}"));
    }
    Ok(value.to_string())
}

fn read_usize_field(item: &Value, key: &str) -> Result<usize> {
    let raw = item
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("missing numeric field: {key}"))?;
    if raw < 0 {
        return Err(anyhow!("field must be >= 0: {key}"));
    }
    usize::try_from(raw).map_err(|_| anyhow!("field does not fit usize: {key}"))
}

fn parse_or_default(value: Option<&Value>, default: DateTime<Utc>) -> DateTime<Utc> {
    let Some(raw) = value.and_then(Value::as_str) else {
        return default;
    };
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or(default)
}

fn parse_optional(value: Option<&Value>) -> Option<DateTime<Utc>> {
    value
        .and_then(Value::as_str)
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

fn heuristic_items(body: &str) -> Vec<Value> {
    split_segments_with_spans(body)
        .into_iter()
        .take(8)
        .map(|(span_start, span_end, text)| {
            serde_json::json!({
                "s": "note",
                "p": "states",
                "o": text,
                "span_start": span_start as i64,
                "span_end": span_end as i64,
                "valid_from": Value::Null,
                "valid_until": Value::Null,
                "confidence": 0.5_f64,
            })
        })
        .collect()
}

fn split_segments_with_spans(body: &str) -> Vec<(usize, usize, String)> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    for (idx, ch) in body.char_indices() {
        if ch == '.' || ch == '!' || ch == '?' || ch == '\n' {
            let end = idx + ch.len_utf8();
            push_trimmed_segment(body, start, end, &mut segments);
            start = end;
        }
    }
    if start < body.len() {
        push_trimmed_segment(body, start, body.len(), &mut segments);
    }
    segments
}

fn push_trimmed_segment(
    body: &str,
    start: usize,
    end: usize,
    out: &mut Vec<(usize, usize, String)>,
) {
    let Some(raw) = body.get(start..end) else {
        return;
    };
    let trimmed_start_delta = raw.len() - raw.trim_start().len();
    let trimmed_end_delta = raw.len() - raw.trim_end().len();
    let span_start = start + trimmed_start_delta;
    let span_end = end.saturating_sub(trimmed_end_delta);
    if span_end <= span_start {
        return;
    }
    let Some(trimmed) = body.get(span_start..span_end) else {
        return;
    };
    let normalized = trimmed
        .trim_matches(|ch: char| ch.is_whitespace() || ch == '.' || ch == '!' || ch == '?')
        .trim();
    let char_len = normalized.chars().count();
    if !(8..=300).contains(&char_len) {
        return;
    }
    out.push((span_start, span_end, normalized.to_string()));
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::{TimeZone, Utc};

    use super::*;
    use std::sync::Arc;

    use crate::claims::mock::MockExtractorLlm;
    use crate::claims::ClaimStore;
    use crate::index::Index;
    use crate::note::{Frontmatter, Note, NoteSource, Privacy};

    fn make_note(id: &str, body: &str, privacy: Privacy) -> Note {
        Note {
            path: PathBuf::from(format!("vault/{id}.md")),
            fm: Frontmatter {
                id: id.to_string(),
                region: "test/unit".to_string(),
                source: NoteSource::Personal,
                privacy,
                created: Utc
                    .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                    .single()
                    .expect("valid created date"),
                updated: Utc
                    .with_ymd_and_hms(2026, 1, 2, 0, 0, 0)
                    .single()
                    .expect("valid updated date"),
                summary: "test note".to_string(),
                tags: Vec::new(),
                refs: Vec::new(),
            },
            body: body.to_string(),
            wikilinks: Vec::new(),
        }
    }

    fn make_extractor(canned_response: &str) -> ClaimExtractor {
        ClaimExtractor {
            llm: Arc::new(MockExtractorLlm {
                canned_response: canned_response.to_string(),
            }),
            model_label: "test/extractor".to_string(),
        }
    }

    #[test]
    fn render_body_with_offsets_includes_byte_markers() {
        let rendered = render_body_with_offsets("aa\nbbb\n");
        assert!(rendered.contains("[byte:000000] aa\n"));
        assert!(rendered.contains("[byte:000003] bbb\n"));
    }

    #[tokio::test]
    async fn extractor_accepts_two_valid_claims() {
        let body = "Rado works at HMC. INTERNORGA had 3500 exhibitors.";
        let first = body.find("Rado works at HMC").expect("first span");
        let second = body
            .find("INTERNORGA had 3500 exhibitors")
            .expect("second span");
        let response = format!(
            r#"[{{"s":"Rado","p":"works_at","o":"HMC","span_start":{first},"span_end":{},"valid_from":null,"valid_until":null,"confidence":1.0}},
{{"s":"INTERNORGA","p":"had_exhibitor_count","o":"3500 exhibitors","span_start":{second},"span_end":{},"valid_from":null,"valid_until":null,"confidence":1.0}}]"#,
            first + "Rado works at HMC".len(),
            second + "INTERNORGA had 3500 exhibitors".len()
        );
        let extractor = make_extractor(&response);
        let note = make_note("note-1", body, Privacy::Private);

        let claims = extractor
            .extract(&note, body)
            .await
            .expect("extract claims");
        assert_eq!(claims.len(), 2);
    }

    #[tokio::test]
    async fn extractor_rejects_out_of_bounds_span() {
        let body = "Rado works at HMC.";
        let response = r#"[{"s":"Rado","p":"works_at","o":"HMC","span_start":0,"span_end":1000,"valid_from":null,"valid_until":null,"confidence":1.0}]"#;
        let extractor = make_extractor(response);
        let note = make_note("note-2", body, Privacy::Private);

        let claims = extractor
            .extract(&note, body)
            .await
            .expect("extract claims");
        assert!(claims.is_empty());
    }

    #[tokio::test]
    async fn extractor_rejects_too_short_span_text() {
        let body = "abcde";
        let response = r#"[{"s":"X","p":"is","o":"Y","span_start":0,"span_end":4,"valid_from":null,"valid_until":null,"confidence":1.0}]"#;
        let extractor = make_extractor(response);
        let note = make_note("note-3", body, Privacy::Private);

        let claims = extractor
            .extract(&note, body)
            .await
            .expect("extract claims");
        assert!(claims.is_empty());
    }

    #[tokio::test]
    async fn extractor_inherits_secret_from_inline_marker() {
        let body = "Comp <!--privacy:secret-->salary 95k<!--/privacy--> note";
        let salary_start = body.find("salary 95k").expect("salary span");
        let salary_end = salary_start + "salary 95k".len();
        let response = format!(
            r#"[{{"s":"Comp","p":"has_salary","o":"95k","span_start":{salary_start},"span_end":{salary_end},"valid_from":null,"valid_until":null,"confidence":1.0}}]"#
        );
        let extractor = make_extractor(&response);
        let note = make_note("note-4", body, Privacy::Private);

        let claims = extractor
            .extract(&note, body)
            .await
            .expect("extract claims");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].privacy, Privacy::Secret);
    }

    #[tokio::test]
    async fn extractor_returns_error_for_malformed_json() {
        let body = "Rado works at HMC.";
        let extractor = make_extractor("{not json");
        let note = make_note("note-5", body, Privacy::Private);

        let claims = extractor.extract(&note, body).await.expect("must fallback");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].predicate, "states");
    }

    #[test]
    fn claim_store_round_trip_and_delete_for_note() {
        let index = Index::open(std::path::Path::new(":memory:")).expect("open index");
        let note = make_note("note-store", "Store body", Privacy::Private);
        index
            .upsert_note(&note, &note.body)
            .expect("upsert note for fk");

        let claim = Claim {
            id: Claim::compute_id("Rado", "works_at", "HMC", &note.fm.id, 0),
            subject: "Rado".to_string(),
            predicate: "works_at".to_string(),
            object: "HMC".to_string(),
            note_id: note.fm.id.clone(),
            span_start: 0,
            span_end: 10,
            span_fingerprint: Claim::compute_fingerprint("Store body"),
            valid_from: note.fm.created,
            valid_until: None,
            confidence: 1.0,
            privacy: Privacy::Private,
            extracted_by: "test".to_string(),
            extracted_at: Utc::now(),
        };

        let store = ClaimStore::new(&index);
        store.upsert(&claim).expect("upsert claim");
        let fetched = store
            .get(&claim.id)
            .expect("get claim")
            .expect("claim exists");
        assert_eq!(fetched.id, claim.id);

        store
            .delete_for_note(&note.fm.id)
            .expect("delete claims for note");
        assert!(store
            .get(&claim.id)
            .expect("get claim after delete")
            .is_none());
    }
}
