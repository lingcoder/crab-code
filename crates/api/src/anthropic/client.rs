//! Anthropic Messages API client — HTTP + SSE + retry.

use eventsource_stream::Eventsource;
use futures::stream::{Stream, StreamExt, TryStreamExt};

use super::convert;
use super::types::{AnthropicResponse, AnthropicSseEvent};
use crate::error::{ApiError, Result};
use crate::types::{MessageRequest, MessageResponse, StreamEvent};

/// Anthropic API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Connection-establishment timeout. Bounds only the TCP/TLS handshake, not the
/// streaming body, so long extended-thinking turns are never cut off.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Maximum idle gap between SSE events before the stream is treated as a
/// transport timeout. Bounds stalls without capping total response duration.
const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Anthropic Messages API client.
pub struct AnthropicClient {
    http: reqwest::Client,
    base_url: String,
    auth: Box<dyn crab_auth::AuthProvider>,
    /// When set, this client targets AWS Bedrock Runtime: requests use the
    /// `/model/<id>/invoke` path, the Bedrock body envelope, and `SigV4`
    /// signing instead of `/v1/messages` with `x-api-key` / bearer auth.
    bedrock_model_id: Option<String>,
}

impl AnthropicClient {
    pub fn new(base_url: &str, auth: Box<dyn crab_auth::AuthProvider>) -> Self {
        Self::build(base_url, auth, None)
    }

    /// Create a client targeting AWS Bedrock Runtime for `model_id`.
    ///
    /// Requests are signed per-request with the provider's
    /// [`crab_auth::AuthProvider::sign_request`] (`SigV4`) and routed to the
    /// Bedrock `InvokeModel` endpoint.
    pub fn new_bedrock(
        base_url: &str,
        auth: Box<dyn crab_auth::AuthProvider>,
        model_id: &str,
    ) -> Self {
        Self::build(base_url, auth, Some(model_id.to_string()))
    }

    fn build(
        base_url: &str,
        auth: Box<dyn crab_auth::AuthProvider>,
        bedrock_model_id: Option<String>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .pool_max_idle_per_host(4)
            .build()
            .unwrap_or_else(|e| {
                panic!("failed to build reqwest HTTP client (TLS init error): {e}")
            });

        Self {
            http,
            base_url: base_url.to_string(),
            auth,
            bedrock_model_id,
        }
    }

    /// Request path for the Messages endpoint.
    ///
    /// Native Anthropic always uses `/v1/messages`. Bedrock uses
    /// `/model/<id>/invoke`, or `/model/<id>/invoke-with-response-stream` when
    /// `stream` is set. The Bedrock model id (which may be an ARN containing
    /// `:` and `/`) is percent-encoded as a single path segment, and the same
    /// encoded path is used for both the URL and `SigV4` signing so they agree.
    fn request_path(&self, stream: bool) -> String {
        self.bedrock_model_id.as_ref().map_or_else(
            || "/v1/messages".to_string(),
            |id| {
                let action = if stream {
                    "invoke-with-response-stream"
                } else {
                    "invoke"
                };
                format!("/model/{}/{action}", urlencoding::encode(id))
            },
        )
    }

    /// Build a POST request to the given Messages-endpoint `path` with auth and
    /// headers.
    ///
    /// On Bedrock, signs the exact `(method, host, path, body)` with `SigV4`
    /// and attaches the resulting headers; otherwise uses `x-api-key` / bearer.
    async fn build_request(&self, path: &str, body: &[u8]) -> Result<reqwest::RequestBuilder> {
        let url = format!("{}{path}", self.base_url);

        let host = signing_host(&self.base_url)?;
        if let Some(signed) = self.auth.sign_request("POST", &host, path, body) {
            let mut builder = self
                .http
                .post(&url)
                .header("content-type", "application/json")
                .body(body.to_vec());
            for (name, value) in signed.headers {
                builder = builder.header(name, value);
            }
            return Ok(builder);
        }

        let auth = self.auth.get_auth().await.map_err(ApiError::Common)?;
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

    /// Serialize a request body for the active target (Bedrock envelope or
    /// native Anthropic body).
    fn encode_body(&self, req: &MessageRequest<'_>, stream: bool) -> Result<Vec<u8>> {
        let api_req = convert::to_anthropic_request(req, stream);
        if self.bedrock_model_id.is_some() {
            convert::to_bedrock_body(&api_req)
        } else {
            serde_json::to_vec(&api_req).map_err(ApiError::Json)
        }
    }

    /// Streaming call — POST the Messages endpoint with `stream: true`.
    ///
    /// Native Anthropic returns SSE; Bedrock returns `vnd.amazon.eventstream`
    /// binary frames decoded via [`bedrock_stream`]. Both map to the same
    /// `StreamEvent` family.
    #[allow(clippy::needless_pass_by_value)] // req must be owned to move into async block
    pub fn stream<'a>(
        &'a self,
        req: MessageRequest<'a>,
    ) -> impl Stream<Item = Result<StreamEvent>> + Send + 'a {
        let inner = futures::stream::once(async move {
            let body = self.encode_body(&req, true)?;
            let path = self.request_path(true);
            let request = self.build_request(&path, &body).await?;
            let response = request.send().await.map_err(ApiError::Http)?;

            let status = response.status();
            if !status.is_success() {
                return Err(error_from_response(response).await);
            }

            #[cfg(feature = "bedrock")]
            if self.bedrock_model_id.is_some() {
                return Ok(
                    super::bedrock_stream::decode_bedrock_stream(response.bytes_stream()).boxed(),
                );
            }

            Ok(parse_native_sse_stream(response))
        })
        .try_flatten();

        crate::stream_timeout::with_idle_timeout(inner, STREAM_IDLE_TIMEOUT)
    }

    /// Non-streaming call — POST `/v1/messages`.
    ///
    /// # Errors
    ///
    /// Returns `ApiError` on HTTP, JSON, or API-level errors.
    pub async fn send(&self, req: MessageRequest<'_>) -> Result<MessageResponse> {
        let body = self.encode_body(&req, false)?;
        let path = self.request_path(false);
        let request = self.build_request(&path, &body).await?;
        let response = request.send().await.map_err(ApiError::Http)?;

        let status = response.status();
        if !status.is_success() {
            return Err(error_from_response(response).await);
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

    /// Health check — verify the API is reachable and the key is valid.
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

/// Parse a native Anthropic SSE response body into a `StreamEvent` stream.
///
/// Unparseable data lines are skipped with a debug log rather than failing the
/// whole stream; unknown event types are absorbed by `AnthropicSseEvent`'s
/// `#[serde(other)]` fallback.
fn parse_native_sse_stream(
    response: reqwest::Response,
) -> futures::stream::BoxStream<'static, Result<StreamEvent>> {
    response
        .bytes_stream()
        .eventsource()
        .map_err(|e| ApiError::Sse(e.to_string()))
        .filter_map(|result| async move {
            match result {
                Err(e) => Some(Err(e)),
                Ok(event) => {
                    if event.data.is_empty() {
                        return None;
                    }
                    match serde_json::from_str::<AnthropicSseEvent>(&event.data) {
                        Ok(sse) => convert::sse_event_to_stream_event(sse).map(Ok),
                        Err(e) => {
                            tracing::debug!(
                                error = %e,
                                "skipping unparseable Anthropic SSE data line"
                            );
                            None
                        }
                    }
                }
            }
        })
        .boxed()
}

/// Extract the host (authority without scheme/port) from a base URL for use as
/// the `SigV4` `host` header. Falls back to the trimmed input when no `://`
/// separator is present.
fn signing_host(base_url: &str) -> Result<String> {
    let after_scheme = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    let host = after_scheme
        .split(['/', '?'])
        .next()
        .unwrap_or(after_scheme)
        .split('@')
        .next_back()
        .unwrap_or(after_scheme);
    if host.is_empty() {
        return Err(ApiError::Common(crab_core::Error::Other(format!(
            "cannot derive signing host from base URL: {base_url}"
        ))));
    }
    Ok(host.to_string())
}

/// Map a non-2xx response to an `ApiError`, preserving the `Retry-After` header
/// for 429/529 and the real status code for every other error.
async fn error_from_response(response: reqwest::Response) -> ApiError {
    let status = response.status();
    if crate::retry_after::is_rate_limit_status(status.as_u16()) {
        return crate::retry_after::rate_limited_from_headers(response.headers());
    }
    let text = response.text().await.unwrap_or_default();
    ApiError::Api {
        status: status.as_u16(),
        message: text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_auth() -> Box<dyn crab_auth::AuthProvider> {
        Box::new(crab_auth::ApiKeyProvider::new("test-key".into()))
    }

    #[test]
    fn signing_host_strips_scheme_and_path() {
        assert_eq!(
            signing_host("https://bedrock-runtime.us-east-1.amazonaws.com").unwrap(),
            "bedrock-runtime.us-east-1.amazonaws.com"
        );
        assert_eq!(
            signing_host("https://bedrock-runtime.us-east-1.amazonaws.com/model/x/invoke").unwrap(),
            "bedrock-runtime.us-east-1.amazonaws.com"
        );
    }

    #[test]
    fn signing_host_strips_userinfo_keeps_port() {
        assert_eq!(
            signing_host("https://user:pass@example.com:8443/path").unwrap(),
            "example.com:8443"
        );
    }

    #[test]
    fn signing_host_without_scheme() {
        assert_eq!(signing_host("example.com").unwrap(), "example.com");
    }

    #[test]
    fn request_path_native_is_messages() {
        let client = AnthropicClient::new("https://api.anthropic.com", dummy_auth());
        assert_eq!(client.request_path(false), "/v1/messages");
        // Native ignores the stream flag; SSE is selected by the request body.
        assert_eq!(client.request_path(true), "/v1/messages");
    }

    #[test]
    fn request_path_bedrock_plain_model_id() {
        let client = AnthropicClient::new_bedrock(
            "https://bedrock-runtime.us-east-1.amazonaws.com",
            dummy_auth(),
            "anthropic.claude-sonnet-4-20250514-v2:0",
        );
        assert_eq!(
            client.request_path(false),
            "/model/anthropic.claude-sonnet-4-20250514-v2%3A0/invoke"
        );
    }

    #[test]
    fn request_path_bedrock_streaming_uses_response_stream_action() {
        let client = AnthropicClient::new_bedrock(
            "https://bedrock-runtime.us-east-1.amazonaws.com",
            dummy_auth(),
            "anthropic.claude",
        );
        assert_eq!(
            client.request_path(true),
            "/model/anthropic.claude/invoke-with-response-stream"
        );
        assert_eq!(client.request_path(false), "/model/anthropic.claude/invoke");
    }

    #[test]
    fn request_path_bedrock_arn_is_encoded() {
        let client = AnthropicClient::new_bedrock(
            "https://bedrock-runtime.us-east-1.amazonaws.com",
            dummy_auth(),
            "arn:aws:bedrock:us-east-1:1:inference-profile/us.anthropic.claude",
        );
        let path = client.request_path(false);
        assert!(path.starts_with("/model/"));
        assert!(path.ends_with("/invoke"));
        // Colons and slashes inside the ARN are percent-encoded, not raw.
        let segment = path
            .strip_prefix("/model/")
            .and_then(|s| s.strip_suffix("/invoke"))
            .unwrap();
        assert!(!segment.contains(':'));
        assert!(!segment.contains('/'));
        assert!(segment.contains("%3A"));
        assert!(segment.contains("%2F"));
    }
}
