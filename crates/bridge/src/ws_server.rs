//! WebSocket server for persistent IDE bridge connections.
//!
//! Runs a WebSocket server that accepts connections from IDE extensions
//! and routes messages to/from the REPL bridge. Handles authentication,
//! connection lifecycle, and message framing.

use std::net::SocketAddr;

use tokio::sync::broadcast;

use crate::protocol::BridgeNotification;
use crate::types::ConnectionId;

/// Configuration for the WebSocket server.
#[derive(Debug, Clone)]
pub struct WsServerConfig {
    /// Address to bind to.
    pub bind_addr: SocketAddr,
    /// Maximum concurrent connections.
    pub max_connections: usize,
    /// Whether to require authentication tokens.
    pub require_auth: bool,
    /// Ping interval in seconds (0 = disabled).
    pub ping_interval_secs: u64,
}

impl Default for WsServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], 3100)),
            max_connections: 8,
            require_auth: true,
            ping_interval_secs: 30,
        }
    }
}

/// The WebSocket server state.
pub struct WsServer {
    /// Server configuration.
    config: WsServerConfig,
    /// Active connection count.
    connection_count: usize,
    /// Broadcast sender for notifications.
    _broadcast_tx: broadcast::Sender<BridgeNotification>,
}

impl WsServer {
    /// Create a new WebSocket server.
    pub fn new(config: WsServerConfig) -> Self {
        let (broadcast_tx, _) = broadcast::channel(256);
        Self {
            config,
            connection_count: 0,
            _broadcast_tx: broadcast_tx,
        }
    }

    /// Start the WebSocket server, listening for connections.
    ///
    /// This method runs until the cancellation token is triggered.
    pub async fn run(
        &mut self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> crab_common::Result<()> {
        let _ = cancel;
        todo!("WsServer::run — bind, accept WebSocket connections, spawn handlers")
    }

    /// Get the bind address.
    #[must_use]
    pub const fn bind_addr(&self) -> SocketAddr {
        self.config.bind_addr
    }

    /// Get the current connection count.
    #[must_use]
    pub const fn connection_count(&self) -> usize {
        self.connection_count
    }

    /// Check if authentication is required.
    #[must_use]
    pub const fn requires_auth(&self) -> bool {
        self.config.require_auth
    }
}

/// Handle a single WebSocket connection.
#[allow(dead_code)]
async fn handle_connection(
    _conn_id: ConnectionId,
    _addr: SocketAddr,
    _require_auth: bool,
) -> crab_common::Result<()> {
    todo!("handle_connection — authenticate, enter message loop, clean up on disconnect")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = WsServerConfig::default();
        assert_eq!(config.bind_addr.port(), 3100);
        assert_eq!(config.max_connections, 8);
        assert!(config.require_auth);
    }

    #[test]
    fn new_server_has_no_connections() {
        let server = WsServer::new(WsServerConfig::default());
        assert_eq!(server.connection_count(), 0);
        assert!(server.requires_auth());
    }

    #[test]
    fn bind_addr_accessible() {
        let config = WsServerConfig {
            bind_addr: SocketAddr::from(([0, 0, 0, 0], 9090)),
            ..Default::default()
        };
        let server = WsServer::new(config);
        assert_eq!(server.bind_addr().port(), 9090);
    }
}
