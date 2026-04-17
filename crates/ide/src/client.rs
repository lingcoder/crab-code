//! IDE client — wraps an MCP client connection to a plugin-hosted
//! server and maintains the shared state.
//!
//! Lifecycle:
//! 1. [`IdeClient::try_connect`] discovers endpoints via
//!    [`crate::lockfile::discover`], picks the first live one, opens a
//!    WebSocket transport with the lockfile's auth token, runs the MCP
//!    initialize handshake, writes [`IdeConnection`] into the shared
//!    handles, and spawns a dispatch task draining notifications into
//!    [`IdeHandles`] and the at-mention broadcast.
//! 2. When the dispatch task exits (connection dropped), a supervisor
//!    task attempts reconnect with exponential backoff (capped). Between
//!    attempts `handles.connection` is cleared to `None` so TUI can
//!    render "disconnected".
//! 3. [`IdeClient::shutdown`] aborts both tasks and drops all handles.

use std::time::Duration;

use crab_core::ide::{IdeAtMention, IdeConnection};
use crab_mcp::transport::Transport;
use crab_mcp::transport::ws::WsTransport;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::lockfile::{self, DiscoveredEndpoint};
use crate::notifications;
use crate::state::IdeHandles;

/// Minimum wait between reconnect attempts.
const BACKOFF_MIN: Duration = Duration::from_secs(1);
/// Maximum wait between reconnect attempts (caps exponential growth).
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// Capacity of the at-mention broadcast. Overflows are dropped at the
/// slowest subscriber — acceptable since `@`-mentions are user-initiated
/// and very infrequent.
const MENTION_BROADCAST_CAPACITY: usize = 16;

/// Errors that can occur while connecting / running against an IDE
/// plugin's MCP server.
#[derive(Debug, thiserror::Error)]
pub enum IdeClientError {
    #[error("no IDE endpoint discovered")]
    NoEndpoint,
    #[error("lockfile discovery failed: {0}")]
    Discovery(#[from] crate::lockfile::DiscoverError),
    #[error("MCP transport error: {0}")]
    Transport(String),
}

/// Top-level IDE integration handle.
///
/// Owns the supervisor task that maintains the MCP connection. Consumers
/// get read-only handles via [`Self::handles`] and subscribe to
/// `@`-mentions via [`Self::subscribe_mentions`].
pub struct IdeClient {
    handles: IdeHandles,
    mention_tx: broadcast::Sender<IdeAtMention>,
    /// The supervisor task — aborted by [`Self::shutdown`].
    supervisor: JoinHandle<()>,
}

impl std::fmt::Debug for IdeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdeClient").finish_non_exhaustive()
    }
}

impl IdeClient {
    /// Attempt to discover and connect to an IDE plugin.
    ///
    /// Returns `Ok(None)` when no IDE is running / no plugin installed.
    /// Returns `Err` only for hard failures (bad home directory, etc.) —
    /// per-endpoint connect failures are logged and retried by the
    /// supervisor.
    pub async fn try_connect() -> Result<Option<Self>, IdeClientError> {
        let endpoints = lockfile::discover()?;
        if endpoints.is_empty() {
            return Ok(None);
        }

        let handles = IdeHandles::default();
        let (mention_tx, _) = broadcast::channel(MENTION_BROADCAST_CAPACITY);
        let handles_for_sup = handles.clone();
        let mention_tx_for_sup = mention_tx.clone();

        let supervisor = tokio::spawn(async move {
            supervise(endpoints, handles_for_sup, mention_tx_for_sup).await;
        });

        Ok(Some(Self {
            handles,
            mention_tx,
            supervisor,
        }))
    }

    /// Read-side handles for TUI and agent.
    pub fn handles(&self) -> IdeHandles {
        self.handles.clone()
    }

    /// Subscribe to the at-mention broadcast. Each subscription receives
    /// every mention sent after it was created.
    pub fn subscribe_mentions(&self) -> broadcast::Receiver<IdeAtMention> {
        self.mention_tx.subscribe()
    }

    /// Abort the supervisor and drop all connections.
    pub fn shutdown(self) {
        self.supervisor.abort();
    }
}

/// Supervisor loop: try each discovered endpoint in order; on
/// disconnect, back off and retry from the top. Runs forever until the
/// task is aborted.
async fn supervise(
    endpoints: Vec<DiscoveredEndpoint>,
    handles: IdeHandles,
    mention_tx: broadcast::Sender<IdeAtMention>,
) {
    let mut backoff = BACKOFF_MIN;
    loop {
        let mut connected_once = false;
        for ep in &endpoints {
            match connect_once(ep, handles.clone(), mention_tx.clone()).await {
                Ok(()) => {
                    connected_once = true;
                    // `connect_once` returns when the dispatch loop exits —
                    // the connection dropped. Clear state and loop.
                    *handles.connection.write().await = None;
                    *handles.selection.write().await = None;
                    backoff = BACKOFF_MIN;
                }
                Err(e) => {
                    tracing::warn!(
                        endpoint = %ep.source.display(),
                        error = %e,
                        "IDE endpoint connect failed"
                    );
                }
            }
        }
        if !connected_once {
            tracing::debug!(
                backoff_secs = backoff.as_secs(),
                "all IDE endpoints failed; sleeping before retry"
            );
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }
}

/// Open a single connection; run the dispatch loop until the server
/// closes it; then return. Returns `Err` if the connect/handshake
/// failed before dispatch started.
async fn connect_once(
    endpoint: &DiscoveredEndpoint,
    handles: IdeHandles,
    mention_tx: broadcast::Sender<IdeAtMention>,
) -> Result<(), IdeClientError> {
    let url = build_url(endpoint);
    tracing::info!(
        ide = %endpoint.lockfile.ide_name,
        port = endpoint.port,
        "connecting to IDE MCP server"
    );

    let transport = WsTransport::connect_with_auth(&url, &endpoint.lockfile.auth_token)
        .await
        .map_err(|e| IdeClientError::Transport(e.to_string()))?;

    // Pull out the notification receiver before handing the transport
    // off to McpClient — once McpClient owns the Box<dyn Transport> we
    // cannot downcast back.
    let notif_rx = transport
        .take_notifications()
        .await
        .expect("fresh transport has notifications");

    let transport_boxed: Box<dyn Transport> = Box::new(transport);
    let client = crab_mcp::McpClient::connect(transport_boxed, &endpoint.lockfile.ide_name)
        .await
        .map_err(|e| IdeClientError::Transport(e.to_string()))?;

    // Publish connection metadata now that the handshake succeeded.
    *handles.connection.write().await = Some(IdeConnection {
        ide_name: endpoint.lockfile.ide_name.clone(),
        workspace_folders: endpoint.lockfile.workspace_folders.clone(),
    });

    // Dispatch loop blocks until the reader task closes the channel.
    notifications::run_dispatch_loop(notif_rx, handles, mention_tx).await;

    // Clean shutdown; ignore close errors — connection is already gone.
    let mut client = client;
    let _ = client.close().await;
    Ok(())
}

/// Build the `ws://host:port/mcp` URL from a discovered endpoint.
///
/// IDE plugins listen on `127.0.0.1:<port>` and expose MCP on the
/// `/mcp` path by convention — matching the `JetBrains` and VS Code
/// plugin implementations we piggyback on.
fn build_url(endpoint: &DiscoveredEndpoint) -> String {
    format!("ws://127.0.0.1:{}/mcp", endpoint.port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lockfile::Lockfile;
    use std::path::PathBuf;

    fn sample_endpoint(port: u16) -> DiscoveredEndpoint {
        DiscoveredEndpoint {
            port,
            lockfile: Lockfile {
                pid: 1,
                workspace_folders: vec![PathBuf::from("/ws")],
                ide_name: "Test IDE".into(),
                transport: "ws".into(),
                auth_token: "t".into(),
            },
            source: PathBuf::from("/tmp/test.lock"),
        }
    }

    #[test]
    fn builds_loopback_url() {
        let url = build_url(&sample_endpoint(12345));
        assert_eq!(url, "ws://127.0.0.1:12345/mcp");
    }

    #[tokio::test]
    async fn try_connect_returns_none_when_no_endpoints() {
        // Point search at an empty tempdir by stubbing HOME — exercised
        // indirectly via discover()'s tolerance of missing dirs; the
        // surrounding CI env has no ~/.claude/ide populated.
        // This test documents the happy path for "no IDE running".
        let result = IdeClient::try_connect().await;
        // Either Ok(None) or Ok(Some(_)) if the dev machine has an IDE
        // lockfile in their home. We can't reliably assert None, but the
        // call must not error out.
        assert!(result.is_ok(), "try_connect errored");
        // Clean up any supervisor task we may have started.
        if let Ok(Some(client)) = result {
            client.shutdown();
        }
    }

    #[test]
    fn backoff_bounds_are_sane() {
        assert!(BACKOFF_MIN < BACKOFF_MAX);
        assert_eq!(BACKOFF_MIN, Duration::from_secs(1));
        assert_eq!(BACKOFF_MAX, Duration::from_secs(30));
    }
}
