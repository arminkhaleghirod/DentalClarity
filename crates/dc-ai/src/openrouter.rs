//! OpenRouter adapter for demo mode.
//!
//! OpenRouter exposes an OpenAI-compatible chat completions endpoint. Only
//! synthetic data may be sent through it; [`OpenRouter::new`] refuses to build
//! the adapter outside demo mode. Speech synthesis through
//! `heygen/heygen-voice-1-turbo` is added in milestone M5.

use crate::{AiError, ProviderPolicy, Result, TextModel, TextRequest, TextResponse, Usage};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};

const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

pub struct OpenRouter {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl std::fmt::Debug for OpenRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenRouter")
            .field("base_url", &self.base_url)
            .field("api_key", &"[redacted]")
            .finish()
    }
}

impl OpenRouter {
    pub fn new(policy: ProviderPolicy, api_key: String) -> Result<Self> {
        policy.check_openrouter()?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| AiError::Request(e.to_string()))?;
        Ok(Self {
            client,
            base_url: DEFAULT_BASE_URL.to_owned(),
            api_key,
        })
    }

    /// Points the adapter at a different endpoint, for tests against a local stub.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    model: Option<String>,
    choices: Vec<Choice>,
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChoiceMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    cost: Option<f64>,
}

#[async_trait]
impl TextModel for OpenRouter {
    fn provider(&self) -> &'static str {
        "openrouter"
    }

    async fn generate(&self, request: TextRequest) -> Result<TextResponse> {
        let mut body = json!({
            "model": request.model,
            "messages": request.messages,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "usage": { "include": true },
        });
        if let Some(effort) = request.reasoning {
            body["reasoning"] = json!({ "effort": effort });
        }
        if let Some(schema) = &request.response_schema {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": { "name": "output", "strict": true, "schema": schema },
            });
        }

        let started = Instant::now();
        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::Request(e.without_url().to_string()))?;

        let status = response.status();
        if status.as_u16() == 429 || status.as_u16() == 402 {
            return Err(AiError::Limited(format!("HTTP {status}")));
        }
        if !status.is_success() {
            return Err(AiError::Request(format!("HTTP {status}")));
        }

        let parsed: ChatResponse = response
            .json()
            .await
            .map_err(|e| AiError::BadResponse(e.to_string()))?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| AiError::BadResponse("no message content".into()))?;
        let usage = parsed.usage.map_or_else(Usage::default, |u| Usage {
            input_tokens: u.prompt_tokens.unwrap_or(0),
            output_tokens: u.completion_tokens.unwrap_or(0),
            cost_usd: u.cost,
        });

        Ok(TextResponse {
            model: parsed.model.unwrap_or(request.model),
            content,
            usage,
            latency_ms: started.elapsed().as_millis() as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_core::Mode;

    #[test]
    fn cannot_be_built_in_clinical_mode() {
        let result = OpenRouter::new(ProviderPolicy::for_mode(Mode::Clinical), "key".into());
        assert!(matches!(result, Err(AiError::NotAllowed { .. })));
    }

    #[test]
    fn debug_output_hides_the_key() {
        let adapter =
            OpenRouter::new(ProviderPolicy::for_mode(Mode::Demo), "sk-secret".into()).unwrap();
        assert!(!format!("{adapter:?}").contains("sk-secret"));
    }
}
