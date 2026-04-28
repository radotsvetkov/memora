use std::collections::HashSet;

use super::parser::parse_claim_markers;
use super::validator::CitationCheck;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CitationStatus {
    Verified,
    Unverified,
    FingerprintMismatch,
    QuoteMismatch,
}

#[derive(Debug, Clone)]
pub struct CitedAnswer {
    pub raw_text: String,
    pub clean_text: String,
    pub checks: Vec<CitationCheck>,
    pub verified_count: usize,
    pub unverified_count: usize,
    pub mismatch_count: usize,
    pub redacted_count: usize,
    pub degraded: bool,
}

pub fn rewrite_with_only_verified(text: &str, verified: &HashSet<String>) -> String {
    let sentence_bounds = split_sentence_bounds(text);
    let markers = parse_claim_markers(text);
    let mut keep = vec![true; sentence_bounds.len()];

    for (start, _, claim_id) in markers {
        if verified.contains(&claim_id) {
            continue;
        }
        if let Some((idx, _)) =
            sentence_bounds
                .iter()
                .enumerate()
                .find(|(_, (sentence_start, sentence_end))| {
                    start >= *sentence_start && start < *sentence_end
                })
        {
            keep[idx] = false;
        }
    }

    let mut rebuilt = String::new();
    for (idx, (start, end)) in sentence_bounds.iter().enumerate() {
        if keep[idx] {
            rebuilt.push_str(&text[*start..*end]);
        }
    }

    rebuilt
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn split_sentence_bounds(text: &str) -> Vec<(usize, usize)> {
    let mut bounds = Vec::new();
    let mut start = 0usize;
    let mut idx = 0usize;
    while idx < text.len() {
        let rem = &text[idx..];
        let boundary_len = if rem.starts_with(". ")
            || rem.starts_with("! ")
            || rem.starts_with("? ")
            || rem.starts_with("\n\n")
        {
            Some(2)
        } else {
            None
        };

        if let Some(len) = boundary_len {
            let end = idx + len;
            if end > start {
                bounds.push((start, end));
            }
            start = end;
            idx = end;
        } else {
            idx += 1;
        }
    }
    if start < text.len() {
        bounds.push((start, text.len()));
    }
    if bounds.is_empty() && !text.is_empty() {
        bounds.push((0, text.len()));
    }
    bounds
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::rewrite_with_only_verified;

    #[test]
    fn rewrite_drops_sentences_with_unverified_markers() {
        let text = "Good sentence [claim:aaaaaaaaaaaaaaaa]. Bad sentence [claim:bbbbbbbbbbbbbbbb].";
        let verified = HashSet::from(["aaaaaaaaaaaaaaaa".to_string()]);
        let cleaned = rewrite_with_only_verified(text, &verified);
        assert!(cleaned.contains("aaaaaaaaaaaaaaaa"));
        assert!(!cleaned.contains("bbbbbbbbbbbbbbbb"));
        assert!(!cleaned.contains("Bad sentence"));
    }
}
