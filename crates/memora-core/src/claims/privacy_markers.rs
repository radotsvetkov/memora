use regex::Regex;

use crate::note::Privacy;

#[derive(Debug, Clone)]
struct MarkerToken {
    start: usize,
    end: usize,
    kind: MarkerKind,
}

#[derive(Debug, Clone, Copy)]
enum MarkerKind {
    Open(Privacy),
    Close,
}

#[derive(Debug, Clone)]
struct OpenMarker {
    content_start: usize,
    level: Privacy,
}

pub fn parse_privacy_spans(body: &str) -> Vec<(usize, usize, Privacy)> {
    let re = Regex::new(r"<!--\s*privacy:(public|private|secret)\s*-->|<!--\s*/privacy\s*-->")
        .expect("privacy marker regex should compile");
    let mut tokens = Vec::new();
    for capture in re.captures_iter(body) {
        let Some(matched) = capture.get(0) else {
            continue;
        };
        let kind = if let Some(level_match) = capture.get(1) {
            let level = match level_match.as_str() {
                "public" => Privacy::Public,
                "private" => Privacy::Private,
                "secret" => Privacy::Secret,
                _ => continue,
            };
            MarkerKind::Open(level)
        } else {
            MarkerKind::Close
        };
        tokens.push(MarkerToken {
            start: matched.start(),
            end: matched.end(),
            kind,
        });
    }

    // Nested markers (e.g. a `secret` block inside a `private` block) are
    // legitimate, not an error: both spans are kept, and `privacy_for_span`'s
    // max-over-overlaps composition makes the innermost (most restrictive)
    // level win wherever spans overlap. Dropping either span here would
    // silently strip protection from real content, so this must fail closed.
    let mut stack: Vec<OpenMarker> = Vec::new();
    let mut spans = Vec::new();
    for token in tokens {
        match token.kind {
            MarkerKind::Open(level) => {
                if !stack.is_empty() {
                    tracing::warn!(
                        position = token.start,
                        "nested privacy marker detected; both levels will be enforced"
                    );
                }
                stack.push(OpenMarker {
                    content_start: token.end,
                    level,
                });
            }
            MarkerKind::Close => {
                let Some(open) = stack.pop() else {
                    tracing::warn!(
                        position = token.start,
                        "unmatched closing privacy marker detected"
                    );
                    continue;
                };
                if token.start >= open.content_start {
                    spans.push((open.content_start, token.start, open.level));
                }
            }
        }
    }

    if !stack.is_empty() {
        tracing::warn!("unmatched opening privacy marker detected");
    }

    spans
}

pub fn privacy_for_span(
    span_start: usize,
    span_end: usize,
    marker_spans: &[(usize, usize, Privacy)],
    note_privacy: Privacy,
) -> Privacy {
    let mut level = note_privacy;
    for (marker_start, marker_end, marker_level) in marker_spans {
        let overlaps = span_start < *marker_end && span_end > *marker_start;
        if overlaps {
            level = level.max(*marker_level);
        }
    }
    level
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_privacy_spans_returns_content_only() {
        let body = "abc <!--privacy:secret-->salary 95k<!--/privacy--> xyz";
        let spans = parse_privacy_spans(body);
        assert_eq!(spans.len(), 1);
        let (start, end, privacy) = spans[0];
        assert_eq!(privacy, Privacy::Secret);
        assert_eq!(&body[start..end], "salary 95k");
    }

    #[test]
    fn parse_privacy_spans_keeps_both_levels_for_nested_markers() {
        let body = "<!--privacy:private-->outer <!--privacy:secret-->inner<!--/privacy--> text<!--/privacy-->";
        let spans = parse_privacy_spans(body);
        assert_eq!(spans.len(), 2);

        let inner = spans
            .iter()
            .find(|(_, _, level)| *level == Privacy::Secret)
            .expect("inner secret span present");
        assert_eq!(&body[inner.0..inner.1], "inner");

        let outer = spans
            .iter()
            .find(|(_, _, level)| *level == Privacy::Private)
            .expect("outer private span present");
        assert_eq!(
            &body[outer.0..outer.1],
            "outer <!--privacy:secret-->inner<!--/privacy--> text"
        );

        // Content inside the inner marker must resolve to the more restrictive
        // Secret level, even though it also falls inside the outer Private span.
        let inner_start = body.find("inner").unwrap();
        let level = privacy_for_span(inner_start, inner_start + 5, &spans, Privacy::Public);
        assert_eq!(level, Privacy::Secret);
    }

    #[test]
    fn parse_privacy_spans_skips_unmatched_markers() {
        let body = "text <!--privacy:secret-->no close";
        let spans = parse_privacy_spans(body);
        assert!(spans.is_empty());
    }

    #[test]
    fn privacy_for_span_uses_max_level_when_overlapping() {
        let markers = vec![(10, 20, Privacy::Public), (12, 16, Privacy::Secret)];
        let level = privacy_for_span(14, 18, &markers, Privacy::Private);
        assert_eq!(level, Privacy::Secret);
    }

    #[test]
    fn privacy_for_span_returns_note_level_without_overlap() {
        let markers = vec![(10, 20, Privacy::Secret)];
        let level = privacy_for_span(21, 25, &markers, Privacy::Private);
        assert_eq!(level, Privacy::Private);
    }
}
