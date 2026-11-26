//! OpenRouter adapter for demo mode.
//!
//! OpenRouter exposes an OpenAI-compatible chat completions endpoint. Only
//! synthetic data may be sent through it; [`OpenRouter::new`] refuses to build
//! the adapter outside demo mode. Speech synthesis lives in
//! [`crate::openrouter_speech`].

use crate::{AiError, ProviderPolicy, Result, TextModel, TextRequest, TextResponse, Usage};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};

const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
/// Per-call timeout for calls without hidden reasoning.
pub const TIMEOUT: Duration = Duration::from_secs(45);
/// Per-call timeout for calls with hidden reasoning, which can think for a
/// minute or more before the reply starts.
pub const REASONING_TIMEOUT: Duration = Duration::from_secs(120);

pub struct OpenRouter {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    routing: serde_json::Value,
    /// Sampling seed sent with every request, when set. The app leaves it unset.
    seed: Option<u64>,
    /// Replaces both default timeouts when set.
    timeout: Option<Duration>,
    /// Benchmark-only request adjustments; the app uses the defaults.
    tweaks: Tweaks,
}

/// Adjustments ClarityEval makes to the request body. The app never sets them.
#[derive(Debug, Clone, Default)]
struct Tweaks {
    /// Model id sent upstream instead of the request's (a route suffix removed).
    model: Option<String>,
    /// Leave `temperature` out, for models whose endpoints do not accept it.
    omit_temperature: bool,
    /// OpenRouter `service_tier`, such as `flex`.
    service_tier: Option<String>,
}

/// Provider routing sent with every text request.
///
/// OpenRouter's default picks the cheapest provider, which for our models
/// means preemptible and 4-bit tiers that stall on long reasoning calls.
/// This policy tries the preferred providers first, falls back to others on
/// errors, sorts fallbacks by throughput, and only uses providers that
/// support every parameter we send, do not store or train on prompts, and
/// run 8-bit or better weights.
pub fn default_routing(order: &[&str]) -> serde_json::Value {
    json!({
        "order": order,
        "allow_fallbacks": true,
        "sort": "throughput",
        "require_parameters": true,
        "data_collection": "deny",
        "quantizations": ["fp8", "bf16", "fp16", "unknown"],
    })
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
        // The timeout is set per request; see [`OpenRouter::timeout_for`].
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| AiError::Request(e.to_string()))?;
        Ok(Self {
            client,
            base_url: DEFAULT_BASE_URL.to_owned(),
            api_key,
            routing: default_routing(crate::defaults::DEMO_PROVIDER_ORDER),
            seed: None,
            timeout: None,
            tweaks: Tweaks::default(),
        })
    }

    /// Sends `model` upstream in place of the request's model id.
    pub fn with_upstream_model(mut self, model: impl Into<String>) -> Self {
        self.tweaks.model = Some(model.into());
        self
    }

    /// Leaves `temperature` out of every request. With `require_parameters`
    /// routing, sending it to a model whose endpoints ignore it finds no
    /// endpoint at all.
    pub fn without_temperature(mut self, omit: bool) -> Self {
        self.tweaks.omit_temperature = omit;
        self
    }

    /// Asks for an OpenRouter service tier (`flex` or `priority`).
    pub fn with_service_tier(mut self, tier: Option<String>) -> Self {
        self.tweaks.service_tier = tier;
        self
    }

    /// Sends `seed` with every request. With `require_parameters` routing this
    /// limits the call to providers that accept a seed, so callers decide
    /// per model whether to set it. Used by ClarityEval; the app never sets it.
    pub fn with_seed(mut self, seed: Option<u64>) -> Self {
        self.seed = seed;
        self
    }

    /// Replaces the per-request timeout ([`TIMEOUT`], or [`REASONING_TIMEOUT`]
    /// for reasoning calls). Used by ClarityEval for single calls that write
    /// much more than one section.
    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self> {
        self.timeout = Some(timeout);
        Ok(self)
    }

    /// The timeout for one request.
    pub fn timeout_for(&self, request: &TextRequest) -> Duration {
        self.timeout.unwrap_or(if request.reasoning.is_some() {
            REASONING_TIMEOUT
        } else {
            TIMEOUT
        })
    }

    /// Replaces the provider routing policy.
    pub fn with_routing(mut self, routing: serde_json::Value) -> Self {
        self.routing = routing;
        self
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
    provider: Option<String>,
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
        let body = self.request_body(&request);
        let timeout = self.timeout_for(&request);
        // A timed-out call may still be billed, so it is reported as a
        // response that came back unusable, not as a failed request.
        let timed_out = || AiError::BadResponse(format!("no reply within {} s", timeout.as_secs()));
        let started = Instant::now();
        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .timeout(timeout)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    timed_out()
                } else {
                    AiError::Request(e.without_url().to_string())
                }
            })?;

        let status = response.status();
        if status.as_u16() == 429 || status.as_u16() == 402 {
            return Err(AiError::Limited(format!("HTTP {status}")));
        }
        if !status.is_success() {
            return Err(AiError::Request(format!("HTTP {status}")));
        }

        let parsed: serde_json::Value = response.json().await.map_err(|e| {
            if e.is_timeout() {
                timed_out()
            } else {
                AiError::BadResponse(e.without_url().to_string())
            }
        })?;
        parse_reply(parsed, &request.model, started.elapsed().as_millis() as u64)
    }
}

impl OpenRouter {
    /// The chat completions body for `request`, as [`TextModel::generate`]
    /// sends it. Public so a batch client can send the identical body.
    pub fn request_body(&self, request: &TextRequest) -> serde_json::Value {
        let mut body = json!({
            "model": self.tweaks.model.as_deref().unwrap_or(&request.model),
            "messages": request.messages,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "usage": { "include": true },
            "provider": self.routing,
        });
        if self.tweaks.omit_temperature
            && let Some(o) = body.as_object_mut()
        {
            o.remove("temperature");
        }
        if let Some(tier) = &self.tweaks.service_tier {
            body["service_tier"] = json!(tier);
        }
        if let Some(seed) = self.seed {
            body["seed"] = json!(seed);
        }
        if let Some(effort) = request.reasoning {
            body["reasoning"] = json!({ "effort": effort });
        }
        if let Some(schema) = &request.response_schema {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": { "name": "output", "strict": true, "schema": schema },
            });
        }
        body
    }
}

/// Reads a chat completions reply. `requested` is the model id used when the
/// reply does not name one.
pub fn parse_reply(
    reply: serde_json::Value,
    requested: &str,
    latency_ms: u64,
) -> Result<TextResponse> {
    let parsed: ChatResponse =
        serde_json::from_value(reply).map_err(|e| AiError::BadResponse(e.to_string()))?;
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
        model: parsed.model.unwrap_or_else(|| requested.to_owned()),
        route: parsed.provider,
        content,
        usage,
        latency_ms,
    })
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
    fn reasoning_calls_get_a_longer_timeout() {
        let adapter = OpenRouter::new(ProviderPolicy::for_mode(Mode::Demo), "key".into()).unwrap();
        let mut request = TextRequest {
            model: "m".into(),
            messages: vec![],
            response_schema: None,
            max_tokens: 10,
            temperature: 0.0,
            reasoning: None,
        };
        assert_eq!(adapter.timeout_for(&request), TIMEOUT);
        request.reasoning = Some(crate::ReasoningEffort::High);
        assert_eq!(adapter.timeout_for(&request), REASONING_TIMEOUT);
        let adapter = adapter.with_timeout(Duration::from_secs(240)).unwrap();
        assert_eq!(adapter.timeout_for(&request), Duration::from_secs(240));
    }

    #[test]
    fn tweaks_change_only_the_benchmark_body() {
        let request = TextRequest {
            model: "a/b:flex".into(),
            messages: vec![],
            response_schema: None,
            max_tokens: 10,
            temperature: 0.2,
            reasoning: None,
        };
        let plain = OpenRouter::new(ProviderPolicy::for_mode(Mode::Demo), "k".into()).unwrap();
        let body = plain.request_body(&request);
        assert_eq!(body["model"], "a/b:flex");
        assert!(body.get("temperature").is_some() && body.get("service_tier").is_none());
        let tweaked = OpenRouter::new(ProviderPolicy::for_mode(Mode::Demo), "k".into())
            .unwrap()
            .with_upstream_model("a/b")
            .without_temperature(true)
            .with_service_tier(Some("flex".into()));
        let body = tweaked.request_body(&request);
        assert_eq!(body["model"], "a/b");
        assert!(body.get("temperature").is_none());
        assert_eq!(body["service_tier"], "flex");
    }

    #[test]
    fn debug_output_hides_the_key() {
        let adapter =
            OpenRouter::new(ProviderPolicy::for_mode(Mode::Demo), "sk-secret".into()).unwrap();
        assert!(!format!("{adapter:?}").contains("sk-secret"));
    }
}
