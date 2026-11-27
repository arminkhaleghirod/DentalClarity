//! The model wrapper `clarityeval retry` puts around every call (DECISIONS E21).
//!
//! A rerun exists to replace infrastructure failures, so a transport error
//! (rate limit, timeout, unreachable provider, empty reply) is waited out
//! with long backoff instead of ending the section. A reply that stopped at
//! the output limit before any answer is the model's own result: it is passed
//! on as an empty answer, which the pipeline and the baseline then fail like
//! any other unusable output.

use async_trait::async_trait;
use dc_ai::openrouter::OUTPUT_LIMIT_NO_ANSWER;
use dc_ai::{AiError, Result, TextModel, TextRequest, TextResponse, Usage};
use std::sync::Arc;
use std::time::Duration;

/// Attempts per call before the error is passed on.
pub const ATTEMPTS: u32 = 12;
/// First wait; it doubles after each failure up to [`MAX_WAIT`].
const FIRST_WAIT: Duration = Duration::from_secs(20);
const MAX_WAIT: Duration = Duration::from_secs(600);

pub struct Persistent {
    inner: Arc<dyn TextModel>,
}

impl Persistent {
    pub fn new(inner: Arc<dyn TextModel>) -> Self {
        Self { inner }
    }
}

/// Whether `err` says the model ran out of output before answering.
pub fn cut_off(err: &AiError) -> bool {
    matches!(err, AiError::BadResponse(m) if m.contains(OUTPUT_LIMIT_NO_ANSWER))
}

fn wait(attempt: u32) -> Duration {
    FIRST_WAIT
        .saturating_mul(2u32.saturating_pow(attempt))
        .min(MAX_WAIT)
}

#[async_trait]
impl TextModel for Persistent {
    fn provider(&self) -> &'static str {
        self.inner.provider()
    }

    async fn generate(&self, request: TextRequest) -> Result<TextResponse> {
        let mut attempt = 0;
        loop {
            match self.inner.generate(request.clone()).await {
                Ok(r) => return Ok(r),
                Err(e @ AiError::NotAllowed { .. }) => return Err(e),
                Err(e) if cut_off(&e) => {
                    return Ok(TextResponse {
                        model: request.model.clone(),
                        route: None,
                        content: String::new(),
                        usage: Usage::default(),
                        latency_ms: 0,
                    });
                }
                Err(e) if attempt + 1 >= ATTEMPTS => return Err(e),
                Err(e) => {
                    eprintln!(
                        "retry: {} attempt {} failed ({e}); waiting {} s",
                        request.model,
                        attempt + 1,
                        wait(attempt).as_secs()
                    );
                    tokio::time::sleep(wait(attempt)).await;
                    attempt += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Script(Mutex<Vec<Result<TextResponse>>>);

    #[async_trait]
    impl TextModel for Script {
        fn provider(&self) -> &'static str {
            "script"
        }
        async fn generate(&self, _: TextRequest) -> Result<TextResponse> {
            self.0.lock().unwrap().remove(0)
        }
    }

    fn request() -> TextRequest {
        TextRequest {
            model: "m".into(),
            messages: vec![],
            response_schema: None,
            max_tokens: 1,
            temperature: 0.0,
            reasoning: None,
        }
    }

    #[test]
    fn waits_grow_to_a_cap() {
        assert_eq!(wait(0), FIRST_WAIT);
        assert_eq!(wait(1), FIRST_WAIT * 2);
        assert_eq!(wait(20), MAX_WAIT);
    }

    #[tokio::test]
    async fn a_cut_off_reply_becomes_an_empty_answer() {
        let script = Script(Mutex::new(vec![Err(AiError::BadResponse(
            OUTPUT_LIMIT_NO_ANSWER.into(),
        ))]));
        let p = Persistent::new(Arc::new(script));
        let r = p.generate(request()).await.unwrap();
        assert!(r.content.is_empty());
    }

    #[tokio::test]
    async fn refusals_by_policy_are_not_retried() {
        let script = Script(Mutex::new(vec![Err(AiError::NotAllowed {
            provider: "openrouter",
            mode: dc_core::Mode::Clinical,
        })]));
        let p = Persistent::new(Arc::new(script));
        assert!(p.generate(request()).await.is_err());
    }
}
