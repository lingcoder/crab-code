//! OpenAI-compatible Chat Completions API client — HTTP + SSE + retry.

use futures::stream::{self, Stream, StreamExt};

use crate::error::{ApiError, Result};
use crate::types::{MessageRequest, MessageResponse, StreamEvent};

use super::convert;

/// Chat Completions API client.
///
/// Compatible with `OpenAI`, Ollama, `DeepSeek`, vLLM, and any provider
/// implementing the `/v1/chat/completions` endpoint.
pub struct OpenAiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    anti_track: Option<crab_config::AntiTrackConfig>,
}

impl OpenAiClient {
    pub fn new(base_url: &str, api_key: Option<String>) -> Self {
        Self::new_with_anti_track(base_url, api_key, None)
    }

    pub fn new_with_anti_track(
        base_url: &str,
        api_key: Option<String>,
        anti_track: Option<crab_config::AntiTrackConfig>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .pool_max_idle_per_host(4)
            .build()
            .expect("failed to build HTTP client");

        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            anti_track,
        }
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

    /// Build the request with standard headers.
    fn build_request(&self, body: &impl serde::Serialize) -> reqwest::RequestBuilder {
        let url = format!("{}/chat/completions", self.base_url);
        let mut builder = self.http.post(&url);

        if let Some(key) = &self.api_key {
            builder = builder.bearer_auth(key);
        }

        builder
            .header("Content-Type", "application/json")
            .json(body)
    }

    /// Streaming call — POST `/chat/completions` with `stream: true`.
    ///
    /// Returns a stream of `StreamEvent` mapped from SSE `data:` lines.
    /// The stream ends when `data: [DONE]` is received.
    #[allow(clippy::needless_pass_by_value)] // req must be owned to move into async block
    pub fn stream<'a>(
        &'a self,
        req: MessageRequest<'a>,
    ) -> impl Stream<Item = Result<StreamEvent>> + Send + 'a {
        let chat_req = convert::to_chat_completion_request(&req, true);

        // We need to use stream::once + flatten to handle the async request
        // setup followed by the streaming response.
        stream::once(async move {
            let resp = self.apply_anti_track(self.build_request(&chat_req)).send().await.map_err(|e| {
                if e.is_timeout() {
                    ApiError::Timeout
                } else {
                    ApiError::Http(e)
                }
            })?;

            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(ApiError::Api {
                    status: status.as_u16(),
                    message: body,
                });
            }

            Ok(parse_sse_stream(resp))
        })
        .flat_map(|result| match result {
            Ok(event_stream) => event_stream.boxed(),
            Err(e) => stream::once(async move { Err(e) }).boxed(),
        })
    }

    /// Non-streaming call — POST `/chat/completions`.
    ///
    /// # Errors
    ///
    /// Returns `ApiError` on HTTP, JSON, or API-level errors.
    pub async fn send(&self, req: MessageRequest<'_>) -> Result<MessageResponse> {
        let chat_req = convert::to_chat_completion_request(&req, false);

        let resp = self.apply_anti_track(self.build_request(&chat_req)).send().await.map_err(|e| {
            if e.is_timeout() {
                ApiError::Timeout
            } else {
                ApiError::Http(e)
            }
        })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiError::Api {
                status: status.as_u16(),
                message: body,
            });
        }

        let body: super::types::ChatCompletionResponse = resp.json().await?;
        convert::from_chat_completion_response(body)
    }

    /// Base URL for this client.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// List available models from the OpenAI-compatible API.
    ///
    /// Calls `GET /models` and returns a list of model info.
    pub async fn list_models(&self) -> crate::error::Result<Vec<crate::capabilities::ModelInfo>> {
        let url = format!("{}/models", self.base_url);
        let mut builder = self.http.get(&url);

        if let Some(key) = &self.api_key {
            builder = builder.bearer_auth(key);
        }

        let builder = self.apply_anti_track(builder);
        let response = builder.send().await.map_err(|e| {
            if e.is_timeout() {
                crate::error::ApiError::Timeout
            } else {
                crate::error::ApiError::Http(e)
            }
        })?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(crate::error::ApiError::Api {
                status: status.as_u16(),
                message: text,
            });
        }

        let body: serde_json::Value = response.json().await?;
        let models = body
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let id = m.get("id")?.as_str()?.to_string();
                        Some(crate::capabilities::ModelInfo {
                            id,
                            name: None,
                            provider: "openai".into(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(models)
    }

    /// Health check — verify the API is reachable and the key is valid.
    ///
    /// Sends a GET to `/models` with a short timeout to validate connectivity.
    pub async fn health_check(&self) -> crate::capabilities::HealthStatus {
        let start = std::time::Instant::now();
        let url = format!("{}/models", self.base_url);

        let mut builder = self
            .http
            .get(&url)
            .timeout(std::time::Duration::from_secs(10));

        if let Some(key) = &self.api_key {
            builder = builder.bearer_auth(key);
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

/// Parse an SSE response body into a stream of `StreamEvent`s.
///
/// Each `data: {...}` line is parsed as a `ChatCompletionChunk` and converted
/// to internal events. The stream terminates on `data: [DONE]`.
fn parse_sse_stream(resp: reqwest::Response) -> impl Stream<Item = Result<StreamEvent>> + Send {
    use eventsource_stream::Eventsource;

    resp.bytes_stream()
        .eventsource()
        .take_while(|event| {
            let done = matches!(event, Ok(ev) if ev.data == "[DONE]");
            async move { !done }
        })
        .flat_map(move |event| match event {
            Ok(ev) => {
                if ev.data.is_empty() || ev.data == "[DONE]" {
                    return stream::iter(vec![]).boxed();
                }

                match serde_json::from_str::<super::types::ChatCompletionChunk>(&ev.data) {
                    Ok(chunk) => {
                        let events: Vec<Result<StreamEvent>> =
                            convert::chunk_to_stream_event(&chunk)
                                .into_iter()
                                .map(Ok)
                                .collect();
                        stream::iter(events).boxed()
                    }
                    Err(e) => stream::once(async move {
                        Err(ApiError::Sse(format!("failed to parse SSE chunk: {e}")))
                    })
                    .boxed(),
                }
            }
            Err(e) => {
                stream::once(async move { Err(ApiError::Sse(format!("SSE stream error: {e}"))) })
                    .boxed()
            }
        })
}
