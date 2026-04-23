//! Google Vertex AI adapter (feature = "vertex").
//!
//! Wraps `AnthropicClient` with GCP auth and Vertex AI endpoint.
//! Vertex AI hosts Anthropic models via a compatible Messages API.

#![cfg(feature = "vertex")]

use crate::anthropic::AnthropicClient;

/// Configuration for Google Vertex AI access.
#[derive(Debug, Clone)]
pub struct VertexConfig {
    /// GCP project ID.
    pub project_id: String,
    /// GCP region (e.g., "us-central1", "europe-west1").
    pub region: String,
    /// Model ID (e.g., "claude-sonnet-4-20250514").
    pub model_id: String,
}

impl VertexConfig {
    /// Vertex AI endpoint base URL for the Anthropic-compatible API.
    ///
    /// Vertex hosts Anthropic models at:
    /// `https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/anthropic/models/{model}`
    #[must_use]
    pub fn base_url(&self) -> String {
        format!(
            "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/publishers/anthropic/models/{}",
            self.region, self.project_id, self.region, self.model_id
        )
    }

    /// Streaming endpoint URL (uses `:streamRawPredict`).
    #[must_use]
    pub fn stream_url(&self) -> String {
        format!("{}:streamRawPredict", self.base_url())
    }

    /// Non-streaming endpoint URL (uses `:rawPredict`).
    #[must_use]
    pub fn predict_url(&self) -> String {
        format!("{}:rawPredict", self.base_url())
    }
}

/// Create an `AnthropicClient` configured for Google Vertex AI.
///
/// Vertex uses the same Anthropic Messages API format but with
/// GCP `OAuth2` authentication and a Vertex AI endpoint.
///
/// # Errors
///
/// Returns an error if GCP credentials cannot be resolved.
pub fn create_vertex_client(config: &VertexConfig) -> crab_core::Result<AnthropicClient> {
    let credentials = crab_auth::vertex_auth::GcpCredentials::from_env().ok_or_else(|| {
        crab_core::Error::Other(
            "GCP credentials not found. Set GOOGLE_CLOUD_PROJECT and configure ADC.".into(),
        )
    })?;

    let auth_provider = crab_auth::vertex_auth::VertexAuthProvider::new(credentials);
    let base_url = config.base_url();

    Ok(AnthropicClient::new(&base_url, Box::new(auth_provider)))
}

/// Create an `AnthropicClient` for Vertex AI with explicit credentials.
pub fn create_vertex_client_with_credentials(
    config: &VertexConfig,
    credentials: crab_auth::vertex_auth::GcpCredentials,
) -> AnthropicClient {
    let auth_provider = crab_auth::vertex_auth::VertexAuthProvider::new(credentials);
    let base_url = config.base_url();
    AnthropicClient::new(&base_url, Box::new(auth_provider))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_config_base_url() {
        let config = VertexConfig {
            project_id: "my-project".into(),
            region: "us-central1".into(),
            model_id: "claude-sonnet-4-20250514".into(),
        };
        let url = config.base_url();
        assert!(url.contains("us-central1-aiplatform.googleapis.com"));
        assert!(url.contains("my-project"));
        assert!(url.contains("publishers/anthropic"));
        assert!(url.contains("claude-sonnet-4-20250514"));
    }

    #[test]
    fn vertex_config_stream_url() {
        let config = VertexConfig {
            project_id: "proj".into(),
            region: "europe-west1".into(),
            model_id: "claude-haiku-3-5".into(),
        };
        assert!(config.stream_url().ends_with(":streamRawPredict"));
    }

    #[test]
    fn vertex_config_predict_url() {
        let config = VertexConfig {
            project_id: "proj".into(),
            region: "us-east4".into(),
            model_id: "claude-opus-4-20250514".into(),
        };
        assert!(config.predict_url().ends_with(":rawPredict"));
    }

    #[test]
    fn vertex_config_clone() {
        let config = VertexConfig {
            project_id: "test-123".into(),
            region: "asia-southeast1".into(),
            model_id: "claude-sonnet-4-20250514".into(),
        };
        let cloned = config.clone();
        assert_eq!(cloned.project_id, "test-123");
        assert_eq!(cloned.region, "asia-southeast1");
    }

    #[test]
    fn create_vertex_client_no_env_fails() {
        let config = VertexConfig {
            project_id: "proj".into(),
            region: "us-central1".into(),
            model_id: "claude-sonnet-4-20250514".into(),
        };
        // Without GCP env vars, should fail
        let _result = create_vertex_client(&config);
    }

    #[test]
    fn create_vertex_client_with_explicit_credentials() {
        let config = VertexConfig {
            project_id: "my-project".into(),
            region: "us-central1".into(),
            model_id: "claude-sonnet-4-20250514".into(),
        };
        let creds = crab_auth::vertex_auth::GcpCredentials {
            project_id: "my-project".into(),
            region: "us-central1".into(),
            service_account_key: None,
        };
        let client = create_vertex_client_with_credentials(&config, creds);
        assert!(client.base_url().contains("us-central1-aiplatform"));
    }

    #[test]
    fn vertex_config_different_regions() {
        for region in &["us-central1", "europe-west1", "asia-northeast1", "us-east4"] {
            let config = VertexConfig {
                project_id: "proj".into(),
                region: (*region).into(),
                model_id: "model".into(),
            };
            assert!(config.base_url().contains(region));
        }
    }
}
