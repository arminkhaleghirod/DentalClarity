//! Shared AWS setup for the clinical providers ([`crate::bedrock`] and
//! [`crate::polly`]).
//!
//! Credentials come from one of two places. When the caller passes a static
//! access key (read from the DPAPI-protected secret store on the practice
//! server), it is used as is. Otherwise the SDK's default chain runs:
//! environment variables, the shared config and credentials files
//! (including IAM Identity Center profiles), and instance or container
//! roles. SDK retries are turned off, so the callers' own retry rules
//! (docs/DECISIONS.md D12) apply to every AWS call the same way they apply
//! to OpenRouter calls.

use crate::{AiError, Result};
use aws_config::retry::RetryConfig;
use aws_config::{BehaviorVersion, Region, SdkConfig};
use aws_sdk_bedrockruntime::config::Credentials;

/// Where the AWS adapters connect and how they sign in.
#[derive(Clone, Default)]
pub struct AwsSettings {
    /// Region, such as `us-west-2`. `None` uses
    /// [`crate::defaults::CLINICAL_REGION`].
    pub region: Option<String>,
    /// Named profile from the shared AWS config, for development machines.
    pub profile: Option<String>,
    /// Static access key, for a practice server without another credential
    /// source.
    pub static_key: Option<StaticKey>,
}

impl std::fmt::Debug for AwsSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsSettings")
            .field("region", &self.region)
            .field("profile", &self.profile)
            .field(
                "static_key",
                &self.static_key.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

/// An IAM access key. Never logged.
#[derive(Clone)]
pub struct StaticKey {
    pub access_key_id: String,
    pub secret_access_key: String,
}

impl AwsSettings {
    /// The region these settings resolve to.
    pub fn region(&self) -> String {
        self.region
            .clone()
            .unwrap_or_else(|| crate::defaults::CLINICAL_REGION.to_owned())
    }
}

/// Loads the SDK configuration. This does not call AWS; credentials are
/// resolved on the first request.
pub async fn load(settings: &AwsSettings) -> Result<SdkConfig> {
    let mut loader = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(settings.region()))
        .retry_config(RetryConfig::disabled());
    if let Some(profile) = &settings.profile {
        loader = loader.profile_name(profile);
    }
    if let Some(key) = &settings.static_key {
        if key.access_key_id.trim().is_empty() || key.secret_access_key.trim().is_empty() {
            return Err(AiError::Request(
                "the stored AWS access key is empty".into(),
            ));
        }
        loader = loader.credentials_provider(Credentials::new(
            key.access_key_id.trim(),
            key.secret_access_key.trim(),
            None,
            None,
            "dentalclarity-secret-store",
        ));
    }
    Ok(loader.load().await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_hides_the_key() {
        let settings = AwsSettings {
            region: None,
            profile: None,
            static_key: Some(StaticKey {
                access_key_id: "AKIAEXAMPLE".into(),
                secret_access_key: "very-secret".into(),
            }),
        };
        let shown = format!("{settings:?}");
        assert!(!shown.contains("very-secret") && !shown.contains("AKIAEXAMPLE"));
        assert_eq!(settings.region(), "us-west-2");
    }
}
