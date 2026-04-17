//! Errors surfaced by [`super::RemoteClient`].
//!
//! Wire-level (envelope / JSON-RPC) failures are kept separate from
//! protocol-level (server returned an error reply) failures so consumers
//! can retry the former but not the latter.

use crate::protocol::JsonRpcError;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("invalid server URL: {0}")]
    InvalidUrl(String),

    #[error("invalid auth token (not HTTP-header-safe): {0}")]
    InvalidAuthToken(String),

    #[error("WebSocket handshake failed: {0}")]
    Handshake(#[source] tokio_tungstenite::tungstenite::Error),

    #[error("WebSocket read/write error: {0}")]
    Transport(#[source] tokio_tungstenite::tungstenite::Error),

    #[error("server speaks protocol {server}, client speaks {client}")]
    IncompatibleProtocol { server: String, client: String },

    #[error("server replied with JSON-RPC error {}: {}", .0.code, .0.message)]
    ServerError(JsonRpcError),

    #[error("connection closed before the response arrived (pending id {0})")]
    ConnectionClosed(u64),

    #[error("failed to (de)serialise {what}: {source}")]
    Serde {
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },

    #[error("client is already closed")]
    AlreadyClosed,
}
