use memora_llm::LlmDestination;

use crate::claims::privacy_markers::parse_privacy_spans;
use crate::note::Privacy;

/// Redact secret inline spans in place, preserving byte length so LLM span offsets stay valid.
pub fn redact_secret_spans_preserving_length(
    body: &str,
    marker_spans: &[(usize, usize, Privacy)],
) -> String {
    let mut bytes = body.as_bytes().to_vec();
    for (start, end, level) in marker_spans {
        if *level != Privacy::Secret {
            continue;
        }
        if *end > bytes.len() || *start >= *end {
            continue;
        }
        for byte in &mut bytes[*start..*end] {
            *byte = b'X';
        }
    }
    // SAFETY: we only substitute ASCII `X` at existing UTF-8 boundaries.
    String::from_utf8(bytes).unwrap_or_else(|_| body.to_string())
}

/// Prepare note body for an LLM extraction call, respecting destination and note privacy.
pub fn body_for_llm_extraction(
    body: &str,
    note_privacy: Privacy,
    destination: LlmDestination,
    redact_secret_in_cloud: bool,
) -> Option<String> {
    if !redact_secret_in_cloud || destination == LlmDestination::Local {
        return Some(body.to_string());
    }
    if note_privacy == Privacy::Secret {
        return None;
    }
    let marker_spans = parse_privacy_spans(body);
    Some(redact_secret_spans_preserving_length(body, &marker_spans))
}

/// Redact note body for MCP/CLI display. Secret notes return fully redacted text.
pub fn redact_body_for_display(body: &str, note_privacy: Privacy) -> (String, bool) {
    if note_privacy == Privacy::Secret {
        return ("[redacted]".to_string(), true);
    }
    let marker_spans = parse_privacy_spans(body);
    let has_secret = marker_spans
        .iter()
        .any(|(_, _, level)| *level == Privacy::Secret);
    if !has_secret {
        return (body.to_string(), false);
    }
    (
        redact_secret_spans_preserving_length(body, &marker_spans),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_byte_length_when_redacting_inline_secret() {
        let body = "Comp <!--privacy:secret-->salary 95000<!--/privacy-->";
        let spans = parse_privacy_spans(body);
        let redacted = redact_secret_spans_preserving_length(body, &spans);
        assert_eq!(redacted.len(), body.len());
        assert!(!redacted.contains("95000"));
    }

    #[test]
    fn secret_note_body_fully_redacted_for_display() {
        let (body, redacted) = redact_body_for_display("top secret", Privacy::Secret);
        assert!(redacted);
        assert_eq!(body, "[redacted]");
    }
}
