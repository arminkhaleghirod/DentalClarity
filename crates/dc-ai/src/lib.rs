//! Model providers.
//!
//! The traits are split by capability: text generation and speech synthesis
//! are separate because providers support them differently. Which adapters
//! may be built is decided by [`ProviderPolicy`], derived from the deployment
//! [`Mode`]. Clinical mode cannot construct the OpenRouter adapter at all.

pub mod fake;
pub mod openrouter;
pub mod openrouter_speech;

use async_trait::async_trait;
use dc_core::Mode;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("provider {provider} is not allowed in {mode} mode")]
    NotAllowed { provider: &'static str, mode: Mode },
    #[error("provider request failed: {0}")]
    Request(String),
    #[error("provider returned an unusable response: {0}")]
    BadResponse(String),
    #[error("rate limited or over budget: {0}")]
    Limited(String),
}

pub type Result<T> = std::result::Result<T, AiError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

/// How much hidden reasoning a model may do before answering, for models that support it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextRequest {
    pub model: String,
    pub messages: Vec<Message>,
    /// JSON Schema the response must follow. `None` for free text.
    pub response_schema: Option<serde_json::Value>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub reasoning: Option<ReasoningEffort>,
}

/// App-managed model defaults. Administrators do not pick models; these
/// change only through a ClarityEval-backed decision (docs/DECISIONS.md D4).
pub mod defaults {
    use super::ReasoningEffort;

    /// Demo mode text model on OpenRouter.
    pub const DEMO_TEXT_MODEL: &str = "deepseek/deepseek-v4.1-flash-1017";
    pub const DEMO_TEXT_REASONING: ReasoningEffort = ReasoningEffort::High;
    /// OpenRouter providers tried first for the demo text model, in order.
    /// Chosen by benchmark on 2026-11-26: PrimeIntellect matched the fastest
    /// tiers on real section prompts (about 4 s, about 210 tokens per second)
    /// at a fifth of their cost. Other fast, non-retaining providers are the
    /// fallback when it fails.
    pub const DEMO_PROVIDER_ORDER: &[&str] = &["primeintellect"];
    /// Demo mode speech model on OpenRouter.
    pub const DEMO_SPEECH_MODEL: &str = "heygen/heygen-voice-1-turbo";
    /// Voice for demo narration. The model offers ten preset voices
    /// (nadine, tabitha, bram, gareth, noelle, meredith, sabrina, lucian,
    /// weston, vincent) and no voice descriptions. This default is provisional
    /// until the M9 listening test; `DC_SPEECH_VOICE` overrides it.
    pub const DEMO_SPEECH_VOICE: &str = "meredith";
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// Cost reported by the provider in US dollars, when it reports one.
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextResponse {
    pub model: String,
    /// Upstream provider that served the call, when the router reports it.
    pub route: Option<String>,
    pub content: String,
    pub usage: Usage,
    pub latency_ms: u64,
}

#[async_trait]
pub trait TextModel: Send + Sync {
    fn provider(&self) -> &'static str;
    async fn generate(&self, request: TextRequest) -> Result<TextResponse>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechRequest {
    pub model: String,
    pub text: String,
    /// BCP 47 language tag.
    pub language: String,
    pub voice: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechClip {
    pub mime_type: String,
    pub bytes: Vec<u8>,
    pub duration_ms: Option<u32>,
}

#[async_trait]
pub trait SpeechModel: Send + Sync {
    fn provider(&self) -> &'static str;
    async fn synthesize(&self, request: SpeechRequest) -> Result<SpeechClip>;
}

/// Which providers the running mode permits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderPolicy {
    mode: Mode,
}

impl ProviderPolicy {
    pub fn for_mode(mode: Mode) -> Self {
        Self { mode }
    }

    pub fn allows_openrouter(self) -> bool {
        self.mode == Mode::Demo
    }

    pub fn allows_aws(self) -> bool {
        self.mode == Mode::Clinical
    }

    pub fn check_openrouter(self) -> Result<()> {
        if self.allows_openrouter() {
            Ok(())
        } else {
            Err(AiError::NotAllowed {
                provider: "openrouter",
                mode: self.mode,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clinical_mode_refuses_openrouter() {
        let policy = ProviderPolicy::for_mode(Mode::Clinical);
        assert!(!policy.allows_openrouter());
        assert!(matches!(
            policy.check_openrouter(),
            Err(AiError::NotAllowed { .. })
        ));
        assert!(policy.allows_aws());
    }

    #[test]
    fn demo_mode_uses_openrouter_only() {
        let policy = ProviderPolicy::for_mode(Mode::Demo);
        assert!(policy.check_openrouter().is_ok());
        assert!(!policy.allows_aws());
    }
}
