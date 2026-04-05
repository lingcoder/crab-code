//! LLM API clients for Crab Code.
//!
//! Two independent clients — Anthropic (Messages API) and OpenAI-compatible
//! (Chat Completions) — dispatched via the `LlmBackend` enum.
//! No dynamic trait dispatch; compile-time determined, exhaustive match.

pub mod ab_compare;
pub mod anthropic;
pub mod batch;
pub mod cache;
pub mod capabilities;
pub mod context_optimizer;
pub mod dedup;
pub mod degradation;
pub mod error;
pub mod error_classifier;
pub mod fallback;
pub mod middleware;
pub mod model_selector;
pub mod openai;
pub mod output_format;
pub mod parallel;
pub mod rate_limit;
pub mod response_cache;
pub mod response_scorer;
pub mod retry_strategy;
pub mod stream_aggregator;
pub mod streaming;
pub mod token_budget;
pub mod token_limiter;
pub mod types;
pub mod usage_stats;

#[cfg(feature = "bedrock")]
pub mod bedrock;
#[cfg(feature = "vertex")]
pub mod vertex;

use futures::future::Either;
use futures::stream::Stream;

use crate::error::Result;
use crate::types::{MessageRequest, MessageResponse, StreamEvent};

/// LLM backend enum — provider count is bounded (2 standards + 2 cloud variants).
///
/// Enum dispatch: zero dynamic dispatch overhead, exhaustive match ensures no variant missed.
pub enum LlmBackend {
    Anthropic(anthropic::AnthropicClient),
    OpenAi(openai::OpenAiClient),
    #[cfg(feature = "bedrock")]
    Bedrock(anthropic::AnthropicClient),
    #[cfg(feature = "vertex")]
    Vertex(anthropic::AnthropicClient),
}

impl LlmBackend {
    /// Stream a message request, returning internal `StreamEvent`s.
    pub fn stream_message<'a>(
        &'a self,
        req: MessageRequest<'a>,
    ) -> impl Stream<Item = Result<StreamEvent>> + Send + 'a {
        match self {
            Self::Anthropic(c) => Either::Left(c.stream(req)),
            Self::OpenAi(c) => Either::Right(c.stream(req)),
            #[cfg(feature = "bedrock")]
            Self::Bedrock(c) => Either::Left(c.stream(req)),
            #[cfg(feature = "vertex")]
            Self::Vertex(c) => Either::Left(c.stream(req)),
        }
    }

    /// Non-streaming message send.
    ///
    /// # Errors
    ///
    /// Returns `ApiError` on HTTP, JSON, or API-level errors.
    pub async fn send_message(&self, req: MessageRequest<'_>) -> Result<MessageResponse> {
        match self {
            Self::Anthropic(c) => c.send(req).await,
            Self::OpenAi(c) => c.send(req).await,
            #[cfg(feature = "bedrock")]
            Self::Bedrock(c) => c.send(req).await,
            #[cfg(feature = "vertex")]
            Self::Vertex(c) => c.send(req).await,
        }
    }

    /// Provider name string.
    #[must_use]
    pub const fn name(&self) -> &str {
        match self {
            Self::Anthropic(_) => "anthropic",
            Self::OpenAi(_) => "openai",
            #[cfg(feature = "bedrock")]
            Self::Bedrock(_) => "bedrock",
            #[cfg(feature = "vertex")]
            Self::Vertex(_) => "vertex",
        }
    }

    /// List available models from the provider.
    pub async fn list_models(&self) -> Result<Vec<capabilities::ModelInfo>> {
        match self {
            Self::Anthropic(c) => c.list_models().await,
            Self::OpenAi(c) => c.list_models().await,
            #[cfg(feature = "bedrock")]
            Self::Bedrock(c) => c.list_models().await,
            #[cfg(feature = "vertex")]
            Self::Vertex(c) => c.list_models().await,
        }
    }

    /// Health check — verify the provider is reachable and the API key is valid.
    pub async fn health_check(&self) -> capabilities::HealthStatus {
        match self {
            Self::Anthropic(c) => c.health_check().await,
            Self::OpenAi(c) => c.health_check().await,
            #[cfg(feature = "bedrock")]
            Self::Bedrock(c) => c.health_check().await,
            #[cfg(feature = "vertex")]
            Self::Vertex(c) => c.health_check().await,
        }
    }

    /// Get capabilities for a specific model, using known defaults.
    #[must_use]
    pub fn model_capabilities(&self, model_id: &str) -> capabilities::ModelCapabilities {
        match self {
            Self::Anthropic(_) => capabilities::ModelCapabilities::anthropic(model_id),
            Self::OpenAi(_) => capabilities::ModelCapabilities::openai(model_id),
            #[cfg(feature = "bedrock")]
            Self::Bedrock(_) => capabilities::ModelCapabilities::anthropic(model_id),
            #[cfg(feature = "vertex")]
            Self::Vertex(_) => capabilities::ModelCapabilities::anthropic(model_id),
        }
    }
}

/// Create an `LlmBackend` from settings.
///
/// Routes to the appropriate client based on `api_provider`:
/// - `"openai"`, `"ollama"`, `"deepseek"`, `"vllm"` → `OpenAiClient`
/// - `"bedrock"` → `BedrockClient` (requires `bedrock` feature)
/// - `"vertex"` → `VertexClient` (requires `vertex` feature)
/// - Everything else (including `None`) → `AnthropicClient`
#[must_use]
pub fn create_backend(settings: &crab_config::Settings) -> LlmBackend {
    match settings.api_provider.as_deref() {
        Some("openai" | "ollama" | "deepseek" | "vllm") => {
            let base_url = settings
                .api_base_url
                .as_deref()
                .unwrap_or("https://api.openai.com/v1");
            let api_key = settings
                .api_key
                .clone()
                .or_else(|| std::env::var("OPENAI_API_KEY").ok());
            LlmBackend::OpenAi(openai::OpenAiClient::new(base_url, api_key))
        }
        #[cfg(feature = "bedrock")]
        Some("bedrock") => {
            let region = settings
                .api_base_url
                .as_deref()
                .unwrap_or("us-east-1")
                .to_string();
            let model_id = settings
                .model
                .as_deref()
                .unwrap_or("anthropic.claude-sonnet-4-20250514-v2:0")
                .to_string();
            let config = bedrock::BedrockConfig {
                region,
                model_id,
                inference_profile: None,
            };
            bedrock::create_bedrock_client(&config).map_or_else(
                |_| {
                    // Fall back to direct Anthropic if Bedrock auth fails
                    let auth = crab_auth::create_auth_provider(settings);
                    LlmBackend::Anthropic(anthropic::AnthropicClient::new(
                        "https://api.anthropic.com",
                        auth,
                    ))
                },
                LlmBackend::Bedrock,
            )
        }
        #[cfg(feature = "vertex")]
        Some("vertex") => {
            let project_id = std::env::var("GOOGLE_CLOUD_PROJECT")
                .or_else(|_| std::env::var("GCLOUD_PROJECT"))
                .unwrap_or_default();
            let region = settings
                .api_base_url
                .as_deref()
                .unwrap_or("us-central1")
                .to_string();
            let model_id = settings
                .model
                .as_deref()
                .unwrap_or("claude-sonnet-4-20250514")
                .to_string();
            let config = vertex::VertexConfig {
                project_id,
                region,
                model_id,
            };
            vertex::create_vertex_client(&config).map_or_else(
                |_| {
                    let auth = crab_auth::create_auth_provider(settings);
                    LlmBackend::Anthropic(anthropic::AnthropicClient::new(
                        "https://api.anthropic.com",
                        auth,
                    ))
                },
                LlmBackend::Vertex,
            )
        }
        _ => {
            let base_url = settings
                .api_base_url
                .as_deref()
                .unwrap_or("https://api.anthropic.com");
            let auth = crab_auth::create_auth_provider(settings);
            LlmBackend::Anthropic(anthropic::AnthropicClient::new(base_url, auth))
        }
    }
}
