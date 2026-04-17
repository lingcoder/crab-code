//! `initialize` handshake messages — the first round-trip after the
//! WebSocket handshake. Carries protocol-version + mutual identification
//! between client and server.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `initialize` request params — first message sent by the client
/// after the WebSocket handshake.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// Protocol version the client speaks. Server rejects on major mismatch.
    pub protocol_version: String,
    /// Free-form client identification — useful for server-side logging
    /// and for the TUI to display "connected: vscode-extension 1.2.3".
    pub client_info: ClientInfo,
}

/// Client identification carried in [`InitializeParams`]. Mirrors the
/// MCP equivalent so a future merge with `crab-mcp::ClientInfo` stays
/// painless.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// `initialize` response — server echoes its own identity + negotiated
/// version.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub server_info: ServerInfo,
}

/// Server identification carried in [`InitializeResult`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[cfg(test)]
mod tests {
    use super::super::PROTOCOL_VERSION;
    use super::*;

    #[test]
    fn initialize_params_roundtrip() {
        let params = InitializeParams {
            protocol_version: PROTOCOL_VERSION.to_string(),
            client_info: ClientInfo {
                name: "test-client".into(),
                version: "1.0".into(),
            },
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"protocolVersion\""));
        assert!(json.contains("\"clientInfo\""));
        let back: InitializeParams = serde_json::from_str(&json).unwrap();
        assert_eq!(back.protocol_version, params.protocol_version);
    }
}
