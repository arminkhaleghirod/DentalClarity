//! Live smoke test of the clinical providers with synthetic text: one Bedrock
//! call with structured output and one Polly sentence. It creates no AWS
//! resources and sends no patient data.
//!
//! ```sh
//! aws sso login --profile <profile>
//! AWS_PROFILE=<profile> cargo run -p dc-ai --features aws --example aws_smoke
//! ```
//!
//! `DC_AWS_REGION`, `DC_BEDROCK_TEXT_MODEL`, `DC_POLLY_ENGINE` and
//! `DC_POLLY_VOICE` override the defaults. The WAV is written to the system
//! temp directory.

use dc_ai::aws::{self, AwsSettings};
use dc_ai::bedrock::Bedrock;
use dc_ai::polly::{self, Polly};
use dc_ai::{
    Message, ProviderPolicy, Role, SpeechModel, SpeechRequest, TextModel, TextRequest, defaults,
};
use dc_core::Mode;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    let settings = AwsSettings {
        region: var("DC_AWS_REGION"),
        profile: var("AWS_PROFILE"),
        static_key: None,
    };
    let config = aws::load(&settings).await?;
    let policy = ProviderPolicy::for_mode(Mode::Clinical);

    let model =
        var("DC_BEDROCK_TEXT_MODEL").unwrap_or_else(|| defaults::CLINICAL_TEXT_MODEL.into());
    let bedrock = Bedrock::new(policy, &config)?;
    let request = TextRequest {
        model: model.clone(),
        messages: vec![
            Message {
                role: Role::System,
                content: "You explain dental procedures in plain language for patients.".into(),
            },
            Message {
                role: Role::User,
                content: "In one short sentence, say what a dental crown is. This is a test \
                          with no patient data."
                    .into(),
            },
        ],
        response_schema: Some(json!({
            "type": "object",
            "properties": { "sentence": { "type": "string" } },
            "required": ["sentence"],
            "additionalProperties": false,
        })),
        max_tokens: 2000,
        temperature: 0.2,
        reasoning: None,
    };
    let reply = bedrock.generate(request).await?;
    println!(
        "bedrock {model}: {} ms, {} input and {} output tokens",
        reply.latency_ms, reply.usage.input_tokens, reply.usage.output_tokens
    );
    println!("  reply: {}", reply.content);

    let engine = var("DC_POLLY_ENGINE").unwrap_or_else(|| defaults::CLINICAL_SPEECH_ENGINE.into());
    let voice = var("DC_POLLY_VOICE").unwrap_or_else(|| defaults::CLINICAL_SPEECH_VOICE.into());
    let text = "A crown is a cap that covers a tooth to protect it.";
    let clip = Polly::new(policy, &config)?
        .synthesize(SpeechRequest {
            model: engine.clone(),
            text: text.into(),
            language: "en-US".into(),
            voice: voice.clone(),
        })
        .await?;
    let path = std::env::temp_dir().join("dentalclarity-polly-smoke.wav");
    std::fs::write(&path, &clip.bytes)?;
    let cost = text.chars().count() as f64 * polly::usd_per_million_chars(&engine) / 1e6;
    println!(
        "polly {engine}/{voice}: {} bytes, {:?} ms of audio, about ${cost:.6}; saved to {}",
        clip.bytes.len(),
        clip.duration_ms,
        path.display()
    );
    Ok(())
}
