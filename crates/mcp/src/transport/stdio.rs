use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};

use crate::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::transport::Transport;

/// Per-request timeout — a request that gets no response within this window
/// fails rather than hanging forever (e.g. the server died mid-request).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Stdin/stdout transport for MCP servers launched as child processes.
///
/// The MCP stdio protocol frames each message as a single line of JSON
/// terminated by `\n`, with no embedded newlines (per the MCP spec).
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
    ) -> crab_core::Result<Self> {
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
            crab_core::Error::Other(format!("failed to spawn MCP server '{command}': {e}"))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| crab_core::Error::Other("failed to capture MCP server stdin".into()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| crab_core::Error::Other("failed to capture MCP server stdout".into()))?;

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
                        // A response carries an "id"; a notification does not.
                        if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&data) {
                            let mut map = pending_clone.lock().await;
                            if let Some(tx) = map.remove(&resp.id) {
                                let _ = tx.send(resp);
                            }
                        } else if let Ok(notif) = serde_json::from_str::<JsonRpcNotification>(&data)
                        {
                            // No consumer surface for stdio server notifications
                            // yet; log so they're observable but not lost silently.
                            tracing::debug!(
                                method = notif.method,
                                "received MCP server notification (no consumer wired)"
                            );
                        }
                    }
                    Ok(None) => {
                        tracing::debug!("MCP server stdout closed");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("error reading from MCP server: {e}");
                        break;
                    }
                }
            }
            // Reader is exiting (EOF or error): drop every pending sender so
            // in-flight `rx.await` callers get an error promptly instead of
            // hanging forever waiting on a dead server.
            pending_clone.lock().await.clear();
        });

        Ok(Self {
            writer,
            pending,
            reader_handle: Mutex::new(Some(reader_handle)),
            child: Arc::new(Mutex::new(child)),
        })
    }

    /// Write a newline-delimited JSON message to the server's stdin.
    async fn write_message(&self, json: &str) -> crab_core::Result<()> {
        let mut frame = String::with_capacity(json.len() + 1);
        frame.push_str(json);
        frame.push('\n');
        let mut writer = self.writer.lock().await;
        writer
            .write_all(frame.as_bytes())
            .await
            .map_err(|e| crab_core::Error::Other(format!("failed to write to MCP server: {e}")))?;
        writer.flush().await.map_err(|e| {
            crab_core::Error::Other(format!("failed to flush MCP server stdin: {e}"))
        })?;
        drop(writer);
        Ok(())
    }
}

impl Transport for StdioTransport {
    fn send(
        &self,
        req: JsonRpcRequest,
    ) -> Pin<Box<dyn Future<Output = crab_core::Result<JsonRpcResponse>> + Send + '_>> {
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
                crab_core::Error::Other(format!("failed to serialize request: {e}"))
            })?;

            tracing::debug!(method = %req.method, id, "sending MCP request");
            self.write_message(&json).await?;

            // Wait for the response, bounded by REQUEST_TIMEOUT so a dead or
            // wedged server can't hang the caller indefinitely.
            match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
                Ok(Ok(resp)) => Ok(resp),
                Ok(Err(_)) => Err(crab_core::Error::Other(
                    "MCP server closed connection before responding".into(),
                )),
                Err(_) => {
                    self.pending.lock().await.remove(&id);
                    Err(crab_core::Error::Other(format!(
                        "MCP server did not respond within {REQUEST_TIMEOUT:?}"
                    )))
                }
            }
        })
    }

    fn notify(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = crab_core::Result<()>> + Send + '_>> {
        let notif = JsonRpcNotification::new(
            method.to_string(),
            if params.is_null() { None } else { Some(params) },
        );
        Box::pin(async move {
            let json = serde_json::to_string(&notif).map_err(|e| {
                crab_core::Error::Other(format!("failed to serialize notification: {e}"))
            })?;
            tracing::debug!(method = notif.method, "sending MCP notification");
            self.write_message(&json).await
        })
    }

    fn close(&self) -> Pin<Box<dyn Future<Output = crab_core::Result<()>> + Send + '_>> {
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

/// Read a single newline-delimited JSON message from an async reader.
///
/// Returns `Ok(None)` on EOF, `Ok(Some(line))` on success. Blank lines are
/// skipped. Per the MCP stdio spec each JSON-RPC message is one line with no
/// embedded newlines.
async fn read_message<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> crab_core::Result<Option<String>> {
    let mut line = String::new();
    loop {
        line.clear();
        let bytes_read = reader
            .read_line(&mut line)
            .await
            .map_err(|e| crab_core::Error::Other(format!("failed to read MCP message: {e}")))?;

        if bytes_read == 0 {
            return Ok(None); // EOF
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue; // Tolerate blank lines between messages.
        }
        return Ok(Some(trimmed.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn read_message_parses_newline_delimited_json() {
        let data = b"{\"jsonrpc\":\"2.0\"}\n";
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
    async fn read_message_skips_blank_lines() {
        let data = b"\n\n{}\n";
        let mut reader = BufReader::new(&data[..]);
        let msg = read_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(msg, "{}");
    }

    #[test]
    fn request_timeout_is_bounded() {
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
    }

    /// A server that exits immediately (closing stdout) must cause an in-flight
    /// request to fail promptly rather than hang forever. The reader task hits
    /// EOF, drains `pending`, and the dropped sender resolves `rx.await` with an
    /// error well before the 30s request timeout.
    #[tokio::test]
    async fn send_errors_when_server_dies_before_responding() {
        let (command, args) = if cfg!(windows) {
            (
                "cmd".to_string(),
                vec!["/C".to_string(), "exit".to_string()],
            )
        } else {
            (
                "sh".to_string(),
                vec!["-c".to_string(), "exit 0".to_string()],
            )
        };

        let transport = StdioTransport::spawn(&command, &args, None)
            .await
            .expect("spawn short-lived server");

        let req = JsonRpcRequest::new("initialize", None);
        // Bound the whole call so a regression (a real hang) fails the test
        // instead of stalling the suite.
        let result = tokio::time::timeout(Duration::from_secs(5), transport.send(req)).await;

        assert!(
            result.is_ok(),
            "send hung instead of erroring on dead server"
        );
        assert!(
            result.unwrap().is_err(),
            "send should error when the server dies before responding"
        );
    }
}
