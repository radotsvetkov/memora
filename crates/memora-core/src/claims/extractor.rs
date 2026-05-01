use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use memora_llm::LlmClient;

use crate::claims::privacy_markers::{parse_privacy_spans, privacy_for_span};
use crate::claims::Claim;
use crate::note::Note;

pub const EXTRACTION_PROMPT_TEMPLATE: &str = r#"You extract atomic factual claims from a note as JSON.

OUTPUT FORMAT — return only a JSON array (no prose, no markdown):

[
  {
    "subject": "<entity, lowercase, hyphenated if multi-word>",
    "predicate": "<verb_or_relation, lowercase, snake_case>",
    "object": "<entity or value, lowercase>",
    "span_start": <byte offset where claim source begins>,
    "span_end": <byte offset where claim source ends>,
    "valid_from": "<ISO 8601 datetime or null>",
    "valid_until": "<ISO 8601 datetime or null>",
    "confidence": <0.0 to 1.0>
  }
]

RULES:
- Return at most 8 claims per call.
- Return [] if the note has no extractable atomic claims (too short,
  too vague, only narrative).
- Each claim must be a single atomic fact. Compound facts split
  into multiple claims.
- Span offsets refer to byte ranges in the note BODY (not frontmatter).
- Output only the JSON array. No code fences. No commentary.

GOOD example:

Input note body:
  "We decided on Postgres for the user store on 2026-04-15.
  Earlier we'd considered MongoDB but ruled it out due to schema drift."

Output:
  [
    {
      "subject": "user-store",
      "predicate": "uses_database",
      "object": "postgres",
      "span_start": 21,
      "span_end": 29,
      "valid_from": "2026-04-15T00:00:00Z",
      "valid_until": null,
      "confidence": 0.95
    },
    {
      "subject": "mongodb",
      "predicate": "rejected_for",
      "object": "user-store",
      "span_start": 80,
      "span_end": 87,
      "valid_from": null,
      "valid_until": null,
      "confidence": 0.85
    }
  ]

BAD examples — do NOT produce these:

Bare single object: {"subject": "x", ...}
  (Must be wrapped in an array.)

Code fence: ```json
[...]
```
  (Output the array directly without code fences.)

Prose: "Here are the claims I extracted: [...]"
  (No commentary. JSON only.)

Empty fields: {"subject": "x", "predicate": "is", "object": ""}
  (If the object would be empty, the claim is not atomic; skip it.)

- Never use JSON null for subject, predicate, or object. Omit the claim instead.

Note id: {{NOTE_ID}}
Note source: {{NOTE_SOURCE}}
Note created: {{NOTE_CREATED}}
Note body (with byte offsets visible as comments at every line start):

{{NOTE_BODY_WITH_OFFSETS}}

OUTPUT JSON ARRAY ONLY."#;

/// Single claim record as returned by the LLM (before privacy / fingerprint).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LlmClaimRecord {
    /// Local models sometimes emit JSON `null` here; treat as missing and discard in validation.
    #[serde(default, alias = "s")]
    pub subject: Option<String>,
    #[serde(default, alias = "p")]
    pub predicate: Option<String>,
    #[serde(default, alias = "o")]
    pub object: Option<String>,
    pub span_start: i64,
    pub span_end: i64,
    #[serde(default)]
    pub valid_from: Option<Value>,
    #[serde(default)]
    pub valid_until: Option<Value>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Clone)]
pub struct ClaimExtractor {
    pub llm: Arc<dyn LlmClient>,
    pub model_label: String,
}

impl ClaimExtractor {
    pub async fn extract(&self, note: &Note, body: &str) -> Result<Vec<Claim>> {
        let marker_spans = parse_privacy_spans(body);
        let prompt = self.render_prompt(note, body);
        let text = self
            .llm
            .chat_json(&prompt, None, 2_000, 0.1)
            .await
            .map_err(|err| {
                tracing::warn!(
                    error = %err,
                    "claim extractor LLM call failed"
                );
                anyhow::Error::from(err)
            })?;

        let records = parse_extraction_response(&text).map_err(|err| {
            tracing::warn!(
                error = %err,
                raw_response = %text,
                "claim extractor failed to parse JSON response"
            );
            err
        })?;

        let mut claims = Vec::new();

        for item in records.into_iter().take(8) {
            match self.build_claim_item(note, body, &marker_spans, &item) {
                Ok(claim) => claims.push(claim),
                Err(err) => {
                    tracing::warn!(error = %err, ?item, "discarding invalid extracted claim");
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
        item: &LlmClaimRecord,
    ) -> Result<Claim> {
        let subject = non_empty_trimmed(&item.subject, "subject")?;
        let predicate = non_empty_trimmed(&item.predicate, "predicate")?;
        let object = non_empty_trimmed(&item.object, "object")?;
        if subject == "..."
            || predicate == "..."
            || object == "..."
            || subject == "[redacted]"
            || predicate == "[redacted]"
            || object == "[redacted]"
        {
            return Err(anyhow!("invalid placeholder field value"));
        }

        let span_start = usize::try_from(item.span_start)
            .map_err(|_| anyhow!("span_start does not fit usize"))?;
        let span_end =
            usize::try_from(item.span_end).map_err(|_| anyhow!("span_end does not fit usize"))?;

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

        let valid_from = datetime_or_default(&item.valid_from, note.fm.created);
        let valid_until = optional_datetime(&item.valid_until);
        let confidence = item.confidence.map(|v| v as f32).unwrap_or(0.7);
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

fn non_empty_trimmed(opt: &Option<String>, field: &str) -> Result<String> {
    let Some(raw) = opt else {
        return Err(anyhow!("missing or empty field: {field}"));
    };
    let value = raw.trim();
    if value.is_empty() {
        return Err(anyhow!("missing or empty field: {field}"));
    }
    Ok(value.to_string())
}

fn datetime_or_default(value: &Option<Value>, default: DateTime<Utc>) -> DateTime<Utc> {
    let Some(raw) = value else {
        return default;
    };
    if raw.is_null() {
        return default;
    }
    let Some(s) = raw.as_str() else {
        return default;
    };
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or(default)
}

fn optional_datetime(value: &Option<Value>) -> Option<DateTime<Utc>> {
    let Some(raw) = value else {
        return None;
    };
    if raw.is_null() {
        return None;
    }
    raw.as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

/// Strip optional markdown code fences; tolerates ```json and generic ``` blocks.
pub(crate) fn strip_code_fences(s: &str) -> String {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("```json") {
        let rest = rest.trim_start_matches(['\r', '\n']);
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim().to_string();
        }
    }
    if let Some(rest) = s.strip_prefix("```JSON") {
        let rest = rest.trim_start_matches(['\r', '\n']);
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim().to_string();
        }
    }
    if let Some(rest) = s.strip_prefix("```") {
        let rest = rest.trim_start_matches(['\r', '\n']);
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim().to_string();
        }
    }
    s.to_string()
}

/// Parse LLM JSON into claim records, accepting several response shapes.
pub(crate) fn parse_extraction_response(raw: &str) -> Result<Vec<LlmClaimRecord>> {
    let trimmed = strip_code_fences(raw).trim().to_string();

    // Shape 1: array of claims (the requested format)
    if let Ok(claims) = serde_json::from_str::<Vec<LlmClaimRecord>>(&trimmed) {
        return Ok(claims);
    }

    // Shape 2: wrapped object {"claims": [...]}
    #[derive(Deserialize)]
    struct Wrapped {
        claims: Vec<LlmClaimRecord>,
    }
    if let Ok(wrapped) = serde_json::from_str::<Wrapped>(&trimmed) {
        return Ok(wrapped.claims);
    }

    // Shape 3: single bare claim object — wrap into a vec
    if let Ok(claim) = serde_json::from_str::<LlmClaimRecord>(&trimmed) {
        return Ok(vec![claim]);
    }

    // Shape 4: generic JSON value (array of heterogeneous objects, extra wrappers)
    if let Ok(value) = serde_json::from_str::<Value>(&trimmed) {
        if let Some(arr) = value.as_array() {
            let mut out = Vec::with_capacity(arr.len());
            for v in arr {
                match serde_json::from_value::<LlmClaimRecord>(v.clone()) {
                    Ok(c) => out.push(c),
                    Err(_) => continue,
                }
            }
            if !out.is_empty() {
                return Ok(out);
            }
        }
        if let Some(obj) = value.as_object() {
            for (_k, v) in obj {
                if let Ok(arr) = serde_json::from_value::<Vec<LlmClaimRecord>>(v.clone()) {
                    return Ok(arr);
                }
            }
        }
    }

    tracing::debug!(
        raw_response = raw,
        "failed to parse claim extractor response after all shapes"
    );
    bail!("could not parse claims from response: {}", trimmed)
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

    #[test]
    fn parse_array_of_claims_succeeds() {
        let raw = r#"[{"subject":"a","predicate":"b","object":"c","span_start":0,"span_end":5,"valid_from":null,"valid_until":null,"confidence":1.0}]"#;
        let v = parse_extraction_response(raw).expect("parse");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].subject.as_deref(), Some("a"));
    }

    #[test]
    fn parse_wrapped_object_succeeds() {
        let raw = r#"{"claims":[{"subject":"x","predicate":"y","object":"z","span_start":0,"span_end":5,"valid_from":null,"valid_until":null,"confidence":0.5}]}"#;
        let v = parse_extraction_response(raw).expect("parse");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].predicate.as_deref(), Some("y"));
    }

    #[test]
    fn parse_single_bare_claim_wraps_into_vec() {
        let raw = r#"{"s":"follow-up","p":"happens","o":"tomorrow","span_start":0,"span_end":19,"valid_from":null,"valid_until":null,"confidence":1.0}"#;
        let v = parse_extraction_response(raw).expect("parse");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].subject.as_deref(), Some("follow-up"));
    }

    #[test]
    fn parse_bare_object_with_json_null_object_field_succeeds() {
        let raw = r#"{"subject":"x","predicate":"y","object":null,"span_start":0,"span_end":5,"valid_from":null,"valid_until":null,"confidence":0.5}"#;
        let v = parse_extraction_response(raw).expect("JSON null must deserialize");
        assert_eq!(v.len(), 1);
        assert!(v[0].object.is_none());
    }

    #[test]
    fn parse_response_with_code_fences_strips_them() {
        let raw = "```json\n[{\"subject\":\"a\",\"predicate\":\"b\",\"object\":\"c\",\"span_start\":0,\"span_end\":5,\"valid_from\":null,\"valid_until\":null,\"confidence\":1.0}]\n```";
        let v = parse_extraction_response(raw).expect("parse");
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn parse_short_field_names_via_aliases() {
        let raw = r#"[{"s":"x","p":"y","o":"z","span_start":0,"span_end":5,"valid_from":null,"valid_until":null}]"#;
        let v = parse_extraction_response(raw).expect("parse");
        assert_eq!(v[0].subject.as_deref(), Some("x"));
        assert_eq!(v[0].predicate.as_deref(), Some("y"));
        assert_eq!(v[0].object.as_deref(), Some("z"));
    }

    #[test]
    fn parse_garbage_returns_err() {
        let err = parse_extraction_response("not json at all").unwrap_err();
        assert!(err.to_string().contains("could not parse claims"), "{err}");
    }

    #[test]
    fn heuristic_fallback_fn_removed() {
        // Built at compile time so this source file does not contain the substring literally.
        let forbidden = concat!("fn ", "heuristic_", "items");
        let src = include_str!("extractor.rs");
        assert!(
            !src.contains(forbidden),
            "heuristic fallback must stay removed"
        );
    }

    #[tokio::test]
    async fn extractor_accepts_two_valid_claims() {
        let body = "Rado works at HMC. INTERNORGA had 3500 exhibitors.";
        let first = body.find("Rado works at HMC").expect("first span");
        let second = body
            .find("INTERNORGA had 3500 exhibitors")
            .expect("second span");
        let response = format!(
            r#"[{{"subject":"Rado","predicate":"works_at","object":"HMC","span_start":{first},"span_end":{},"valid_from":null,"valid_until":null,"confidence":1.0}},
{{"subject":"INTERNORGA","predicate":"had_exhibitor_count","object":"3500 exhibitors","span_start":{second},"span_end":{},"valid_from":null,"valid_until":null,"confidence":1.0}}]"#,
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
        let response = r#"[{"subject":"Rado","predicate":"works_at","object":"HMC","span_start":0,"span_end":1000,"valid_from":null,"valid_until":null,"confidence":1.0}]"#;
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
        let response = r#"[{"subject":"X","predicate":"is","object":"Y","span_start":0,"span_end":4,"valid_from":null,"valid_until":null,"confidence":1.0}]"#;
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
            r#"[{{"subject":"Comp","predicate":"has_salary","object":"95k","span_start":{salary_start},"span_end":{salary_end},"valid_from":null,"valid_until":null,"confidence":1.0}}]"#
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

        let err = extractor
            .extract(&note, body)
            .await
            .expect_err("parse must fail");
        assert!(
            err.to_string().contains("could not parse claims")
                || err.to_string().contains("failed to parse"),
            "{err}"
        );
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
