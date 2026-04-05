use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};

use crate::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::transport::Transport;

/// Stdin/stdout transport for MCP servers launched as child processes.
///
/// The MCP stdio protocol frames messages as `Content-Length: N\r\n\r\n{json}`
/// (similar to LSP). Each message is a single JSON-RPC object.
pub struct StdioTransport {
    /// Writer to the child process's stdin (shared for concurrent sends).
    writer: Arc<Mutex<tokio::process::ChildStdin>>,
    /// Pending response senders, keyed by request ID.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    /// Handle to the reader task so we can abort it on close.
    reader_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Handle to the child process.
    child: Arc<Mutex<Child>>,
}

impl StdioTransport {
    /// Spawn an MCP server process and create a stdio transport connected to it.
    ///
    /// The `command` is the executable and `args` are its arguments.
    /// Environment variables can be passed via `env`.
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: Option<&HashMap<String, String>>,
    ) -> crab_common::Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if let Some(env_vars) = env {
            for (k, v) in env_vars {
                cmd.env(k, v);
            }
        }

        let mut child = cmd.spawn().map_err(|e| {
            crab_common::Error::Other(format!("failed to spawn MCP server '{command}': {e}"))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            crab_common::Error::Other("failed to capture MCP server stdin".into())
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            crab_common::Error::Other("failed to capture MCP server stdout".into())
        })?;

        let writer = Arc::new(Mutex::new(stdin));
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Spawn a background task to read responses from stdout.
        let pending_clone = Arc::clone(&pending);
        let reader_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message(&mut reader).await {
                    Ok(Some(data)) => {
                        // Try to parse as a response (has "id" field)
                        if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&data) {
                            let mut map = pending_clone.lock().await;
                            if let Some(tx) = map.remove(&resp.id) {
                                let _ = tx.send(resp);
                            }
                        }
                        // Notifications from server are silently dropped for now.
                        // TODO: emit server notifications through a channel.
                    }
                    Ok(None) => {
                        // EOF — server process closed stdout
                        tracing::debug!("MCP server stdout closed");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("error reading from MCP server: {e}");
                        break;
                    }
                }
            }
        });

        Ok(Self {
            writer,
            pending,
            reader_handle: Mutex::new(Some(reader_handle)),
            child: Arc::new(Mutex::new(child)),
        })
    }

    /// Write a framed message (Content-Length header + body) to stdin.
    async fn write_message(&self, json: &str) -> crab_common::Result<()> {
        let frame = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
        let mut writer = self.writer.lock().await;
        writer.write_all(frame.as_bytes()).await.map_err(|e| {
            crab_common::Error::Other(format!("failed to write to MCP server: {e}"))
        })?;
        writer.flush().await.map_err(|e| {
            crab_common::Error::Other(format!("failed to flush MCP server stdin: {e}"))
        })?;
        drop(writer);
        Ok(())
    }
}

impl Transport for StdioTransport {
    fn send(
        &self,
        req: JsonRpcRequest,
    ) -> Pin<Box<dyn Future<Output = crab_common::Result<JsonRpcResponse>> + Send + '_>> {
        Box::pin(async move {
            let id = req.id;

            // Register a oneshot channel for the response before sending.
            let (tx, rx) = oneshot::channel();
            {
                let mut map = self.pending.lock().await;
                map.insert(id, tx);
            }

            // Serialize and send the request.
            let json = serde_json::to_string(&req).map_err(|e| {
                crab_common::Error::Other(format!("failed to serialize request: {e}"))
            })?;

            tracing::debug!(method = %req.method, id, "sending MCP request");
            self.write_message(&json).await?;

            // Wait for the response from the reader task.
            rx.await.map_err(|_| {
                crab_common::Error::Other("MCP server closed connection before responding".into())
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
            tracing::debug!(method = notif.method, "sending MCP notification");
            self.write_message(&json).await
        })
    }

    fn close(&self) -> Pin<Box<dyn Future<Output = crab_common::Result<()>> + Send + '_>> {
        Box::pin(async move {
            // Abort the reader task.
            let reader_handle = self.reader_handle.lock().await.take();
            if let Some(handle) = reader_handle {
                handle.abort();
            }

            // Kill the child process.
            let _ = self.child.lock().await.kill().await;
            tracing::debug!("MCP server process terminated");
            Ok(())
        })
    }
}

/// Read a single Content-Length framed message from an async reader.
///
/// Returns `Ok(None)` on EOF, `Ok(Some(body))` on success.
async fn read_message<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> crab_common::Result<Option<String>> {
    // Read headers until we find Content-Length.
    let mut content_length: Option<usize> = None;
    let mut header_line = String::new();

    loop {
        header_line.clear();
        let bytes_read = reader
            .read_line(&mut header_line)
            .await
            .map_err(|e| crab_common::Error::Other(format!("failed to read header: {e}")))?;

        if bytes_read == 0 {
            return Ok(None); // EOF
        }

        let trimmed = header_line.trim();
        if trimmed.is_empty() {
            // Empty line = end of headers.
            break;
        }

        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            content_length = value.trim().parse().ok();
        }
    }

    let length = content_length.ok_or_else(|| {
        crab_common::Error::Other("missing Content-Length header in MCP message".into())
    })?;

    // Read exactly `length` bytes of the body.
    let mut body = vec![0u8; length];
    tokio::io::AsyncReadExt::read_exact(reader, &mut body)
        .await
        .map_err(|e| crab_common::Error::Other(format!("failed to read message body: {e}")))?;

    String::from_utf8(body)
        .map(Some)
        .map_err(|e| crab_common::Error::Other(format!("invalid UTF-8 in MCP message: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn read_message_parses_content_length_frame() {
        let data = b"Content-Length: 17\r\n\r\n{\"jsonrpc\":\"2.0\"}";
        let mut reader = BufReader::new(&data[..]);
        let msg = read_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(msg, "{\"jsonrpc\":\"2.0\"}");
    }

    #[tokio::test]
    async fn read_message_returns_none_on_eof() {
        let data = b"";
        let mut reader = BufReader::new(&data[..]);
        let msg = read_message(&mut reader).await.unwrap();
        assert!(msg.is_none());
    }

    #[tokio::test]
    async fn read_message_multiple_headers() {
        let data = b"Content-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
        let mut reader = BufReader::new(&data[..]);
        let msg = read_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(msg, "{}");
    }
}
