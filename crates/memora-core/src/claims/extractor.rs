use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

use memora_llm::{LlmClient, LlmError};

use crate::claims::privacy_markers::{parse_privacy_spans, privacy_for_span};
use crate::claims::Claim;
use crate::note::Note;

pub const EXTRACTION_PROMPT_TEMPLATE: &str = r#"You extract atomic factual claims from a note as JSON.

OUTPUT FORMAT — return only a JSON array (no prose, no markdown):

[
  {
    "subject": "<entity, lowercase, hyphenated if multi-word>",
    "predicate": "<verb_or_relation, lowercase, snake_case>",
    "object": "<OPTIONAL: entity or value, lowercase — omit or null for unary predicates>",
    "span_start": <byte offset where claim source begins>,
    "span_end": <byte offset where claim source ends>,
    "valid_from": "<ISO 8601 datetime or null>",
    "valid_until": "<ISO 8601 datetime or null>",
    "confidence": <0.0 to 1.0>
  }
]

In the array, each claim has these fields:
- subject (required, non-empty)
- predicate (required, non-empty, snake_case)
- object (OPTIONAL — omit or use null for unary predicates that describe a state,
  like "completed", "active", "deprecated", or "in_early_stages")
- span_start, span_end: byte offsets into the note body
- valid_from, valid_until: ISO dates or null
- confidence: 0.0 to 1.0

EXAMPLES of unary predicates (object omitted):
- {"subject": "akmon-launch", "predicate": "completed", "span_start": 0, "span_end": 42, "valid_from": null, "valid_until": null, "confidence": 0.9}
- {"subject": "csv-tool", "predicate": "in_alpha_phase", "span_start": 0, "span_end": 60, "valid_from": null, "valid_until": null, "confidence": 0.85}
- {"subject": "memora-prompts", "predicate": "deprecated", "span_start": 0, "span_end": 30, "valid_from": null, "valid_until": null, "confidence": 0.8}

EXAMPLES of binary predicates (with object):
- {"subject": "csv-tool", "predicate": "uses_language", "object": "rust", ...}
- {"subject": "akmon", "predicate": "depends_on", "object": "openapi-generator", ...}

Rule: if a fact has a clear "what is it about" (object), use binary.
Otherwise unary. Don't pad with empty strings.

RULES:
- Return at most 8 claims per call.
- Return [] if the note has no extractable atomic claims (too short,
  too vague, only narrative).
- Each claim must be a single atomic fact. Compound facts split
  into multiple claims.
- Span offsets refer to byte ranges in the note BODY (not frontmatter).
- Output only the JSON array. No code fences. No commentary.

SUBJECT CONSTRUCTION:
- Subjects should be lowercase and use hyphens for multi-word entities.
- The underlying phrase (after un-hyphenating) MUST appear in the note body.
- Good: body says "the Akmon project decided" → subject "akmon".
- Good: body says "Mila reviewed the decision log" → subject "mila".
- Good: body says "csv-tool benchmark Q1" → subject "csv-tool" (already kebab).
- Bad: body says "we discussed deployment" → subject "deployment-strategy"
  ("deployment-strategy" is not in the body; fabrication).
- If the note title implies a subject (e.g., title is "meeting-akmon-release-readiness"),
  it is fine to use that as a subject if the body discusses it.

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

Empty object string: {"subject": "x", "predicate": "is", "object": ""}
  (Use unary form: omit "object" or use null — never an empty string.)

- Never use JSON null for subject or predicate. Omit the claim instead.

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
    /// Optional; omit or null for unary predicates. Empty string is normalized away.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimExtractionDisposition {
    Extracted,
    Empty,
    AllInvalidAfterRetry,
}

#[derive(Debug, Clone)]
pub struct ClaimExtractionResult {
    pub claims: Vec<Claim>,
    pub disposition: ClaimExtractionDisposition,
}

#[derive(Debug, Error)]
pub enum ClaimExtractionError {
    #[error("rate limited")]
    RateLimited,
    #[error("failed to parse extraction response: {0}")]
    Parse(String),
    #[error("llm call failed: {0}")]
    Llm(String),
}

impl ClaimExtractor {
    pub async fn extract(&self, note: &Note, body: &str) -> Result<Vec<Claim>> {
        self.extract_with_metadata(note, body)
            .await
            .map(|result| result.claims)
            .map_err(anyhow::Error::from)
    }

    pub async fn extract_with_metadata(
        &self,
        note: &Note,
        body: &str,
    ) -> Result<ClaimExtractionResult, ClaimExtractionError> {
        let marker_spans = parse_privacy_spans(body);
        let prompt = self.build_prompt(note, body);
        let text = self
            .llm
            .chat_json(&prompt, None, 2_000, 0.1)
            .await
            .map_err(|err| {
                tracing::warn!(
                    error = %err,
                    "claim extractor LLM call failed"
                );
                classify_llm_error(err)
            })?;

        let records = parse_extraction_response(&text).map_err(|err| {
            tracing::warn!(
                error = %err,
                raw_response = %text,
                "claim extractor failed to parse JSON response"
            );
            ClaimExtractionError::Parse(err.to_string())
        })?;

        let mut claims = Self::validate_records(note, body, &marker_spans, &records, self);

        if claims.is_empty() && records.is_empty() {
            return Ok(ClaimExtractionResult {
                claims,
                disposition: ClaimExtractionDisposition::Empty,
            });
        }

        if claims.is_empty() && !records.is_empty() {
            tracing::info!(
                path = %note.path.display(),
                parsed_count = records.len(),
                "first extraction returned all-invalid claims; retrying with feedback"
            );
            let retry_prompt = self.build_retry_prompt(note, body);
            let retry_text = self
                .llm
                .chat_json(&retry_prompt, None, 2_000, 0.05)
                .await
                .map_err(|err| {
                    tracing::warn!(error = %err, "claim extractor retry LLM call failed");
                    classify_llm_error(err)
                })?;

            let retry_records = parse_extraction_response(&retry_text).map_err(|err| {
                tracing::warn!(
                    error = %err,
                    raw_response = %retry_text,
                    "claim extractor retry failed to parse JSON response"
                );
                ClaimExtractionError::Parse(err.to_string())
            })?;

            claims = Self::validate_records(note, body, &marker_spans, &retry_records, self);
            let disposition = if claims.is_empty() && !retry_records.is_empty() {
                ClaimExtractionDisposition::AllInvalidAfterRetry
            } else if claims.is_empty() {
                ClaimExtractionDisposition::Empty
            } else {
                ClaimExtractionDisposition::Extracted
            };
            return Ok(ClaimExtractionResult {
                claims,
                disposition,
            });
        }

        Ok(ClaimExtractionResult {
            claims,
            disposition: ClaimExtractionDisposition::Extracted,
        })
    }

    fn validate_records(
        note: &Note,
        body: &str,
        marker_spans: &[(usize, usize, crate::note::Privacy)],
        records: &[LlmClaimRecord],
        extractor: &ClaimExtractor,
    ) -> Vec<Claim> {
        let mut claims = Vec::new();
        for item in records.iter().take(8) {
            match validate_extracted_record(note, body, marker_spans, item, extractor) {
                Ok(claim) => claims.push(claim),
                Err(err) => {
                    tracing::warn!(error = %err, ?item, "discarding invalid extracted claim");
                }
            }
        }
        claims
    }

    fn build_retry_prompt(&self, note: &Note, body: &str) -> String {
        let base = self.build_prompt(note, body);
        format!(
            "{base}\n\
             \n\
             --- RETRY GUIDANCE ---\n\
             Your previous response did not produce usable claims. Try again \
             with these clarifications:\n\
             \n\
             1. The subject of each claim should be a phrase that ACTUALLY APPEARS \
                in the note body. You may normalize the case (lowercase) and \
                join multi-word subjects with hyphens, but the underlying phrase \
                must be in the text. Example: if the body says 'Akmon release \
                readiness', use subject 'akmon-release-readiness'.\n\
             \n\
             2. If the note has no extractable factual claims (e.g., it's just a \
                short capture or a question to yourself), return an empty array []. \
                Don't force claims out of vague content.\n\
             \n\
             3. For span_start and span_end, use byte offsets pointing to the \
                region of the body where the claim is expressed. If unsure, use \
                0 and the body length.\n\
             \n\
             4. The 'object' field is optional — omit it (use null) for unary \
                predicates like 'completed', 'in_progress', 'deprecated'.\n\
             \n\
             Return ONLY a JSON array. No prose. No code fences."
        )
    }

    fn build_prompt(&self, note: &Note, body: &str) -> String {
        EXTRACTION_PROMPT_TEMPLATE
            .replace("{{NOTE_ID}}", &note.fm.id)
            .replace("{{NOTE_SOURCE}}", &note.fm.source.to_string())
            .replace("{{NOTE_CREATED}}", &note.fm.created.to_rfc3339())
            .replace(
                "{{NOTE_BODY_WITH_OFFSETS}}",
                &render_body_with_offsets(body),
            )
    }
}

fn classify_llm_error(err: LlmError) -> ClaimExtractionError {
    match err {
        LlmError::RateLimited => ClaimExtractionError::RateLimited,
        other => ClaimExtractionError::Llm(other.to_string()),
    }
}

fn validate_extracted_record(
    note: &Note,
    body: &str,
    marker_spans: &[(usize, usize, crate::note::Privacy)],
    item: &LlmClaimRecord,
    extractor: &ClaimExtractor,
) -> Result<Claim> {
    let subject = item
        .subject
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("missing or empty field: subject"))?;
    let predicate = item
        .predicate
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("missing or empty field: predicate"))?;

    let object = item
        .object
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let object_for_placeholder = object.as_deref().unwrap_or("");
    if subject == "..."
        || predicate == "..."
        || object_for_placeholder == "..."
        || subject == "[redacted]"
        || predicate == "[redacted]"
        || object_for_placeholder == "[redacted]"
    {
        return Err(anyhow!("invalid placeholder field value"));
    }

    let (span_start, span_end) = resolve_claim_span(item, note, body, subject)?;

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
        id: Claim::compute_id(
            subject,
            predicate,
            object.as_deref(),
            &note.fm.id,
            span_start,
        ),
        subject: subject.to_string(),
        predicate: predicate.to_string(),
        object,
        note_id: note.fm.id.clone(),
        span_start,
        span_end,
        span_fingerprint: Claim::compute_fingerprint(span_text),
        valid_from,
        valid_until,
        confidence,
        privacy,
        extracted_by: extractor.model_label.clone(),
        extracted_at: Utc::now(),
    })
}

/// Filename stem without extension, e.g. `meeting-akmon.md` → `meeting-akmon`.
fn note_title_stem(note_path: &Path) -> String {
    note_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

/// Cases 1–3 only: subject is evidenced in `text` (used for LLM span slice checks).
fn subject_evidence_in_text(subject: &str, text: &str) -> bool {
    if text.find(subject).is_some() {
        return true;
    }
    let text_lower = text.to_lowercase();
    let subject_lower = subject.to_lowercase();
    if text_lower.find(&subject_lower).is_some() {
        return true;
    }
    let subject_normalized = subject.replace(['-', '_'], " ").to_lowercase();
    let sn = subject_normalized.trim();
    if sn.is_empty() {
        return false;
    }
    let text_normalized = text.to_lowercase();
    text_normalized.find(sn).is_some()
}

/// Byte index where the subject is grounded in the body, title, or wikilinks.
/// Second return: log kind for relaxed matches (`None` = exact literal in body).
fn subject_match_start(
    subject: &str,
    body: &str,
    note_title_stem: &str,
    wikilinks: &[String],
) -> Option<(usize, Option<&'static str>)> {
    if let Some(start) = body.find(subject) {
        return Some((start, None));
    }

    let body_lower = body.to_lowercase();
    let subject_lower = subject.to_lowercase();
    if let Some(start) = body_lower.find(&subject_lower) {
        return Some((start, Some("normalized")));
    }

    let subject_normalized = subject.replace(['-', '_'], " ").to_lowercase();
    let sn = subject_normalized.trim();
    if sn.is_empty() {
        return None;
    }
    let body_normalized = body.to_lowercase();
    if let Some(start) = body_normalized.find(sn) {
        return Some((start, Some("normalized")));
    }

    let title_normalized = note_title_stem.replace(['-', '_'], " ").to_lowercase();
    if title_normalized.contains(sn) {
        return Some((0, Some("title")));
    }

    for link in wikilinks {
        let link_normalized = link.replace(['-', '_'], " ").to_lowercase();
        let ln = link_normalized.trim();
        if ln == sn {
            if let Some(start) = body.find(link.as_str()) {
                return Some((start, Some("wikilink")));
            }
            return Some((0, Some("wikilink")));
        }
    }

    None
}

/// Prefer the LLM span when it fits the body, contains the subject, and passes length bounds.
/// Otherwise recover via substring search on `subject`.
fn resolve_claim_span(
    item: &LlmClaimRecord,
    note: &Note,
    body: &str,
    subject: &str,
) -> Result<(usize, usize)> {
    let body_len = body.len();
    let note_stem = note_title_stem(&note.path);
    let span_start_raw =
        usize::try_from(item.span_start).map_err(|_| anyhow!("span_start does not fit usize"))?;
    let span_end_raw =
        usize::try_from(item.span_end).map_err(|_| anyhow!("span_end does not fit usize"))?;

    let mut use_given = span_end_raw > span_start_raw
        && span_end_raw <= body_len
        && body.get(span_start_raw..span_end_raw).is_some();

    if use_given {
        let slice = body
            .get(span_start_raw..span_end_raw)
            .expect("bounds checked");
        let trimmed_len = slice.trim().chars().count();
        if !(5..=300).contains(&trimmed_len) || !subject_evidence_in_text(subject, slice) {
            use_given = false;
        }
    }

    if use_given {
        return Ok((span_start_raw, span_end_raw));
    }

    let Some((found_start, match_kind)) =
        subject_match_start(subject, body, &note_stem, &note.wikilinks)
    else {
        tracing::warn!(
            subject = %subject,
            "claim subject not found in note body; dropping likely-fabricated claim"
        );
        return Err(anyhow!("subject not found in note body"));
    };

    if let Some(kind) = match_kind {
        tracing::debug!(
            subject = %subject,
            match_kind = kind,
            "accepted claim via normalized subject match"
        );
    }

    let found_end = (found_start + subject.len() + 80).min(body_len);
    if found_end <= found_start {
        return Err(anyhow!("recovered span invalid"));
    }
    tracing::debug!(
        subject = %subject,
        original_span = ?(span_start_raw, span_end_raw),
        recovered_span = ?(found_start, found_end),
        "recovered claim span via substring search"
    );
    Ok((found_start, found_end))
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
    let unfenced = strip_code_fences(raw);
    let trimmed = unfenced.trim();

    if trimmed.is_empty() || trimmed == "{}" || trimmed == "null" || trimmed == "[]" {
        return Ok(Vec::new());
    }

    let trimmed = trimmed.to_string();

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

    use crate::claims::mock::{MockExtractorLlm, MockSequentialExtractorLlm};
    use crate::claims::ClaimStore;
    use crate::index::Index;
    use crate::note::{Frontmatter, Note, NoteSource, Privacy};

    fn make_note_with_path(path: PathBuf, id: &str, body: &str, wikilinks: Vec<String>) -> Note {
        let mut n = make_note(id, body, Privacy::Private);
        n.path = path;
        n.wikilinks = wikilinks;
        n
    }

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
    fn parse_empty_object_returns_empty_vec() {
        assert!(parse_extraction_response("{}").unwrap().is_empty());
    }

    #[test]
    fn parse_null_returns_empty_vec() {
        assert!(parse_extraction_response("null").unwrap().is_empty());
    }

    #[test]
    fn parse_empty_array_returns_empty_vec() {
        assert!(parse_extraction_response("[]").unwrap().is_empty());
    }

    #[test]
    fn subject_validation_accepts_exact_match() {
        let body = "akmon-release-readiness is tracked.";
        let got =
            subject_match_start("akmon-release-readiness", body, "other", &[]).expect("match");
        assert_eq!(got.0, 0);
        assert_eq!(got.1, None);
    }

    #[test]
    fn subject_validation_accepts_case_insensitive_match() {
        let body = "Akmon is the focus today.";
        let got = subject_match_start("akmon", body, "", &[]).expect("match");
        assert_eq!(got.1, Some("normalized"));
    }

    #[test]
    fn subject_validation_accepts_kebab_normalization() {
        let body = "Akmon release readiness was discussed by the team.";
        let got = subject_match_start("akmon-release-readiness", body, "", &[]).expect("match");
        assert_eq!(got.1, Some("normalized"));
    }

    #[test]
    fn subject_validation_accepts_note_title_match() {
        let body = "The team discussed next steps.";
        let title = "meeting-akmon-release-readiness";
        let got = subject_match_start("akmon-release-readiness", body, title, &[]).expect("match");
        assert_eq!(got.1, Some("title"));
    }

    #[test]
    fn subject_validation_accepts_wikilink_target() {
        // Body must not contain the literal subject, or case 1 matches first.
        let body = "See the linked note for decision tracking.";
        let got = subject_match_start(
            "akmon-decision-log",
            body,
            "",
            &["akmon-decision-log".to_string()],
        )
        .expect("match");
        assert_eq!(got.1, Some("wikilink"));
        assert_eq!(got.0, 0);
    }

    #[test]
    fn subject_validation_rejects_truly_fabricated() {
        assert!(
            subject_match_start("fabricated-thing", "hello world today", "other-note", &[])
                .is_none()
        );
    }

    #[test]
    fn note_title_stem_strips_markdown_extension() {
        let p = PathBuf::from("meeting-2026-04-08-akmon-release-readiness.md");
        assert_eq!(
            super::note_title_stem(&p),
            "meeting-2026-04-08-akmon-release-readiness"
        );
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
    async fn extractor_recovers_out_of_bounds_span() {
        let body = "Rado works at HMC.";
        let response = r#"[{"subject":"Rado","predicate":"works_at","object":"HMC","span_start":0,"span_end":1000,"valid_from":null,"valid_until":null,"confidence":1.0}]"#;
        let extractor = make_extractor(response);
        let note = make_note("note-2", body, Privacy::Private);

        let claims = extractor
            .extract(&note, body)
            .await
            .expect("extract claims");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].span_start, body.find("Rado").expect("subject"));
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
    async fn extract_accepts_unary_claim_with_null_object() {
        let body = "vault is in early stages for the work.";
        let end = body.len();
        let response = format!(
            r#"[{{"subject":"vault","predicate":"in_early_stages","span_start":0,"span_end":{end},"valid_from":null,"valid_until":null,"confidence":0.9}}]"#
        );
        let extractor = make_extractor(&response);
        let note = make_note("unary-null", body, Privacy::Private);
        let claims = extractor.extract(&note, body).await.expect("extract");
        assert_eq!(claims.len(), 1);
        assert!(claims[0].object.is_none());
    }

    #[tokio::test]
    async fn extract_accepts_unary_claim_with_empty_string_object() {
        let body = "vault is in early stages for the work.";
        let end = body.len();
        let response = format!(
            r#"[{{"subject":"vault","predicate":"in_early_stages","object":"","span_start":0,"span_end":{end},"valid_from":null,"valid_until":null,"confidence":0.9}}]"#
        );
        let extractor = make_extractor(&response);
        let note = make_note("unary-empty", body, Privacy::Private);
        let claims = extractor.extract(&note, body).await.expect("extract");
        assert_eq!(claims.len(), 1);
        assert!(claims[0].object.is_none());
    }

    #[tokio::test]
    async fn extract_recovers_span_when_offsets_invalid() {
        let body = "akmon launched today";
        let response = r#"[{"subject":"akmon","predicate":"launched","span_start":0,"span_end":200,"valid_from":null,"valid_until":null,"confidence":0.9}]"#;
        let extractor = make_extractor(response);
        let note = make_note("recover-span", body, Privacy::Private);
        let claims = extractor.extract(&note, body).await.expect("extract");
        assert_eq!(claims.len(), 1);
        let expected_end = ("akmon".len() + 80).min(body.len());
        assert_eq!(claims[0].span_start, 0);
        assert_eq!(claims[0].span_end, expected_end);
    }

    #[tokio::test]
    async fn extract_drops_claim_when_subject_not_in_body() {
        let body = "hello world today";
        let response = r#"[{"subject":"fabricated-thing","predicate":"exists","object":"x","span_start":0,"span_end":11,"valid_from":null,"valid_until":null,"confidence":0.9}]"#;
        let extractor = make_extractor(response);
        let note = make_note("fabricated", body, Privacy::Private);
        let claims = extractor.extract(&note, body).await.expect("extract");
        assert!(claims.is_empty());
    }

    #[tokio::test]
    async fn extract_accepts_kebab_subject_when_body_has_spaced_phrase() {
        let body = "Akmon release readiness was discussed by the team.";
        let path = PathBuf::from("vault/meeting-2026-04-08-akmon-release-readiness.md");
        let note = make_note_with_path(path, "meet-1", body, vec![]);
        let response = r#"[{"subject":"akmon-release-readiness","predicate":"was_discussed","span_start":0,"span_end":200,"valid_from":null,"valid_until":null,"confidence":0.9}]"#;
        let extractor = make_extractor(response);
        let claims = extractor.extract(&note, body).await.expect("extract");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].subject, "akmon-release-readiness");
    }

    #[tokio::test]
    async fn extract_retries_on_all_invalid_first_attempt() {
        let body = "Rado works at HMC.";
        let bad =
            r#"[{"subject":"ghost","predicate":"nope","object":"x","span_start":0,"span_end":5}]"#;
        let good_start = body.find("Rado works at HMC").expect("span");
        let good_end = good_start + "Rado works at HMC".len();
        let good = format!(
            r#"[{{"subject":"Rado","predicate":"works_at","object":"HMC","span_start":{good_start},"span_end":{good_end},"valid_from":null,"valid_until":null,"confidence":1.0}}]"#
        );
        let extractor = ClaimExtractor {
            llm: Arc::new(MockSequentialExtractorLlm::new(vec![bad.to_string(), good])),
            model_label: "test/retry".to_string(),
        };
        let note = make_note("retry-ok", body, Privacy::Private);
        let claims = extractor.extract(&note, body).await.expect("extract");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].subject, "Rado");
    }

    #[tokio::test]
    async fn extract_does_not_retry_twice() {
        let body = "short.";
        let bad =
            r#"[{"subject":"ghost","predicate":"nope","object":"x","span_start":0,"span_end":2}]"#;
        let extractor = ClaimExtractor {
            llm: Arc::new(MockSequentialExtractorLlm::new(vec![
                bad.to_string(),
                bad.to_string(),
            ])),
            model_label: "test/noretry".to_string(),
        };
        let note = make_note("retry-stop", body, Privacy::Private);
        let claims = extractor.extract(&note, body).await.expect("extract");
        assert!(claims.is_empty());
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
            id: Claim::compute_id("Rado", "works_at", Some("HMC"), &note.fm.id, 0),
            subject: "Rado".to_string(),
            predicate: "works_at".to_string(),
            object: Some("HMC".to_string()),
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
