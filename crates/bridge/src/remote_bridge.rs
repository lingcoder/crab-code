//! Remote bridge — connect to a daemon-managed Crab Code session.
//!
//! The remote bridge enables IDE extensions to connect to sessions
//! running in the background daemon, enabling headless and remote
//! agent workflows.

use crate::protocol::{BridgeRequest, BridgeResponse};
use crate::types::{ConnectionId, ConnectionState};

/// Configuration for a remote bridge connection.
#[derive(Debug, Clone)]
pub struct RemoteBridgeConfig {
    /// Host to connect to (default: localhost).
    pub host: String,
    /// Port to connect to.
    pub port: u16,
    /// Session token for authentication.
    pub session_token: Option<String>,
    /// Connection timeout in milliseconds.
    pub timeout_ms: u64,
}

impl Default for RemoteBridgeConfig {
    fn default() -> Self {
        Self {
            host: "localhost".into(),
            port: 3100,
            session_token: None,
            timeout_ms: 5000,
        }
    }
}

/// A remote bridge connection to a daemon session.
pub struct RemoteBridge {
    /// Connection configuration.
    config: RemoteBridgeConfig,
    /// Connection ID assigned by the server.
    connection_id: Option<ConnectionId>,
    /// Current connection state.
    state: ConnectionState,
}

impl RemoteBridge {
    /// Create a new remote bridge with the given configuration.
    #[must_use]
    pub fn new(config: RemoteBridgeConfig) -> Self {
        Self {
            config,
            connection_id: None,
            state: ConnectionState::Disconnected,
        }
    }

    /// Connect to the remote daemon session.
    pub async fn connect(&mut self) -> crab_common::Result<()> {
        todo!("RemoteBridge::connect — establish WebSocket connection and authenticate")
    }

    /// Disconnect from the remote session.
    pub async fn disconnect(&mut self) -> crab_common::Result<()> {
        todo!("RemoteBridge::disconnect — gracefully close WebSocket connection")
    }

    /// Send a request to the remote session.
    pub async fn send_request(
        &self,
        request: BridgeRequest,
    ) -> crab_common::Result<BridgeResponse> {
        let _ = request;
        todo!("RemoteBridge::send_request — send via WebSocket and await response")
    }

    /// Current connection state.
    #[must_use]
    pub const fn state(&self) -> ConnectionState {
        self.state
    }

    /// Whether the bridge is currently connected.
    #[must_use]
    pub const fn is_connected(&self) -> bool {
        matches!(self.state, ConnectionState::Connected)
    }

    /// The connection ID, if connected.
    #[must_use]
    pub fn connection_id(&self) -> Option<&ConnectionId> {
        self.connection_id.as_ref()
    }

    /// The configured host.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.config.host
    }

    /// The configured port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.config.port
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = RemoteBridgeConfig::default();
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 3100);
        assert!(config.session_token.is_none());
        assert_eq!(config.timeout_ms, 5000);
    }

    #[test]
    fn new_bridge_is_disconnected() {
        let bridge = RemoteBridge::new(RemoteBridgeConfig::default());
        assert!(!bridge.is_connected());
        assert_eq!(bridge.state(), ConnectionState::Disconnected);
        assert!(bridge.connection_id().is_none());
    }

    #[test]
    fn bridge_host_and_port() {
        let config = RemoteBridgeConfig {
            host: "192.168.1.1".into(),
            port: 8080,
            ..Default::default()
        };
        let bridge = RemoteBridge::new(config);
        assert_eq!(bridge.host(), "192.168.1.1");
        assert_eq!(bridge.port(), 8080);
    }
}
