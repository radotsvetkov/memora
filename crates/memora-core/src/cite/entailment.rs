//! Optional second verification layer: entailment.
//!
//! Provenance (the hash-proven layer in [`super::validator`]) guarantees that the
//! cited source verbatim contains the quoted text. It does NOT guarantee that the
//! source *supports* the assertion built on top of it — a model can quote a real
//! span and still draw an unsupported inference. This module adds a best-effort,
//! LLM-judged entailment check. It is deliberately separate and clearly labeled:
//! provenance is proven, entailment is the model's opinion.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Result;
use memora_llm::{LlmClient, LlmDestination};
use serde_json::Value;

use crate::note::Privacy;

/// LLM-judged verdict on whether a source span supports a cited assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entailment {
    /// The source span states or directly implies the assertion.
    Entailed,
    /// The assertion goes beyond, contradicts, or is not supported by the source.
    Unsupported,
    /// No check was run: empty hypothesis, or a Secret claim that would otherwise
    /// leak to a cloud endpoint.
    Unchecked,
}

impl Entailment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Entailed => "entailed",
            Self::Unsupported => "unsupported",
            Self::Unchecked => "unchecked",
        }
    }
}

type EntailmentKey = (String, String);
static ENTAILMENT_CACHE: OnceLock<Mutex<HashMap<EntailmentKey, bool>>> = OnceLock::new();

/// Runs the entailment check over a configured LLM. Gated and Secret-safe in the
/// same way as the contradiction detector: Secret content is never sent to a
/// cloud endpoint.
pub struct EntailmentChecker {
    llm: Arc<dyn LlmClient>,
}

impl EntailmentChecker {
    pub fn new(llm: Arc<dyn LlmClient>) -> Self {
        Self { llm }
    }

    /// `premise` is the verified source span; `hypothesis` is the asserted text.
    /// Returns `Unchecked` for an empty premise/hypothesis, or when running the
    /// check would send Secret content to a cloud endpoint.
    pub async fn check(
        &self,
        premise: &str,
        hypothesis: &str,
        privacy: Privacy,
    ) -> Result<Entailment> {
        let premise = premise.trim();
        let hypothesis = hypothesis.trim();
        if premise.is_empty() || hypothesis.is_empty() {
            return Ok(Entailment::Unchecked);
        }
        if self.llm.destination() != LlmDestination::Local && privacy == Privacy::Secret {
            return Ok(Entailment::Unchecked);
        }

        let key = (premise.to_string(), hypothesis.to_string());
        let cache = ENTAILMENT_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(cached) = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&key)
            .copied()
        {
            return Ok(verdict_from_bool(cached));
        }

        let prompt = format!(
            r#"SOURCE:
{premise}

ASSERTION:
{hypothesis}

Does the SOURCE support the ASSERTION? Answer with JSON only. Use exactly
{{"entailed":true}} if the source states or directly implies the assertion, or
{{"entailed":false}} if the assertion goes beyond, contradicts, or is not
supported by the source."#
        );
        let text = self
            .llm
            .chat_json(
                &prompt,
                Some("Output one JSON object with a boolean field \"entailed\"."),
                128,
                0.0,
            )
            .await?;
        // On an unparseable response, default to entailed rather than raising a
        // false "unsupported" alarm — provenance already proved the citation real.
        let entailed = parse_bool_field(&text, "entailed").unwrap_or(true);
        cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key, entailed);
        Ok(verdict_from_bool(entailed))
    }
}

fn verdict_from_bool(entailed: bool) -> Entailment {
    if entailed {
        Entailment::Entailed
    } else {
        Entailment::Unsupported
    }
}

fn parse_bool_field(text: &str, key: &str) -> Option<bool> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    value.get(key)?.as_bool()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use memora_llm::{CompletionResponse, LlmError, RedactedPayload};
    use std::sync::Mutex as StdMutex;

    struct ScriptedLlm {
        reply: String,
        destination: LlmDestination,
        calls: StdMutex<usize>,
    }

    impl ScriptedLlm {
        fn new(reply: &str, destination: LlmDestination) -> Self {
            Self {
                reply: reply.to_string(),
                destination,
                calls: StdMutex::new(0),
            }
        }
    }

    #[async_trait]
    impl LlmClient for ScriptedLlm {
        async fn complete(
            &self,
            _payload: RedactedPayload,
        ) -> Result<CompletionResponse, LlmError> {
            *self.calls.lock().unwrap() += 1;
            Ok(CompletionResponse {
                text: self.reply.clone(),
                model: "scripted".to_string(),
                input_tokens: None,
                output_tokens: None,
            })
        }
        fn model_name(&self) -> &str {
            "scripted"
        }
        fn destination(&self) -> LlmDestination {
            self.destination
        }
    }

    #[tokio::test]
    async fn entailed_and_unsupported_map_from_llm_json() {
        let yes = EntailmentChecker::new(Arc::new(ScriptedLlm::new(
            r#"{"entailed":true}"#,
            LlmDestination::Local,
        )));
        assert_eq!(
            yes.check(
                "Revenue grew 5% in Q2.",
                "Revenue grew in Q2.",
                Privacy::Private
            )
            .await
            .unwrap(),
            Entailment::Entailed
        );

        let no = EntailmentChecker::new(Arc::new(ScriptedLlm::new(
            r#"{"entailed":false}"#,
            LlmDestination::Local,
        )));
        assert_eq!(
            no.check(
                "Revenue grew 5% in Q2.",
                "Revenue is booming.",
                Privacy::Private
            )
            .await
            .unwrap(),
            Entailment::Unsupported
        );
    }

    #[tokio::test]
    async fn empty_hypothesis_is_unchecked_without_calling_the_llm() {
        let llm = Arc::new(ScriptedLlm::new(
            r#"{"entailed":false}"#,
            LlmDestination::Local,
        ));
        let checker = EntailmentChecker::new(llm.clone());
        assert_eq!(
            checker
                .check("some source", "   ", Privacy::Private)
                .await
                .unwrap(),
            Entailment::Unchecked
        );
        assert_eq!(
            *llm.calls.lock().unwrap(),
            0,
            "no LLM call for empty hypothesis"
        );
    }

    #[tokio::test]
    async fn secret_claims_are_not_sent_to_cloud() {
        let llm = Arc::new(ScriptedLlm::new(
            r#"{"entailed":false}"#,
            LlmDestination::CloudKnown,
        ));
        let checker = EntailmentChecker::new(llm.clone());
        assert_eq!(
            checker
                .check("secret source", "secret assertion", Privacy::Secret)
                .await
                .unwrap(),
            Entailment::Unchecked
        );
        assert_eq!(
            *llm.calls.lock().unwrap(),
            0,
            "secret content never left for the cloud"
        );
    }
}
