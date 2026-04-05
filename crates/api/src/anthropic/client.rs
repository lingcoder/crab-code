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
}

impl AnthropicClient {
    pub fn new(base_url: &str, auth: Box<dyn crab_auth::AuthProvider>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .pool_max_idle_per_host(4)
            .build()
            .expect("failed to build HTTP client");

        Self {
            http,
            base_url: base_url.to_string(),
            auth,
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
}
