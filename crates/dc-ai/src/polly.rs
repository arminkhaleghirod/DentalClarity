//! Amazon Polly speech adapter for clinical mode.
//!
//! Each sentence is synthesized as 16 kHz, 16-bit mono PCM, the highest rate
//! Polly offers for PCM, and wrapped in a WAV header with
//! [`crate::openrouter_speech::pcm_to_wav`]. The narration cache merges WAV
//! clips by joining their samples, so Polly clips need no other conversion.
//!
//! [`SpeechRequest::model`] names the Polly engine (`neural`, `standard`,
//! `generative` or `long-form`). Polly voices belong to one language, so the
//! configured voice is used for English and [`voice_for`] picks a voice for
//! other languages (DECISIONS D16). [`Polly::new`] refuses to build the
//! adapter outside clinical mode.

use crate::openrouter_speech::pcm_to_wav;
use crate::{AiError, ProviderPolicy, Result, SpeechClip, SpeechModel, SpeechRequest};
use async_trait::async_trait;
use aws_sdk_polly::Client;
use aws_sdk_polly::error::SdkError;
use aws_sdk_polly::operation::synthesize_speech::SynthesizeSpeechError;
use aws_sdk_polly::types::{Engine, OutputFormat, TextType, VoiceId};
use std::time::Duration;

/// PCM sample rate requested from Polly, in Hz.
pub const SAMPLE_RATE: u32 = 16_000;
/// Per-call timeout, the same as for OpenRouter speech (E19).
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// Polly list prices in US dollars per million characters, by engine. Not
/// yet confirmed against the current price list; confirm them before
/// relying on the recorded spend.
pub fn usd_per_million_chars(engine: &str) -> f64 {
    match engine {
        "standard" => 4.0,
        "generative" => 30.0,
        "long-form" => 100.0,
        _ => 16.0,
    }
}

/// Neural voices for languages other than English, by primary language
/// subtag. A language missing here cannot be narrated in clinical mode.
const VOICES: &[(&str, &str, &str)] = &[
    // (language subtag, Polly language code, voice)
    ("es", "es-US", "Lupe"),
    ("fr", "fr-FR", "Lea"),
    ("de", "de-DE", "Vicki"),
    ("pt", "pt-BR", "Camila"),
    ("it", "it-IT", "Bianca"),
    ("zh", "cmn-CN", "Zhiyu"),
    ("ja", "ja-JP", "Kazuha"),
    ("ko", "ko-KR", "Seoyeon"),
    ("hi", "hi-IN", "Kajal"),
    ("ar", "ar-AE", "Hala"),
];

/// The voice for `language` (a BCP 47 tag): `configured` for English,
/// otherwise the table's voice. `None` when Polly has no voice for it here.
pub fn voice_for<'a>(language: &str, configured: &'a str) -> Option<&'a str> {
    let primary = language
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if primary == "en" {
        return Some(configured);
    }
    VOICES
        .iter()
        .find(|(subtag, _, _)| *subtag == primary)
        .map(|(_, _, voice)| *voice)
}

pub struct Polly {
    client: Client,
}

impl std::fmt::Debug for Polly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Polly").finish()
    }
}

impl Polly {
    pub fn new(policy: ProviderPolicy, config: &aws_config::SdkConfig) -> Result<Self> {
        policy.check_aws()?;
        Ok(Self {
            client: Client::new(config),
        })
    }
}

/// Turns Polly's PCM body into a WAV clip with its duration.
pub fn pcm_clip(pcm: &[u8]) -> Result<SpeechClip> {
    if pcm.is_empty() {
        return Err(AiError::BadResponse("empty audio".into()));
    }
    let frames = pcm.len() as u64 / 2;
    let duration_ms = u32::try_from(frames * 1000 / u64::from(SAMPLE_RATE)).ok();
    let bytes = pcm_to_wav(pcm, SAMPLE_RATE, 1)
        .ok_or_else(|| AiError::BadResponse("audio too long for a WAV clip".into()))?;
    Ok(SpeechClip {
        mime_type: "audio/wav".into(),
        bytes,
        duration_ms,
    })
}

fn map_error(err: SdkError<SynthesizeSpeechError>) -> AiError {
    let service = match err {
        SdkError::ServiceError(e) => e.into_err(),
        SdkError::TimeoutError(_) => {
            return AiError::BadResponse("the Polly call timed out".into());
        }
        SdkError::ResponseError(_) => {
            return AiError::BadResponse("Polly sent a reply that could not be read".into());
        }
        other => return AiError::Request(format!("Polly request failed: {other}")),
    };
    let code = service.meta().code().unwrap_or("unknown").to_owned();
    if code.contains("Throttl") {
        return AiError::Limited(code);
    }
    AiError::Request(format!(
        "{code}: {}",
        service.meta().message().unwrap_or("")
    ))
}

#[async_trait]
impl SpeechModel for Polly {
    fn provider(&self) -> &'static str {
        "polly"
    }

    async fn synthesize(&self, request: SpeechRequest) -> Result<SpeechClip> {
        let voice = voice_for(&request.language, &request.voice).ok_or_else(|| {
            AiError::Request(format!(
                "no Polly voice is configured for language {}",
                request.language
            ))
        })?;
        let call = async {
            let output = self
                .client
                .synthesize_speech()
                .engine(Engine::from(request.model.as_str()))
                .voice_id(VoiceId::from(voice))
                .output_format(OutputFormat::Pcm)
                .sample_rate(SAMPLE_RATE.to_string())
                .text_type(TextType::Text)
                .text(&request.text)
                .send()
                .await
                .map_err(map_error)?;
            let body = output
                .audio_stream
                .collect()
                .await
                .map_err(|e| AiError::BadResponse(format!("reading Polly audio: {e}")))?;
            pcm_clip(&body.into_bytes())
        };
        // A call that timed out may still be billed (D12, E19).
        tokio::time::timeout(TIMEOUT, call)
            .await
            .map_err(|_| AiError::BadResponse(format!("no reply within {} s", TIMEOUT.as_secs())))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_core::Mode;

    #[test]
    fn picks_a_voice_per_language() {
        assert_eq!(voice_for("en-US", "Joanna"), Some("Joanna"));
        assert_eq!(voice_for("en", "Matthew"), Some("Matthew"));
        assert_eq!(voice_for("es-MX", "Joanna"), Some("Lupe"));
        assert_eq!(voice_for("ZH_Hans", "Joanna"), Some("Zhiyu"));
        assert_eq!(voice_for("vi", "Joanna"), None);
    }

    #[test]
    fn wraps_pcm_as_wav() {
        // One second of silence.
        let clip = pcm_clip(&vec![0u8; 32_000]).unwrap();
        assert_eq!(clip.mime_type, "audio/wav");
        assert_eq!(clip.duration_ms, Some(1000));
        assert_eq!(&clip.bytes[..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(clip.bytes[24..28].try_into().unwrap()),
            SAMPLE_RATE
        );
        assert_eq!(clip.bytes.len(), 44 + 32_000);
        assert!(matches!(pcm_clip(&[]), Err(AiError::BadResponse(_))));
    }

    #[test]
    fn prices_follow_the_engine() {
        assert_eq!(usd_per_million_chars("neural"), 16.0);
        assert_eq!(usd_per_million_chars("standard"), 4.0);
    }

    #[test]
    fn cannot_be_built_in_demo_mode() {
        let config = aws_config::SdkConfig::builder()
            .behavior_version(aws_config::BehaviorVersion::latest())
            .build();
        assert!(matches!(
            Polly::new(ProviderPolicy::for_mode(Mode::Demo), &config),
            Err(AiError::NotAllowed { .. })
        ));
    }
}
