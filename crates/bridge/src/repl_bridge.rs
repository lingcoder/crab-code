//! REPL bridge — relay between IDE extensions and running Crab Code sessions.
//!
//! The REPL bridge allows an IDE to attach to an active REPL session,
//! sending user messages and receiving streaming responses. It manages
//! the bidirectional message flow and handles session lifecycle events.

use tokio::sync::{broadcast, mpsc};

use crate::protocol::{BridgeNotification, BridgeRequest, BridgeResponse};
use crate::types::{ClientInfo, ConnectionId, ConnectionState};

/// Configuration for the REPL bridge.
#[derive(Debug, Clone)]
pub struct ReplBridgeConfig {
    /// Maximum number of concurrent connections.
    pub max_connections: usize,
    /// Buffer size for the message channel.
    pub channel_buffer: usize,
}

impl Default for ReplBridgeConfig {
    fn default() -> Self {
        Self {
            max_connections: 4,
            channel_buffer: 256,
        }
    }
}

/// Handle to a connected client within the REPL bridge.
#[derive(Debug)]
pub struct ClientHandle {
    /// Connection identifier.
    pub id: ConnectionId,
    /// Client metadata.
    pub info: ClientInfo,
    /// Current connection state.
    pub state: ConnectionState,
}

/// The REPL bridge manages connections between IDE clients and the active session.
pub struct ReplBridge {
    /// Bridge configuration.
    config: ReplBridgeConfig,
    /// Connected clients.
    clients: Vec<ClientHandle>,
    /// Sender for broadcasting notifications to all clients.
    _broadcast_tx: broadcast::Sender<BridgeNotification>,
    /// Receiver for incoming requests from clients.
    _request_rx: mpsc::Receiver<(ConnectionId, BridgeRequest)>,
    /// Sender for incoming requests (cloned to each client handler).
    _request_tx: mpsc::Sender<(ConnectionId, BridgeRequest)>,
}

impl ReplBridge {
    /// Create a new REPL bridge with the given configuration.
    pub fn new(config: ReplBridgeConfig) -> Self {
        let (broadcast_tx, _) = broadcast::channel(config.channel_buffer);
        let (request_tx, request_rx) = mpsc::channel(config.channel_buffer);

        Self {
            config,
            clients: Vec::new(),
            _broadcast_tx: broadcast_tx,
            _request_rx: request_rx,
            _request_tx: request_tx,
        }
    }

    /// Accept a new client connection.
    pub async fn accept_client(&mut self, info: ClientInfo) -> crab_common::Result<ConnectionId> {
        let _ = info;
        todo!("ReplBridge::accept_client — register client and set up message channels")
    }

    /// Disconnect a client by connection ID.
    pub async fn disconnect_client(&mut self, id: &ConnectionId) -> crab_common::Result<()> {
        let _ = id;
        todo!("ReplBridge::disconnect_client — clean up client state and notify")
    }

    /// Send a response to a specific client.
    pub async fn send_response(
        &self,
        client_id: &ConnectionId,
        response: BridgeResponse,
    ) -> crab_common::Result<()> {
        let _ = (client_id, response);
        todo!("ReplBridge::send_response — route response to correct client channel")
    }

    /// Broadcast a notification to all connected clients.
    pub fn broadcast(&self, notification: &BridgeNotification) -> crab_common::Result<()> {
        let _ = notification;
        todo!("ReplBridge::broadcast — send notification via broadcast channel")
    }

    /// Number of currently connected clients.
    #[must_use]
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// Maximum allowed connections.
    #[must_use]
    pub fn max_connections(&self) -> usize {
        self.config.max_connections
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = ReplBridgeConfig::default();
        assert_eq!(config.max_connections, 4);
        assert_eq!(config.channel_buffer, 256);
    }

    #[test]
    fn new_bridge_has_no_clients() {
        let bridge = ReplBridge::new(ReplBridgeConfig::default());
        assert_eq!(bridge.client_count(), 0);
        assert_eq!(bridge.max_connections(), 4);
    }
}
