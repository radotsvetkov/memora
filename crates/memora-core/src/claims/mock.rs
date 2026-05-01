use std::sync::Mutex;

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

/// Invokes `complete` by dequeuing strings from `responses` (FIFO). Used for retry tests.
pub struct MockSequentialExtractorLlm {
    pub responses: Mutex<Vec<String>>,
}

impl MockSequentialExtractorLlm {
    pub fn new(responses: Vec<String>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }
}

#[async_trait]
impl LlmClient for MockSequentialExtractorLlm {
    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let mut guard = self.responses.lock().unwrap_or_else(|e| e.into_inner());
        let text = guard.remove(0);
        Ok(CompletionResponse {
            text,
            model: "mock/sequential".to_string(),
            input_tokens: None,
            output_tokens: None,
        })
    }

    fn model_name(&self) -> &str {
        "mock/sequential"
    }

    fn destination(&self) -> LlmDestination {
        LlmDestination::Local
    }
}
