#[cfg(feature = "sse")]
mod inner {
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    use eventsource_stream::Eventsource as _;
    use futures::StreamExt;
    use tokio::sync::{Mutex, oneshot};

    use crate::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
    use crate::transport::Transport;

    /// HTTP Server-Sent Events transport for remote MCP servers.
    ///
    /// The MCP SSE protocol:
    /// 1. Client opens a GET request to the SSE endpoint.
    /// 2. Server sends an `endpoint` event with the URL for posting messages.
    /// 3. Client POSTs JSON-RPC requests to that endpoint.
    /// 4. Server sends JSON-RPC responses as `message` SSE events.
    pub struct SseTransport {
        /// The base URL of the SSE endpoint (for reference/logging).
        #[allow(dead_code)]
        base_url: String,
        /// The POST endpoint URL received from the server.
        post_url: Arc<Mutex<Option<String>>>,
        /// HTTP client for posting messages.
        http_client: reqwest::Client,
        /// Pending response senders, keyed by request ID.
        pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
        /// Handle to the SSE reader task.
        reader_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    }

    impl SseTransport {
        /// Connect to an MCP SSE endpoint.
        ///
        /// Opens the SSE stream and waits for the `endpoint` event before
        /// returning, so the transport is ready for use immediately.
        pub async fn connect(url: &str) -> crab_common::Result<Self> {
            let http_client = reqwest::Client::new();
            let post_url: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
            let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
                Arc::new(Mutex::new(HashMap::new()));

            // Resolve the base URL for constructing the POST endpoint.
            let base_url = extract_base_url(url);

            // Open the SSE stream.
            let response = http_client
                .get(url)
                .header("Accept", "text/event-stream")
                .send()
                .await
                .map_err(|e| {
                    crab_common::Error::Other(format!(
                        "failed to connect to MCP SSE endpoint '{url}': {e}"
                    ))
                })?;

            if !response.status().is_success() {
                return Err(crab_common::Error::Other(format!(
                    "MCP SSE endpoint returned status {}",
                    response.status()
                )));
            }

            // Parse SSE events from the response body stream.
            let byte_stream = response.bytes_stream();
            let event_stream = byte_stream.eventsource();

            // Notify when the endpoint URL has been received.
            let (endpoint_tx, endpoint_rx) = oneshot::channel::<String>();

            let pending_clone = Arc::clone(&pending);
            let post_url_clone = Arc::clone(&post_url);
            let base_url_clone = base_url.clone();

            let reader_handle = tokio::spawn(async move {
                let mut stream = std::pin::pin!(event_stream);
                let mut endpoint_tx = Some(endpoint_tx);

                while let Some(event_result) = stream.next().await {
                    let event = match event_result {
                        Ok(ev) => ev,
                        Err(e) => {
                            tracing::warn!("SSE stream error: {e}");
                            break;
                        }
                    };

                    match event.event.as_str() {
                        "endpoint" => {
                            // The data is the relative or absolute URL for POSTing.
                            let endpoint = resolve_url(&base_url_clone, event.data.trim());
                            tracing::debug!(endpoint = %endpoint, "received MCP SSE endpoint");

                            *post_url_clone.lock().await = Some(endpoint.clone());

                            if let Some(tx) = endpoint_tx.take() {
                                let _ = tx.send(endpoint);
                            }
                        }
                        "message" => {
                            // Parse as JSON-RPC response.
                            let data = event.data.trim();
                            if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(data) {
                                let mut map = pending_clone.lock().await;
                                if let Some(tx) = map.remove(&resp.id) {
                                    let _ = tx.send(resp);
                                }
                            }
                            // Notifications from server are silently dropped for now.
                        }
                        _ => {
                            // Unknown event types are ignored.
                            tracing::trace!(event_type = %event.event, "ignoring SSE event");
                        }
                    }
                }

                tracing::debug!("MCP SSE stream closed");
            });

            // Wait for the endpoint URL (with timeout).
            let post_endpoint =
                tokio::time::timeout(std::time::Duration::from_secs(30), endpoint_rx)
                    .await
                    .map_err(|_| {
                        crab_common::Error::Other(
                            "timeout waiting for MCP SSE endpoint event".into(),
                        )
                    })?
                    .map_err(|_| {
                        crab_common::Error::Other(
                            "SSE stream closed before sending endpoint".into(),
                        )
                    })?;

            tracing::info!(
                url = url,
                post_endpoint = %post_endpoint,
                "MCP SSE transport connected"
            );

            Ok(Self {
                base_url: url.to_string(),
                post_url,
                http_client,
                pending,
                reader_handle: Mutex::new(Some(reader_handle)),
            })
        }

        /// POST a JSON message to the server's endpoint.
        async fn post_message(&self, json: &str) -> crab_common::Result<()> {
            let url = self.post_url.lock().await.clone().ok_or_else(|| {
                crab_common::Error::Other("MCP SSE endpoint URL not yet received".into())
            })?;

            let resp = self
                .http_client
                .post(&url)
                .header("Content-Type", "application/json")
                .body(json.to_string())
                .send()
                .await
                .map_err(|e| {
                    crab_common::Error::Other(format!("failed to POST to MCP SSE endpoint: {e}"))
                })?;

            if !resp.status().is_success() {
                return Err(crab_common::Error::Other(format!(
                    "MCP SSE POST returned status {}",
                    resp.status()
                )));
            }

            Ok(())
        }
    }

    impl Transport for SseTransport {
        fn send(
            &self,
            req: JsonRpcRequest,
        ) -> Pin<Box<dyn Future<Output = crab_common::Result<JsonRpcResponse>> + Send + '_>>
        {
            Box::pin(async move {
                let id = req.id;

                // Register a oneshot channel for the response.
                let (tx, rx) = oneshot::channel();
                {
                    let mut map = self.pending.lock().await;
                    map.insert(id, tx);
                }

                // Serialize and POST the request.
                let json = serde_json::to_string(&req).map_err(|e| {
                    crab_common::Error::Other(format!("failed to serialize request: {e}"))
                })?;

                tracing::debug!(method = %req.method, id, "sending MCP SSE request");
                self.post_message(&json).await?;

                // Wait for the response from the SSE stream.
                rx.await.map_err(|_| {
                    crab_common::Error::Other("MCP SSE stream closed before responding".into())
                })
            })
        }

        fn notify(
            &self,
            method: &str,
            params: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = crab_common::Result<()>> + Send + '_>> {
            let notif = JsonRpcNotification::new(
                method.to_string(),
                if params.is_null() { None } else { Some(params) },
            );
            Box::pin(async move {
                let json = serde_json::to_string(&notif).map_err(|e| {
                    crab_common::Error::Other(format!("failed to serialize notification: {e}"))
                })?;
                tracing::debug!(method = notif.method, "sending MCP SSE notification");
                self.post_message(&json).await
            })
        }

        fn close(&self) -> Pin<Box<dyn Future<Output = crab_common::Result<()>> + Send + '_>> {
            Box::pin(async move {
                let handle = self.reader_handle.lock().await.take();
                if let Some(h) = handle {
                    h.abort();
                }
                tracing::debug!("MCP SSE transport closed");
                Ok(())
            })
        }
    }

    /// Extract the base URL (scheme + host + port) from a full URL.
    fn extract_base_url(url: &str) -> String {
        // Find the end of scheme://host[:port]
        if let Some(scheme_end) = url.find("://") {
            let after_scheme = &url[scheme_end + 3..];
            if let Some(path_start) = after_scheme.find('/') {
                return url[..scheme_end + 3 + path_start].to_string();
            }
        }
        url.to_string()
    }

    /// Resolve a potentially relative URL against a base URL.
    fn resolve_url(base: &str, url: &str) -> String {
        if url.starts_with("http://") || url.starts_with("https://") {
            url.to_string()
        } else if url.starts_with('/') {
            format!("{base}{url}")
        } else {
            format!("{base}/{url}")
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn extract_base_url_with_path() {
            assert_eq!(
                extract_base_url("https://mcp.example.com/sse"),
                "https://mcp.example.com"
            );
        }

        #[test]
        fn extract_base_url_with_port() {
            assert_eq!(
                extract_base_url("http://localhost:3000/events"),
                "http://localhost:3000"
            );
        }

        #[test]
        fn extract_base_url_no_path() {
            assert_eq!(
                extract_base_url("https://mcp.example.com"),
                "https://mcp.example.com"
            );
        }

        #[test]
        fn resolve_url_absolute() {
            assert_eq!(
                resolve_url("https://base.com", "https://other.com/endpoint"),
                "https://other.com/endpoint"
            );
        }

        #[test]
        fn resolve_url_relative_slash() {
            assert_eq!(
                resolve_url("https://base.com", "/api/messages"),
                "https://base.com/api/messages"
            );
        }

        #[test]
        fn resolve_url_relative_no_slash() {
            assert_eq!(
                resolve_url("https://base.com", "messages"),
                "https://base.com/messages"
            );
        }
    }
}

#[cfg(feature = "sse")]
pub use inner::SseTransport;

// Stub when SSE feature is disabled — allows code to reference the type
// in non-SSE builds without cfg gates everywhere.
#[cfg(not(feature = "sse"))]
pub struct SseTransport;

#[cfg(not(feature = "sse"))]
impl SseTransport {
    pub async fn connect(_url: &str) -> crab_common::Result<Self> {
        Err(crab_common::Error::Other(
            "SSE transport requires the 'sse' feature".into(),
        ))
    }
}

#[cfg(not(feature = "sse"))]
impl crate::transport::Transport for SseTransport {
    fn send(
        &self,
        _req: crate::protocol::JsonRpcRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = crab_common::Result<crate::protocol::JsonRpcResponse>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            Err(crab_common::Error::Other(
                "SSE transport requires the 'sse' feature".into(),
            ))
        })
    }

    fn notify(
        &self,
        _method: &str,
        _params: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crab_common::Result<()>> + Send + '_>>
    {
        Box::pin(async move {
            Err(crab_common::Error::Other(
                "SSE transport requires the 'sse' feature".into(),
            ))
        })
    }

    fn close(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crab_common::Result<()>> + Send + '_>>
    {
        Box::pin(async move { Ok(()) })
    }
}
