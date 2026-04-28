use async_trait::async_trait;
use memora_llm::{CompletionRequest, CompletionResponse, LlmClient, LlmDestination, LlmError};

pub struct MockExtractorLlm {
    pub canned_response: String,
}

#[async_trait]
impl LlmClient for MockExtractorLlm {
    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(CompletionResponse {
            text: self.canned_response.clone(),
            model: "mock/extractor".to_string(),
            input_tokens: None,
            output_tokens: None,
        })
    }

    fn model_name(&self) -> &str {
        "mock/extractor"
    }

    fn destination(&self) -> LlmDestination {
        LlmDestination::Local
    }
}
