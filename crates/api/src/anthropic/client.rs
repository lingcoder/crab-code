//! Anthropic Messages API client — HTTP + SSE + retry.

use eventsource_stream::Eventsource;
use futures::stream::{Stream, StreamExt, TryStreamExt};

use super::convert;
use super::types::{AnthropicResponse, AnthropicSseEvent};
use crate::error::{ApiError, Result};
use crate::types::{MessageRequest, MessageResponse, StreamEvent};

/// Anthropic API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic Messages API client.
pub struct AnthropicClient {
    http: reqwest::Client,
    base_url: String,
    auth: Box<dyn crab_auth::AuthProvider>,
    anti_track: Option<crab_config::AntiTrackConfig>,
}

impl AnthropicClient {
    pub fn new(base_url: &str, auth: Box<dyn crab_auth::AuthProvider>) -> Self {
        Self::new_with_anti_track(base_url, auth, None)
    }

    pub fn new_with_anti_track(
        base_url: &str,
        auth: Box<dyn crab_auth::AuthProvider>,
        anti_track: Option<crab_config::AntiTrackConfig>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .pool_max_idle_per_host(4)
            .build()
            .expect("failed to build HTTP client");

        Self {
            http,
            base_url: base_url.to_string(),
            auth,
            anti_track,
        }
    }

    /// Build a POST request to `/v1/messages` with auth and standard headers.
    async fn build_request(&self, body: &[u8]) -> Result<reqwest::RequestBuilder> {
        let auth = self.auth.get_auth().await.map_err(ApiError::Common)?;

        let url = format!("{}/v1/messages", self.base_url);
        let mut builder = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .header("anthropic-version", ANTHROPIC_VERSION)
            .body(body.to_vec());

        match auth {
            crab_auth::AuthMethod::ApiKey(key) => {
                builder = builder.header("x-api-key", key);
            }
            crab_auth::AuthMethod::OAuth(token) => {
                builder = builder.header("authorization", format!("Bearer {}", token.access_token));
            }
        }

        Ok(builder)
    }

    /// Apply anti-track sanitization to a request before sending.
    fn apply_anti_track(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref config) = self.anti_track {
            match builder.build() {
                Ok(mut request) => {
                    crate::anti_track::sanitize_request(&mut request, config);
                    self.http.request(request.method().clone(), request.url().clone()).headers(request.headers().clone()).body(request.body().cloned().unwrap_or_default())
                }
                Err(_) => builder,
            }
        } else {
            builder
        }
    }

    /// Streaming call — POST `/v1/messages` with `stream: true`.
    ///
    /// Returns a stream of `StreamEvent` mapped from Anthropic SSE events.
    #[allow(clippy::needless_pass_by_value)] // req must be owned to move into async block
    pub fn stream<'a>(
        &'a self,
        req: MessageRequest<'a>,
    ) -> impl Stream<Item = Result<StreamEvent>> + Send + 'a {
        let api_req = convert::to_anthropic_request(&req, true);

        futures::stream::once(async move {
            let body = serde_json::to_vec(&api_req).map_err(ApiError::Json)?;
            let request = self.build_request(&body).await?;
            let request = self.apply_anti_track(request);
            let response = request.send().await.map_err(ApiError::Http)?;

            let status = response.status();
            if !status.is_success() {
                let text = response.text().await.unwrap_or_default();
                return Err(ApiError::Api {
                    status: status.as_u16(),
                    message: text,
                });
            }

            let byte_stream = response.bytes_stream();
            let event_stream = byte_stream.eventsource();

            Ok(event_stream
                .map_err(|e| ApiError::Sse(e.to_string()))
                .filter_map(|result| async move {
                    match result {
                        Err(e) => Some(Err(e)),
                        Ok(event) => {
                            if event.data.is_empty() {
                                return None;
                            }
                            let parsed: std::result::Result<AnthropicSseEvent, _> =
                                serde_json::from_str(&event.data);
                            match parsed {
                                Ok(sse) => convert::sse_event_to_stream_event(sse).map(Ok),
                                Err(e) => Some(Err(ApiError::Json(e))),
                            }
                        }
                    }
                }))
        })
        .try_flatten()
    }

    /// Non-streaming call — POST `/v1/messages`.
    ///
    /// # Errors
    ///
    /// Returns `ApiError` on HTTP, JSON, or API-level errors.
    pub async fn send(&self, req: MessageRequest<'_>) -> Result<MessageResponse> {
        let api_req = convert::to_anthropic_request(&req, false);
        let body = serde_json::to_vec(&api_req).map_err(ApiError::Json)?;
        let request = self.build_request(&body).await?;
        let request = self.apply_anti_track(request);
        let response = request.send().await.map_err(ApiError::Http)?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(ApiError::Api {
                status: status.as_u16(),
                message: text,
            });
        }

        let resp_body = response.bytes().await.map_err(ApiError::Http)?;
        let api_resp: AnthropicResponse =
            serde_json::from_slice(&resp_body).map_err(ApiError::Json)?;

        let id = api_resp.id.clone();
        let (message, usage) = convert::from_anthropic_response(api_resp)?;

        Ok(MessageResponse { id, message, usage })
    }

    /// Base URL for this client.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// List available models from the Anthropic API.
    ///
    /// Calls `GET /v1/models` and returns a list of model info.
    pub async fn list_models(&self) -> Result<Vec<crate::capabilities::ModelInfo>> {
        let auth = self.auth.get_auth().await.map_err(ApiError::Common)?;
        let url = format!("{}/v1/models", self.base_url);

        let mut builder = self
            .http
            .get(&url)
            .header("anthropic-version", ANTHROPIC_VERSION);

        match auth {
            crab_auth::AuthMethod::ApiKey(key) => {
                builder = builder.header("x-api-key", key);
            }
            crab_auth::AuthMethod::OAuth(token) => {
                builder = builder.header("authorization", format!("Bearer {}", token.access_token));
            }
        }

        let builder = self.apply_anti_track(builder);
        let response = builder.send().await.map_err(ApiError::Http)?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(ApiError::Api {
                status: status.as_u16(),
                message: text,
            });
        }

        let body: serde_json::Value = response.json().await.map_err(ApiError::Http)?;
        let models = body
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let id = m.get("id")?.as_str()?.to_string();
                        let name = m
                            .get("display_name")
                            .and_then(|n| n.as_str())
                            .map(String::from);
                        Some(crate::capabilities::ModelInfo {
                            id,
                            name,
                            provider: "anthropic".into(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(models)
    }

    /// Health check — verify the API is reachable and the API key is valid.
    ///
    /// Sends a minimal request to validate connectivity and authentication.
    pub async fn health_check(&self) -> crate::capabilities::HealthStatus {
        let start = std::time::Instant::now();

        // Try to list models as a lightweight auth-validating endpoint
        let auth_result = self.auth.get_auth().await;
        let auth = match auth_result {
            Ok(a) => a,
            Err(e) => {
                return crate::capabilities::HealthStatus {
                    healthy: false,
                    latency: start.elapsed(),
                    error: Some(format!("auth error: {e}")),
                };
            }
        };

        let url = format!("{}/v1/models", self.base_url);
        let mut builder = self
            .http
            .get(&url)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .timeout(std::time::Duration::from_secs(10));

        match auth {
            crab_auth::AuthMethod::ApiKey(key) => {
                builder = builder.header("x-api-key", key);
            }
            crab_auth::AuthMethod::OAuth(token) => {
                builder = builder.header("authorization", format!("Bearer {}", token.access_token));
            }
        }

        let builder = self.apply_anti_track(builder);
        match builder.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    crate::capabilities::HealthStatus {
                        healthy: true,
                        latency: start.elapsed(),
                        error: None,
                    }
                } else {
                    crate::capabilities::HealthStatus {
                        healthy: false,
                        latency: start.elapsed(),
                        error: Some(format!("HTTP {status}")),
                    }
                }
            }
            Err(e) => crate::capabilities::HealthStatus {
                healthy: false,
                latency: start.elapsed(),
                error: Some(e.to_string()),
            },
        }
    }
}
