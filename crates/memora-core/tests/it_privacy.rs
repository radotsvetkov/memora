use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use memora_core::claims::ClaimExtractor;
use memora_core::note::{Frontmatter, Note, NoteSource, Privacy};
use memora_core::privacy::PrivacyFilter;
use memora_llm::{
    CompletionRequest, CompletionResponse, LlmClient, LlmDestination, LlmError, LlmProvider,
};

struct MockExtractorLlm {
    canned_response: String,
}

#[async_trait]
impl LlmClient for MockExtractorLlm {
    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(CompletionResponse {
            text: self.canned_response.clone(),
            model: "mock/privacy".to_string(),
            input_tokens: None,
            output_tokens: None,
        })
    }

    fn model_name(&self) -> &str {
        "mock/privacy"
    }

    fn destination(&self) -> LlmDestination {
        LlmDestination::Local
    }
}

fn make_note(id: &str, body: &str) -> Note {
    Note {
        path: PathBuf::from(format!("vault/{id}.md")),
        fm: Frontmatter {
            id: id.to_string(),
            region: "test/privacy".to_string(),
            source: NoteSource::Personal,
            privacy: Privacy::Private,
            created: Utc
                .with_ymd_and_hms(2026, 4, 1, 0, 0, 0)
                .single()
                .expect("valid created date"),
            updated: Utc
                .with_ymd_and_hms(2026, 4, 2, 0, 0, 0)
                .single()
                .expect("valid updated date"),
            summary: "privacy test".to_string(),
            tags: Vec::new(),
            refs: Vec::new(),
        },
        body: body.to_string(),
        wikilinks: Vec::new(),
    }
}

fn make_claim_extractor(canned_response: String) -> ClaimExtractor {
    ClaimExtractor {
        llm: Arc::new(MockExtractorLlm { canned_response }),
        model_label: "test/privacy".to_string(),
    }
}

#[tokio::test]
async fn cloud_filter_redacts_secret_claim_from_inline_marker() -> Result<()> {
    let body = "Comp <!--privacy:secret-->salary 95000<!--/privacy-->";
    let start = body.find("salary 95000").expect("salary start");
    let end = start + "salary 95000".len();
    let extractor = make_claim_extractor(format!(
        r#"[{{"s":"Comp","p":"has_salary","o":"95000","span_start":{start},"span_end":{end},"valid_from":null,"valid_until":null,"confidence":1.0}}]"#
    ));
    let note = make_note("secret-note", body);
    let claims = extractor.extract(&note, body).await?;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].privacy, Privacy::Secret);

    let filter = PrivacyFilter::new_for(LlmProvider::OpenAi);
    let (redacted, stats) = filter.filter(&claims);
    assert_eq!(stats.redacted, 1);
    assert_eq!(redacted.len(), 1);
    assert!(redacted[0].redacted);
    assert_eq!(redacted[0].subject, "Comp");
    assert_eq!(redacted[0].predicate, "[redacted]");
    assert_eq!(redacted[0].object, "[redacted]");
    Ok(())
}

#[tokio::test]
async fn local_filter_keeps_secret_claim_unredacted() -> Result<()> {
    let body = "Comp <!--privacy:secret-->salary 95000<!--/privacy-->";
    let start = body.find("salary 95000").expect("salary start");
    let end = start + "salary 95000".len();
    let extractor = make_claim_extractor(format!(
        r#"[{{"s":"Comp","p":"has_salary","o":"95000","span_start":{start},"span_end":{end},"valid_from":null,"valid_until":null,"confidence":1.0}}]"#
    ));
    let note = make_note("secret-note-local", body);
    let claims = extractor.extract(&note, body).await?;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].privacy, Privacy::Secret);

    let filter = PrivacyFilter::new_for(LlmProvider::Ollama);
    let (redacted, stats) = filter.filter(&claims);
    assert_eq!(stats.redacted, 0);
    assert_eq!(stats.passed, 1);
    assert_eq!(redacted.len(), 1);
    assert!(!redacted[0].redacted);
    assert_eq!(redacted[0].subject, "Comp");
    assert_eq!(redacted[0].predicate, "has_salary");
    assert_eq!(redacted[0].object, "95000");
    Ok(())
}
