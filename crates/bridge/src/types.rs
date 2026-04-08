//! Shared types used across bridge modules.
//!
//! Defines common enumerations, identifiers, and status types used by
//! the bridge protocol, REPL bridge, and WebSocket server.

use serde::{Deserialize, Serialize};

/// Unique identifier for a bridge connection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConnectionId(pub String);

impl ConnectionId {
    /// Create a new connection ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Current state of a bridge connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    /// Connection is being established.
    Connecting,
    /// Connection is active and authenticated.
    Connected,
    /// Connection is in the process of closing.
    Disconnecting,
    /// Connection has been closed.
    Disconnected,
}

/// Type of IDE client connecting via the bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientType {
    /// VS Code extension.
    VsCode,
    /// `JetBrains` IDE plugin.
    JetBrains,
    /// Generic editor via LSP.
    Lsp,
    /// Web-based client.
    Web,
    /// Unknown / other client.
    Other(String),
}

/// Metadata about a connected client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    /// Client type.
    pub client_type: ClientType,
    /// Client version string.
    pub version: String,
    /// Optional client name.
    pub name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_id_display() {
        let id = ConnectionId::new("conn_123");
        assert_eq!(id.to_string(), "conn_123");
    }

    #[test]
    fn connection_state_serde() {
        let state = ConnectionState::Connected;
        let json = serde_json::to_string(&state).unwrap();
        let parsed: ConnectionState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ConnectionState::Connected);
    }

    #[test]
    fn client_type_serde() {
        let ct = ClientType::VsCode;
        let json = serde_json::to_string(&ct).unwrap();
        assert!(json.contains("vs_code"));
        let parsed: ClientType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ClientType::VsCode);
    }

    #[test]
    fn client_info_serde() {
        let info = ClientInfo {
            client_type: ClientType::JetBrains,
            version: "1.0.0".into(),
            name: Some("IntelliJ".into()),
        };
        let json = serde_json::to_string(&info).unwrap();
        let parsed: ClientInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, "1.0.0");
        assert_eq!(parsed.name.as_deref(), Some("IntelliJ"));
    }
}
