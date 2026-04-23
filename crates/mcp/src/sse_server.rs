//! HTTP SSE server transport for the MCP server.
//!
//! Implements the MCP SSE protocol:
//! - `GET /sse` — opens a Server-Sent Events stream. The server immediately
//!   sends an `endpoint` event with the URL for posting JSON-RPC messages.
//!   Subsequent `message` events carry JSON-RPC responses.
//! - `POST /messages?session_id=<id>` — receives JSON-RPC requests from clients.
//!
//! Supports multiple concurrent client sessions, each with its own SSE stream.
//! Includes request timeout handling (default 30s per request).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::server::{McpServer, ToolHandler};

/// Default timeout for processing a single JSON-RPC request.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Type alias for the SSE sender channel.
type SseSender = tokio::sync::mpsc::UnboundedSender<String>;

/// Session registry mapping session IDs to their SSE senders.
type SessionRegistry = Arc<Mutex<HashMap<String, SseSender>>>;

/// Run an MCP server in HTTP SSE mode.
///
/// Listens on `127.0.0.1:{port}` and handles concurrent client sessions.
/// Each client opens a GET /sse connection and receives responses via SSE.
/// Requests are sent via POST /`messages?session_id`=<id>.
///
/// Returns when the cancellation token is triggered.
pub async fn run_sse<H: ToolHandler + 'static>(
    server: Arc<McpServer<H>>,
    port: u16,
    cancel: CancellationToken,
) -> crab_core::Result<()> {
    run_sse_with_timeout(server, port, cancel, DEFAULT_REQUEST_TIMEOUT).await
}

/// Run SSE server with a custom request timeout.
pub async fn run_sse_with_timeout<H: ToolHandler + 'static>(
    server: Arc<McpServer<H>>,
    port: u16,
    cancel: CancellationToken,
    request_timeout: Duration,
) -> crab_core::Result<()> {
    let listener = TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .map_err(|e| crab_core::Error::Other(format!("failed to bind port {port}: {e}")))?;

    tracing::info!(port, "MCP SSE server listening");

    let sessions: SessionRegistry = Arc::new(Mutex::new(HashMap::new()));

    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                tracing::debug!("MCP SSE server shutting down");
                break;
            }
            result = listener.accept() => {
                let (stream, addr) = result.map_err(|e| {
                    crab_core::Error::Other(format!("accept error: {e}"))
                })?;
                tracing::debug!(%addr, "new HTTP connection");
                let server = Arc::clone(&server);
                let sessions = Arc::clone(&sessions);
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, server, sessions, port, request_timeout).await {
                        tracing::warn!(%addr, error = %e, "connection error");
                    }
                });
            }
        }
    }
    Ok(())
}

/// Parse a minimal HTTP request from a TCP stream.
/// Returns (method, path, body).
async fn parse_http_request(
    stream: &mut BufReader<tokio::net::TcpStream>,
) -> crab_core::Result<(String, String, String)> {
    // Read request line
    let mut request_line = String::new();
    stream
        .read_line(&mut request_line)
        .await
        .map_err(|e| crab_core::Error::Other(format!("failed to read request line: {e}")))?;

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(crab_core::Error::Other("invalid HTTP request line".into()));
    }
    let method = parts[0].to_string();
    let path = parts[1].to_string();

    // Read headers
    let mut content_length: usize = 0;
    let mut header = String::new();
    loop {
        header.clear();
        stream
            .read_line(&mut header)
            .await
            .map_err(|e| crab_core::Error::Other(format!("failed to read header: {e}")))?;
        let trimmed = header.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(val) = trimmed.strip_prefix("Content-Length:") {
            content_length = val.trim().parse().unwrap_or(0);
        }
    }

    // Read body if present
    let body = if content_length > 0 {
        let mut buf = vec![0u8; content_length];
        tokio::io::AsyncReadExt::read_exact(stream, &mut buf)
            .await
            .map_err(|e| crab_core::Error::Other(format!("failed to read body: {e}")))?;
        String::from_utf8(buf)
            .map_err(|e| crab_core::Error::Other(format!("invalid UTF-8 body: {e}")))?
    } else {
        String::new()
    };

    Ok((method, path, body))
}

/// Extract a query parameter from a URL path.
fn query_param<'a>(path: &'a str, key: &str) -> Option<&'a str> {
    let query = path.split('?').nth(1)?;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=')
            && k == key
        {
            return Some(v);
        }
    }
    None
}

/// Handle a single HTTP connection — either SSE GET or JSON-RPC POST.
async fn handle_connection<H: ToolHandler + 'static>(
    stream: tokio::net::TcpStream,
    server: Arc<McpServer<H>>,
    sessions: SessionRegistry,
    port: u16,
    request_timeout: Duration,
) -> crab_core::Result<()> {
    let mut reader = BufReader::new(stream);
    let (method, path, body) = parse_http_request(&mut reader).await?;

    match (method.as_str(), path.split('?').next().unwrap_or(&path)) {
        ("GET", "/sse") => handle_sse_stream(reader.into_inner(), sessions, port).await,
        ("POST", "/messages") => {
            let session_id = query_param(&path, "session_id").unwrap_or("").to_string();
            handle_post_message(
                reader.into_inner(),
                body,
                session_id,
                server,
                sessions,
                request_timeout,
            )
            .await
        }
        _ => {
            let response = "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nNot Found";
            let stream = reader.into_inner();
            let mut stream = stream;
            stream
                .write_all(response.as_bytes())
                .await
                .map_err(|e| crab_core::Error::Other(format!("write error: {e}")))?;
            Ok(())
        }
    }
}

/// Handle GET /sse — establish SSE stream for a new client session.
async fn handle_sse_stream(
    mut stream: tokio::net::TcpStream,
    sessions: SessionRegistry,
    port: u16,
) -> crab_core::Result<()> {
    let session_id = uuid_v4_simple();

    // Send HTTP response headers for SSE
    let headers = "HTTP/1.1 200 OK\r\n\
                   Content-Type: text/event-stream\r\n\
                   Cache-Control: no-cache\r\n\
                   Connection: keep-alive\r\n\
                   Access-Control-Allow-Origin: *\r\n\
                   \r\n";
    stream
        .write_all(headers.as_bytes())
        .await
        .map_err(|e| crab_core::Error::Other(format!("write SSE headers: {e}")))?;

    // Send the `endpoint` event with the POST URL
    let endpoint_url = format!("http://127.0.0.1:{port}/messages?session_id={session_id}");
    let endpoint_event = format!("event: endpoint\ndata: {endpoint_url}\n\n");
    stream
        .write_all(endpoint_event.as_bytes())
        .await
        .map_err(|e| crab_core::Error::Other(format!("write endpoint event: {e}")))?;
    stream
        .flush()
        .await
        .map_err(|e| crab_core::Error::Other(format!("flush: {e}")))?;

    tracing::debug!(session_id = %session_id, "SSE session established");

    // Create a channel for sending SSE events to this client
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    {
        sessions.lock().await.insert(session_id.clone(), tx);
    }

    // Forward channel messages as SSE events until the client disconnects
    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Some(data) => {
                        let event = format!("event: message\ndata: {data}\n\n");
                        if stream.write_all(event.as_bytes()).await.is_err() {
                            break;
                        }
                        if stream.flush().await.is_err() {
                            break;
                        }
                    }
                    None => break, // Channel closed
                }
            }
        }
    }

    // Clean up session
    sessions.lock().await.remove(&session_id);
    tracing::debug!(session_id = %session_id, "SSE session closed");
    Ok(())
}

/// Handle POST /messages — process a JSON-RPC request and send response via SSE.
async fn handle_post_message<H: ToolHandler + 'static>(
    mut stream: tokio::net::TcpStream,
    body: String,
    session_id: String,
    server: Arc<McpServer<H>>,
    sessions: SessionRegistry,
    request_timeout: Duration,
) -> crab_core::Result<()> {
    // Parse JSON-RPC request
    let req: JsonRpcRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => {
            let error_resp = JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: 0,
                result: None,
                error: Some(crate::protocol::JsonRpcError {
                    code: -32700,
                    message: format!("Parse error: {e}"),
                    data: None,
                }),
            };
            let json = serde_json::to_string(&error_resp).unwrap_or_default();
            send_to_session(&sessions, &session_id, &json).await;
            send_http_response(&mut stream, 200, "application/json", &json).await?;
            return Ok(());
        }
    };

    // Process with timeout
    let Ok(resp) = tokio::time::timeout(request_timeout, server.handle_request_public(req)).await
    else {
        let timeout_resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 0,
            result: None,
            error: Some(crate::protocol::JsonRpcError {
                code: -32000,
                message: "Request timed out".into(),
                data: None,
            }),
        };
        let json = serde_json::to_string(&timeout_resp).unwrap_or_default();
        send_to_session(&sessions, &session_id, &json).await;
        send_http_response(&mut stream, 200, "application/json", &json).await?;
        return Ok(());
    };

    let json = serde_json::to_string(&resp)
        .map_err(|e| crab_core::Error::Other(format!("serialize response: {e}")))?;

    // Send response via SSE channel
    send_to_session(&sessions, &session_id, &json).await;

    // Also send HTTP 202 Accepted to the POST request
    send_http_response(&mut stream, 202, "text/plain", "Accepted").await
}

/// Send a message to a session's SSE stream.
async fn send_to_session(sessions: &SessionRegistry, session_id: &str, json: &str) {
    let map = sessions.lock().await;
    if let Some(tx) = map.get(session_id) {
        let _ = tx.send(json.to_string());
    }
}

/// Send an HTTP response on a TCP stream.
async fn send_http_response(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> crab_core::Result<()> {
    let status_text = match status {
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "OK",
    };
    let response = format!(
        "HTTP/1.1 {status} {status_text}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|e| crab_core::Error::Other(format!("write response: {e}")))?;
    stream
        .flush()
        .await
        .map_err(|e| crab_core::Error::Other(format!("flush response: {e}")))?;
    Ok(())
}

/// Generate a simple UUID v4-like session ID without pulling in the uuid crate.
fn uuid_v4_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    #[allow(clippy::cast_possible_truncation)]
    let r: u64 = (t ^ (t >> 17) ^ (t << 13)) as u64;
    format!("{r:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{McpToolDef, ToolCallResult, ToolResultContent, method};
    use crate::server::ToolHandler;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Minimal test handler for SSE server tests.
    struct SseTestHandler;

    impl ToolHandler for SseTestHandler {
        fn list_tools(&self) -> Vec<McpToolDef> {
            vec![McpToolDef {
                name: "ping_tool".into(),
                description: "Returns pong".into(),
                input_schema: json!({"type": "object"}),
            }]
        }

        fn call_tool(
            &self,
            name: &str,
            _arguments: Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolCallResult> + Send + '_>>
        {
            let name = name.to_string();
            Box::pin(async move {
                ToolCallResult {
                    content: vec![ToolResultContent::Text {
                        text: format!("pong from {name}"),
                    }],
                    is_error: false,
                }
            })
        }
    }

    fn make_sse_server() -> Arc<McpServer<SseTestHandler>> {
        Arc::new(McpServer::new(
            "sse-test",
            "0.1.0",
            Arc::new(SseTestHandler),
        ))
    }

    /// Find a free port for testing.
    async fn free_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    }

    #[test]
    fn query_param_extracts() {
        assert_eq!(
            query_param("/messages?session_id=abc123&foo=bar", "session_id"),
            Some("abc123")
        );
        assert_eq!(
            query_param("/messages?session_id=abc123&foo=bar", "foo"),
            Some("bar")
        );
        assert_eq!(query_param("/messages?session_id=abc123", "missing"), None);
        assert_eq!(query_param("/messages", "session_id"), None);
    }

    #[test]
    fn uuid_v4_simple_generates_hex() {
        let id = uuid_v4_simple();
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn sse_server_starts_and_stops() {
        let server = make_sse_server();
        let port = free_port().await;
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move { run_sse(server, port, cancel_clone).await });

        // Give server a moment to bind
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Verify we can connect
        let result = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await;
        assert!(result.is_ok());

        // Stop the server
        cancel.cancel();
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn sse_get_returns_endpoint_event() {
        let server = make_sse_server();
        let port = free_port().await;
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            let _ = run_sse(server, port, cancel_clone).await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Connect and send GET /sse
        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        let request = "GET /sse HTTP/1.1\r\nHost: localhost\r\n\r\n";
        stream.write_all(request.as_bytes()).await.unwrap();

        // Read response — may arrive in multiple chunks, so accumulate
        let mut buf = vec![0u8; 8192];
        let mut total = 0;
        for _ in 0..10 {
            match tokio::time::timeout(Duration::from_millis(500), stream.read(&mut buf[total..]))
                .await
            {
                Ok(Ok(n)) if n > 0 => {
                    total += n;
                    let so_far = String::from_utf8_lossy(&buf[..total]);
                    if so_far.contains("event: endpoint") {
                        break;
                    }
                }
                _ => break,
            }
        }
        let response = String::from_utf8_lossy(&buf[..total]);

        assert!(response.contains("200 OK"));
        assert!(response.contains("text/event-stream"));
        assert!(response.contains("event: endpoint"));
        assert!(response.contains("/messages?session_id="));

        cancel.cancel();
    }

    #[tokio::test]
    async fn sse_404_for_unknown_path() {
        let server = make_sse_server();
        let port = free_port().await;
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            let _ = run_sse(server, port, cancel_clone).await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        let request = "GET /unknown HTTP/1.1\r\nHost: localhost\r\n\r\n";
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut buf = vec![0u8; 1024];
        let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("404"));

        cancel.cancel();
    }

    #[tokio::test]
    async fn sse_post_invalid_json_returns_parse_error() {
        let server = make_sse_server();
        let port = free_port().await;
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            let _ = run_sse(server, port, cancel_clone).await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let body = "not valid json";
        let request = format!(
            "POST /messages?session_id=test123 HTTP/1.1\r\n\
             Host: localhost\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             \r\n\
             {body}",
            body.len()
        );

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut buf = vec![0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("200")); // HTTP 200 with error in body
        assert!(response.contains("Parse error") || response.contains("-32700"));

        cancel.cancel();
    }

    #[tokio::test]
    async fn sse_full_session_initialize_and_tools_list() {
        let server = make_sse_server();
        let port = free_port().await;
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            let _ = run_sse(server, port, cancel_clone).await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Step 1: GET /sse to establish session
        let mut sse_stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        let request = "GET /sse HTTP/1.1\r\nHost: localhost\r\n\r\n";
        sse_stream.write_all(request.as_bytes()).await.unwrap();

        // Read SSE response — may need multiple reads to get the full endpoint event
        let mut sse_response = String::new();
        let mut buf = vec![0u8; 4096];
        for _ in 0..5 {
            let n = tokio::time::timeout(Duration::from_secs(2), sse_stream.read(&mut buf))
                .await
                .unwrap()
                .unwrap();
            sse_response.push_str(&String::from_utf8_lossy(&buf[..n]));
            if sse_response.contains("session_id=") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Extract session_id from the endpoint event
        let session_id = sse_response
            .split("session_id=")
            .nth(1)
            .and_then(|s| s.split(|c: char| !c.is_ascii_hexdigit()).next())
            .expect("expected session_id in endpoint event");

        // Step 2: POST initialize request
        let init_body = serde_json::to_string(&JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 1,
            method: method::INITIALIZE.into(),
            params: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test-sse-client", "version": "1.0"}
            })),
        })
        .unwrap();

        let post_request = format!(
            "POST /messages?session_id={session_id} HTTP/1.1\r\n\
             Host: localhost\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             \r\n\
             {init_body}",
            init_body.len()
        );

        let mut post_stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        post_stream
            .write_all(post_request.as_bytes())
            .await
            .unwrap();

        // Read POST response (202 Accepted)
        let mut post_buf = vec![0u8; 1024];
        let pn = tokio::time::timeout(Duration::from_secs(2), post_stream.read(&mut post_buf))
            .await
            .unwrap()
            .unwrap();
        let post_response = String::from_utf8_lossy(&post_buf[..pn]);
        assert!(post_response.contains("202") || post_response.contains("200"));

        // Read the SSE message event with the initialize response
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut sse_buf = vec![0u8; 4096];
        let sn = tokio::time::timeout(Duration::from_secs(2), sse_stream.read(&mut sse_buf))
            .await
            .unwrap()
            .unwrap();
        let sse_msg = String::from_utf8_lossy(&sse_buf[..sn]).to_string();
        assert!(sse_msg.contains("event: message"));
        assert!(sse_msg.contains("protocolVersion"));

        cancel.cancel();
    }

    #[tokio::test]
    async fn sse_request_timeout() {
        use crate::protocol::{McpToolDef, ToolCallResult, ToolResultContent};

        // Create a handler with a slow tool
        struct SlowHandler;
        impl ToolHandler for SlowHandler {
            fn list_tools(&self) -> Vec<McpToolDef> {
                vec![McpToolDef {
                    name: "slow".into(),
                    description: "Takes forever".into(),
                    input_schema: json!({"type": "object"}),
                }]
            }
            fn call_tool(
                &self,
                _name: &str,
                _arguments: Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolCallResult> + Send + '_>>
            {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    ToolCallResult {
                        content: vec![ToolResultContent::Text {
                            text: "done".into(),
                        }],
                        is_error: false,
                    }
                })
            }
        }

        let server = Arc::new(McpServer::new(
            "slow-server",
            "0.1.0",
            Arc::new(SlowHandler),
        ));
        let port = free_port().await;
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            // Use very short timeout for testing
            let _ =
                run_sse_with_timeout(server, port, cancel_clone, Duration::from_millis(100)).await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        // POST a tools/call that will timeout
        let body = serde_json::to_string(&JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 99,
            method: method::TOOLS_CALL.into(),
            params: Some(json!({"name": "slow", "arguments": {}})),
        })
        .unwrap();

        let request = format!(
            "POST /messages?session_id=nosession HTTP/1.1\r\n\
             Host: localhost\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             \r\n\
             {body}",
            body.len()
        );

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut buf = vec![0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("timed out") || response.contains("-32000"));

        cancel.cancel();
    }
}
