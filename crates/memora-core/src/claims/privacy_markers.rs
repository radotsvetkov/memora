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
    nested: bool,
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

    let mut stack: Vec<OpenMarker> = Vec::new();
    let mut spans = Vec::new();
    for token in tokens {
        match token.kind {
            MarkerKind::Open(level) => {
                let has_parent = !stack.is_empty();
                if !stack.is_empty() {
                    tracing::warn!(
                        position = token.start,
                        "nested privacy marker detected; skipping nested block"
                    );
                    for marker in &mut stack {
                        marker.nested = true;
                    }
                }
                stack.push(OpenMarker {
                    content_start: token.end,
                    level,
                    nested: has_parent,
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
                if open.nested {
                    continue;
                }
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
    fn parse_privacy_spans_skips_nested_markers() {
        let body = "<!--privacy:private-->outer <!--privacy:secret-->inner<!--/privacy--> text<!--/privacy-->";
        let spans = parse_privacy_spans(body);
        assert!(spans.is_empty());
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
