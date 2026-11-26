//! OpenRouter speech adapter for demo mode.
//!
//! `POST /api/v1/audio/speech` takes `model`, `input`, `voice` and
//! `response_format` and answers with raw audio bytes. For
//! `heygen/heygen-voice-1-turbo` the only format offered is `pcm`: 16-bit
//! little-endian samples, described by the response header
//! `Content-Type: audio/pcm;rate=44100;channels=1` (verified 2026-11-26).
//! Browsers cannot play bare PCM, so the adapter wraps it in a WAV header.
//! MP3 or WAV responses, if a future model returns them, pass through as is.
//!
//! The response carries no cost figure; callers price calls by characters.
//! Like the text adapter, [`OpenRouterSpeech::new`] refuses to build outside
//! demo mode.

use crate::{AiError, ProviderPolicy, Result, SpeechClip, SpeechModel, SpeechRequest};
use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
/// A 400 character sentence is about 30 s of 44.1 kHz mono PCM, about
/// 2.6 MB. Anything far larger is not a sentence clip.
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
/// Per-call timeout. A sentence normally takes 1 to 2 s.
const TIMEOUT: Duration = Duration::from_secs(30);
/// Lowest and highest PCM sample rate accepted from the provider, in Hz.
pub const MIN_SAMPLE_RATE: u32 = 8_000;
pub const MAX_SAMPLE_RATE: u32 = 48_000;
/// Most PCM channels accepted from the provider (stereo).
pub const MAX_CHANNELS: u16 = 2;

pub struct OpenRouterSpeech {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl std::fmt::Debug for OpenRouterSpeech {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenRouterSpeech")
            .field("base_url", &self.base_url)
            .field("api_key", &"[redacted]")
            .finish()
    }
}

impl OpenRouterSpeech {
    pub fn new(policy: ProviderPolicy, api_key: String) -> Result<Self> {
        policy.check_openrouter()?;
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
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

/// Request body for one sentence. Only the model, the sentence text and the
/// voice are sent. The language is not a parameter of the endpoint; the model
/// reads it from the text.
pub fn speech_body(request: &SpeechRequest) -> serde_json::Value {
    json!({
        "model": request.model,
        "input": request.text,
        "voice": request.voice,
        "response_format": "pcm",
        "provider": { "data_collection": "deny" },
    })
}

/// Whether `rate` and `channels` describe PCM this server will handle:
/// 8 to 48 kHz, mono or stereo. Anything else from the provider is refused
/// rather than trusted in sample arithmetic.
pub fn supported_pcm(rate: u32, channels: u16) -> bool {
    (MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&rate) && (1..=MAX_CHANNELS).contains(&channels)
}

/// Sample rate and channel count from a `audio/pcm;rate=..;channels=..`
/// content type. Missing parameters fall back to 44.1 kHz mono, the format
/// the endpoint documents for HeyGen. A parameter that does not parse, or a
/// format outside [`supported_pcm`], gives `None`.
fn pcm_format(content_type: &str) -> Option<(u32, u16)> {
    let mut rate = 44_100;
    let mut channels = 1;
    for param in content_type.split(';').skip(1) {
        if let Some((name, value)) = param.split_once('=') {
            match name.trim() {
                "rate" => rate = value.trim().parse().ok()?,
                "channels" => channels = value.trim().parse().ok()?,
                _ => {}
            }
        }
    }
    supported_pcm(rate, channels).then_some((rate, channels))
}

/// Wraps 16-bit little-endian PCM samples in a RIFF/WAVE header. `None` when
/// the format is outside [`supported_pcm`] or the data does not fit a WAV.
pub fn pcm_to_wav(pcm: &[u8], sample_rate: u32, channels: u16) -> Option<Vec<u8>> {
    if !supported_pcm(sample_rate, channels) {
        return None;
    }
    let data_len = u32::try_from(pcm.len()).ok()?;
    let riff_len = data_len.checked_add(36)?;
    let block_align = channels.checked_mul(2)?;
    let byte_rate = sample_rate.checked_mul(u32::from(block_align))?;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_len.to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm);
    Some(out)
}

/// Turns the response body into a playable clip.
fn into_clip(content_type: &str, body: Vec<u8>) -> Result<SpeechClip> {
    if body.is_empty() {
        return Err(AiError::BadResponse("empty audio".into()));
    }
    let base = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "audio/pcm" | "audio/l16" => {
            let bad_format = || AiError::BadResponse("unsupported PCM format".into());
            let (rate, channels) = pcm_format(content_type).ok_or_else(bad_format)?;
            let frames = body.len() as u64 / (u64::from(channels) * 2);
            let duration_ms = frames
                .checked_mul(1000)
                .map(|ms| ms / u64::from(rate))
                .and_then(|ms| u32::try_from(ms).ok());
            Ok(SpeechClip {
                mime_type: "audio/wav".into(),
                bytes: pcm_to_wav(&body, rate, channels).ok_or_else(bad_format)?,
                duration_ms,
            })
        }
        "audio/wav" | "audio/x-wav" | "audio/wave" => Ok(SpeechClip {
            mime_type: "audio/wav".into(),
            bytes: body,
            duration_ms: None,
        }),
        "audio/mpeg" | "audio/mp3" => Ok(SpeechClip {
            mime_type: "audio/mpeg".into(),
            bytes: body,
            duration_ms: None,
        }),
        other => Err(AiError::BadResponse(format!(
            "unexpected content type {other:?}"
        ))),
    }
}

#[async_trait]
impl SpeechModel for OpenRouterSpeech {
    fn provider(&self) -> &'static str {
        "openrouter"
    }

    async fn synthesize(&self, request: SpeechRequest) -> Result<SpeechClip> {
        // A call that timed out may still be billed, so it is reported as a
        // reply that came back unusable, not as a failed request (D12). The
        // caller counts it against the budget and retries it at most once.
        let timed_out = || AiError::BadResponse(format!("no reply within {} s", TIMEOUT.as_secs()));
        let response = self
            .client
            .post(format!("{}/audio/speech", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&speech_body(&request))
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
        if status.is_server_error() {
            return Err(AiError::Request(format!("HTTP {status}")));
        }
        if !status.is_success() {
            // A 4xx is about the request or the account; retrying will not help.
            return Err(AiError::BadResponse(format!("HTTP {status}")));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        if response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
        {
            return Err(AiError::BadResponse("audio too large".into()));
        }
        // The provider has answered by now, so a body that cannot be read
        // is an unusable reply that may have been billed.
        let body = response.bytes().await.map_err(|e| {
            if e.is_timeout() {
                timed_out()
            } else {
                AiError::BadResponse(e.without_url().to_string())
            }
        })?;
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(AiError::BadResponse("audio too large".into()));
        }
        into_clip(&content_type, body.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_core::Mode;

    fn request() -> SpeechRequest {
        SpeechRequest {
            model: "heygen/heygen-voice-1-turbo".into(),
            text: "A crown covers the tooth.".into(),
            language: "en".into(),
            voice: "meredith".into(),
        }
    }

    #[test]
    fn body_sends_only_model_text_voice_and_format() {
        let body = speech_body(&request());
        assert_eq!(
            body,
            json!({
                "model": "heygen/heygen-voice-1-turbo",
                "input": "A crown covers the tooth.",
                "voice": "meredith",
                "response_format": "pcm",
                "provider": { "data_collection": "deny" },
            })
        );
    }

    #[test]
    fn cannot_be_built_in_clinical_mode() {
        let result = OpenRouterSpeech::new(ProviderPolicy::for_mode(Mode::Clinical), "key".into());
        assert!(matches!(result, Err(AiError::NotAllowed { .. })));
    }

    #[test]
    fn debug_output_hides_the_key() {
        let adapter =
            OpenRouterSpeech::new(ProviderPolicy::for_mode(Mode::Demo), "sk-secret".into())
                .unwrap();
        assert!(!format!("{adapter:?}").contains("sk-secret"));
    }

    #[test]
    fn pcm_is_wrapped_as_wav_with_duration() {
        // One second of 8 kHz mono silence.
        let pcm = vec![0u8; 16_000];
        let clip = into_clip("audio/pcm;rate=8000;channels=1", pcm).unwrap();
        assert_eq!(clip.mime_type, "audio/wav");
        assert_eq!(clip.duration_ms, Some(1000));
        assert_eq!(&clip.bytes[0..4], b"RIFF");
        assert_eq!(&clip.bytes[8..16], b"WAVEfmt ");
        assert_eq!(
            u32::from_le_bytes(clip.bytes[24..28].try_into().unwrap()),
            8000
        );
        assert_eq!(
            u32::from_le_bytes(clip.bytes[40..44].try_into().unwrap()),
            16_000
        );
        assert_eq!(clip.bytes.len(), 44 + 16_000);
    }

    #[test]
    fn pcm_format_defaults_and_other_types() {
        assert_eq!(pcm_format("audio/pcm"), Some((44_100, 1)));
        assert_eq!(
            pcm_format("audio/pcm; rate=24000; channels=2"),
            Some((24_000, 2))
        );
        let mp3 = into_clip("audio/mpeg", vec![0xFF, 0xFB, 0x90]).unwrap();
        assert_eq!(mp3.mime_type, "audio/mpeg");
        assert!(into_clip("application/json", vec![b'{']).is_err());
        assert!(into_clip("audio/pcm", Vec::new()).is_err());
    }

    #[test]
    fn pcm_outside_the_supported_range_is_refused() {
        for content_type in [
            "audio/pcm;rate=0",
            "audio/pcm;rate=7999",
            "audio/pcm;rate=48001",
            "audio/pcm;rate=4294967295",
            "audio/pcm;rate=fast",
            "audio/pcm;channels=0",
            "audio/pcm;channels=3",
            "audio/pcm;channels=65535",
            "audio/pcm;channels=70000",
        ] {
            assert_eq!(pcm_format(content_type), None, "{content_type}");
            let err = into_clip(content_type, vec![0; 64]).unwrap_err();
            assert!(matches!(err, AiError::BadResponse(_)), "{content_type}");
        }
        assert_eq!(pcm_format("audio/pcm;rate=8000"), Some((8_000, 1)));
        assert_eq!(
            pcm_format("audio/pcm;rate=48000;channels=2"),
            Some((48_000, 2))
        );
        assert!(pcm_to_wav(&[0; 4], 96_000, 1).is_none());
        assert!(pcm_to_wav(&[0; 4], 44_100, 40_000).is_none());
        assert!(pcm_to_wav(&[0; 4], 44_100, 2).is_some());
    }
}
