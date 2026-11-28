//! AI providers for clinical mode (docs/DECISIONS.md D2, D13 to D15).
//!
//! Clinical mode uses the practice's own AWS account: Bedrock for text and
//! Polly for narration. They are built only when the server was compiled
//! with the `aws` feature and `DC_AI_PROVIDER=aws` is set. Otherwise
//! generation answers `503 provider_not_configured` and narration
//! `503 narration_unavailable`, as before. This module never builds the
//! OpenRouter adapter, and `ProviderPolicy` would refuse it if it tried.
//!
//! | Variable                    | Default                              |
//! |-----------------------------|--------------------------------------|
//! | `DC_AI_PROVIDER`            | unset (no provider); `aws`           |
//! | `DC_AWS_REGION`             | `us-west-2`                          |
//! | `DC_AWS_PROFILE`            | unset (default credential chain)     |
//! | `DC_BEDROCK_TEXT_MODEL`     | `anthropic.claude-haiku-5-5`         |
//! | `DC_BEDROCK_REVIEWER_MODEL` | the text model                       |
//! | `DC_BEDROCK_TEMPERATURE`    | `false` (send the request temperature) |
//! | `DC_POLLY_ENGINE`           | `neural`                             |
//! | `DC_POLLY_VOICE`            | `Joanna` (English; see `polly::voice_for`) |
//!
//! Credentials: when the secret store holds `aws-access-key-id` and
//! `aws-secret-access-key` (set with `dentalclarity secret set`), that key is
//! used. Otherwise the AWS SDK's default chain runs.

use crate::generation::Generation;
use crate::narration::Narration;
use crate::secrets::SecretStore;
use anyhow::{Result, bail};
use sqlx::SqlitePool;
use std::path::Path;

/// The clinical providers, when configured.
pub struct Providers {
    pub generation: Generation,
    pub narration: Narration,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// Builds the clinical providers from the environment, or `None` when none
/// is configured.
pub async fn from_env(
    db: &SqlitePool,
    data_dir: &Path,
    secrets: &SecretStore,
) -> Result<Option<Providers>> {
    match var("DC_AI_PROVIDER")
        .map(|v| v.to_ascii_lowercase())
        .as_deref()
    {
        None => {
            tracing::info!(
                "no AI provider configured for clinical mode (set DC_AI_PROVIDER=aws); generation and narration are disabled"
            );
            Ok(None)
        }
        Some("aws") => build_aws(db, data_dir, secrets).await,
        Some(other) => bail!("DC_AI_PROVIDER must be aws in clinical mode, not {other:?}"),
    }
}

#[cfg(not(feature = "aws"))]
async fn build_aws(_: &SqlitePool, _: &Path, _: &SecretStore) -> Result<Option<Providers>> {
    tracing::error!(
        "DC_AI_PROVIDER=aws, but this build has no AWS support (build with --features aws); generation and narration are disabled"
    );
    Ok(None)
}

#[cfg(feature = "aws")]
async fn build_aws(
    db: &SqlitePool,
    data_dir: &Path,
    secrets: &SecretStore,
) -> Result<Option<Providers>> {
    use crate::secrets::names;
    use anyhow::Context;
    use dc_ai::aws::{AwsSettings, StaticKey};
    use dc_ai::bedrock::Bedrock;
    use dc_ai::polly::{self, Polly};
    use dc_ai::{ProviderPolicy, SpeechModel, TextModel, defaults};
    use dc_core::Mode;
    use std::sync::Arc;

    let static_key = match (
        secrets.get_string(names::AWS_ACCESS_KEY_ID)?,
        secrets.get_string(names::AWS_SECRET_ACCESS_KEY)?,
    ) {
        (Some(access_key_id), Some(secret_access_key)) => {
            if !crate::secrets::PROTECTED {
                tracing::warn!("the AWS access key is stored without DPAPI on this system");
            }
            Some(StaticKey {
                access_key_id,
                secret_access_key,
            })
        }
        (None, None) => None,
        _ => bail!("the secret store holds only half of the AWS access key; set both parts"),
    };
    let settings = AwsSettings {
        region: var("DC_AWS_REGION"),
        profile: var("DC_AWS_PROFILE"),
        static_key,
    };
    let config = dc_ai::aws::load(&settings).await?;
    let policy = ProviderPolicy::for_mode(Mode::Clinical);

    let send_temperature = matches!(
        var("DC_BEDROCK_TEMPERATURE").as_deref(),
        Some("1" | "true" | "yes")
    );
    let text: Arc<dyn TextModel> =
        Arc::new(Bedrock::new(policy, &config)?.with_temperature(send_temperature));
    let writer_model =
        var("DC_BEDROCK_TEXT_MODEL").unwrap_or_else(|| defaults::CLINICAL_TEXT_MODEL.into());
    let reviewer_model = var("DC_BEDROCK_REVIEWER_MODEL").unwrap_or_else(|| writer_model.clone());
    let generation = Generation::new(
        db.clone(),
        None,
        text.clone(),
        writer_model.clone(),
        text,
        reviewer_model.clone(),
        None,
    );

    let engine = var("DC_POLLY_ENGINE").unwrap_or_else(|| defaults::CLINICAL_SPEECH_ENGINE.into());
    let voice = var("DC_POLLY_VOICE").unwrap_or_else(|| defaults::CLINICAL_SPEECH_VOICE.into());
    let audio_dir = data_dir.join("artifacts").join("audio");
    std::fs::create_dir_all(&audio_dir)
        .with_context(|| format!("creating {}", audio_dir.display()))?;
    let speech: Arc<dyn SpeechModel> = Arc::new(Polly::new(policy, &config)?);
    let narration = Narration::new(
        Mode::Clinical,
        db.clone(),
        audio_dir,
        Some(speech),
        engine.clone(),
        voice.clone(),
        generation.budget.clone(),
    )
    .with_price(polly::usd_per_million_chars(&engine));

    tracing::info!(
        region = %settings.region(),
        writer = %writer_model,
        reviewer = %reviewer_model,
        engine = %engine,
        voice = %voice,
        static_key = settings.static_key.is_some(),
        "clinical AI providers enabled (Bedrock and Polly)"
    );
    Ok(Some(Providers {
        generation,
        narration,
    }))
}
