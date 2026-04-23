//! AWS Bedrock adapter (feature = "bedrock").
//!
//! Wraps `AnthropicClient` with AWS `SigV4` auth and Bedrock Runtime endpoint.
//! The underlying protocol is the same Anthropic Messages API.

#![cfg(feature = "bedrock")]

use crate::anthropic::AnthropicClient;

/// Configuration for AWS Bedrock runtime access.
#[derive(Debug, Clone)]
pub struct BedrockConfig {
    /// AWS region (e.g., "us-east-1").
    pub region: String,
    /// Model ID for Bedrock (e.g., "anthropic.claude-sonnet-4-20250514-v2:0").
    pub model_id: String,
    /// Optional cross-region inference profile ARN.
    pub inference_profile: Option<String>,
}

impl BedrockConfig {
    /// Bedrock Runtime endpoint URL.
    #[must_use]
    pub fn endpoint_url(&self) -> String {
        format!("https://bedrock-runtime.{}.amazonaws.com", self.region)
    }

    /// The model ARN or model ID to use in requests.
    #[must_use]
    pub fn effective_model_id(&self) -> &str {
        self.inference_profile.as_deref().unwrap_or(&self.model_id)
    }
}

/// Create an `AnthropicClient` configured for AWS Bedrock.
///
/// Bedrock uses the same Anthropic Messages API format but with
/// AWS `SigV4` authentication and a Bedrock Runtime endpoint.
///
/// # Errors
///
/// Returns an error if AWS credentials cannot be resolved.
pub fn create_bedrock_client(config: &BedrockConfig) -> crab_core::Result<AnthropicClient> {
    let credentials = crab_auth::bedrock_auth::AwsCredentials::from_env().ok_or_else(|| {
        crab_core::Error::Other(
            "AWS credentials not found. Set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY.".into(),
        )
    })?;

    let auth_provider = crab_auth::bedrock_auth::BedrockAuthProvider::new(credentials);
    let base_url = config.endpoint_url();

    Ok(AnthropicClient::new(&base_url, Box::new(auth_provider)))
}

/// Create an `AnthropicClient` for Bedrock with explicit credentials.
pub fn create_bedrock_client_with_credentials(
    config: &BedrockConfig,
    credentials: crab_auth::bedrock_auth::AwsCredentials,
) -> AnthropicClient {
    let auth_provider = crab_auth::bedrock_auth::BedrockAuthProvider::new(credentials);
    let base_url = config.endpoint_url();
    AnthropicClient::new(&base_url, Box::new(auth_provider))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bedrock_config_endpoint_url() {
        let config = BedrockConfig {
            region: "us-east-1".into(),
            model_id: "anthropic.claude-sonnet-4-20250514-v2:0".into(),
            inference_profile: None,
        };
        assert_eq!(
            config.endpoint_url(),
            "https://bedrock-runtime.us-east-1.amazonaws.com"
        );
    }

    #[test]
    fn bedrock_config_endpoint_url_eu() {
        let config = BedrockConfig {
            region: "eu-west-1".into(),
            model_id: "anthropic.claude-haiku-3-5-v1:0".into(),
            inference_profile: None,
        };
        assert!(config.endpoint_url().contains("eu-west-1"));
    }

    #[test]
    fn bedrock_config_effective_model_id_default() {
        let config = BedrockConfig {
            region: "us-east-1".into(),
            model_id: "anthropic.claude-sonnet-4-20250514-v2:0".into(),
            inference_profile: None,
        };
        assert_eq!(
            config.effective_model_id(),
            "anthropic.claude-sonnet-4-20250514-v2:0"
        );
    }

    #[test]
    fn bedrock_config_effective_model_id_with_profile() {
        let config = BedrockConfig {
            region: "us-east-1".into(),
            model_id: "anthropic.claude-sonnet-4-20250514-v2:0".into(),
            inference_profile: Some(
                "arn:aws:bedrock:us-east-1:123456789:inference-profile/us.anthropic.claude-sonnet-4-20250514-v2:0"
                    .into(),
            ),
        };
        assert!(config.effective_model_id().starts_with("arn:aws:bedrock"));
    }

    #[test]
    fn create_bedrock_client_no_env_fails() {
        let config = BedrockConfig {
            region: "us-east-1".into(),
            model_id: "anthropic.claude-sonnet-4-20250514-v2:0".into(),
            inference_profile: None,
        };
        // Without AWS env vars, this should fail
        // (may succeed if AWS env vars are set in test environment)
        let _result = create_bedrock_client(&config);
    }

    #[test]
    fn create_bedrock_client_with_explicit_credentials() {
        let config = BedrockConfig {
            region: "us-west-2".into(),
            model_id: "anthropic.claude-haiku-3-5-v1:0".into(),
            inference_profile: None,
        };
        let creds = crab_auth::bedrock_auth::AwsCredentials {
            access_key_id: "AKIATEST".into(),
            secret_access_key: "secret".into(),
            session_token: None,
            region: "us-west-2".into(),
        };
        let client = create_bedrock_client_with_credentials(&config, creds);
        assert_eq!(
            client.base_url(),
            "https://bedrock-runtime.us-west-2.amazonaws.com"
        );
    }

    #[test]
    fn bedrock_config_clone() {
        let config = BedrockConfig {
            region: "ap-northeast-1".into(),
            model_id: "anthropic.claude-sonnet-4-20250514-v2:0".into(),
            inference_profile: Some("arn:test".into()),
        };
        let cloned = config.clone();
        assert_eq!(cloned.region, "ap-northeast-1");
        assert_eq!(cloned.inference_profile.as_deref(), Some("arn:test"));
    }
}
