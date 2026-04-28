use std::sync::OnceLock;

use regex::Regex;

pub const CLAIM_RE: &str = r"\[claim:([a-f0-9]{16})\]";

fn claim_regex() -> &'static Regex {
    static CLAIM_REGEX: OnceLock<Regex> = OnceLock::new();
    CLAIM_REGEX.get_or_init(|| match Regex::new(CLAIM_RE) {
        Ok(regex) => regex,
        Err(err) => panic!("invalid claim marker regex: {err}"),
    })
}

pub fn parse_claim_markers(text: &str) -> Vec<(usize, usize, String)> {
    claim_regex()
        .captures_iter(text)
        .filter_map(|captures| {
            let full = captures.get(0)?;
            let id = captures.get(1)?;
            Some((full.start(), full.end(), id.as_str().to_string()))
        })
        .collect()
}

pub fn extract_quote_before(text: &str, marker_pos: usize) -> Option<String> {
    if marker_pos == 0 || marker_pos > text.len() {
        return None;
    }
    let prefix = text.get(..marker_pos)?;
    let sentence_start = nearest_sentence_start(prefix);
    let sentence = prefix.get(sentence_start..)?.trim();
    if sentence.is_empty() {
        return None;
    }

    find_double_quoted(sentence)
        .or_else(|| find_single_quoted(sentence))
        .or_else(|| find_markdown_blockquote(sentence))
}

fn nearest_sentence_start(prefix: &str) -> usize {
    let mut best = 0usize;
    for boundary in [". ", "! ", "? ", "\n\n"] {
        if let Some(idx) = prefix.rfind(boundary) {
            best = best.max(idx + boundary.len());
        }
    }
    best
}

fn find_double_quoted(sentence: &str) -> Option<String> {
    let start = sentence.find('"')?;
    let rest = sentence.get(start + 1..)?;
    let end = rest.find('"')?;
    let candidate = rest.get(..end)?.trim();
    if candidate.is_empty() {
        None
    } else {
        Some(candidate.to_string())
    }
}

fn find_single_quoted(sentence: &str) -> Option<String> {
    let start = sentence.find('\'')?;
    let rest = sentence.get(start + 1..)?;
    let end = rest.find('\'')?;
    let candidate = rest.get(..end)?.trim();
    if candidate.is_empty() {
        None
    } else {
        Some(candidate.to_string())
    }
}

fn find_markdown_blockquote(sentence: &str) -> Option<String> {
    let start = sentence.rfind('>')?;
    let rest = sentence.get(start + 1..)?;
    let end = rest.find('\n').unwrap_or(rest.len());
    let candidate = rest.get(..end)?.trim();
    if candidate.is_empty() {
        None
    } else {
        Some(candidate.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_quote_before, parse_claim_markers};

    #[test]
    fn parse_claim_markers_extracts_offsets_and_ids() {
        let text = "A [claim:0123456789abcdef] and [claim:0011223344556677]";
        let markers = parse_claim_markers(text);
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].2, "0123456789abcdef");
        assert_eq!(markers[1].2, "0011223344556677");
        assert_eq!(
            &text[markers[0].0..markers[0].1],
            "[claim:0123456789abcdef]"
        );
    }

    #[test]
    fn extract_quote_before_finds_double_quoted_text() {
        let text = "It says \"Rado works at HMC\" [claim:0123456789abcdef]";
        let pos = text.find("[claim:").expect("marker present");
        let quote = extract_quote_before(text, pos).expect("quote extracted");
        assert_eq!(quote, "Rado works at HMC");
    }

    #[test]
    fn extract_quote_before_finds_single_quoted_text() {
        let text = "Claimed as 'Memora is native Rust' [claim:0123456789abcdef]";
        let pos = text.find("[claim:").expect("marker present");
        let quote = extract_quote_before(text, pos).expect("quote extracted");
        assert_eq!(quote, "Memora is native Rust");
    }

    #[test]
    fn extract_quote_before_finds_blockquote_text() {
        let text = "Context:\n> HMC hosted INTERNORGA\n[claim:0123456789abcdef]";
        let pos = text.find("[claim:").expect("marker present");
        let quote = extract_quote_before(text, pos).expect("quote extracted");
        assert_eq!(quote, "HMC hosted INTERNORGA");
    }
}
