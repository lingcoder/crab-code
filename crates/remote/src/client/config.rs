//! Configuration for [`super::RemoteClient`].
//!
//! Split out so `cli` / `daemon` can load + validate config independently
//! of actually opening a connection (e.g. `crab config check` can verify
//! the shape without touching the network).

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::protocol::ClientInfo;

/// Where and how to connect to a crab-proto server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientConfig {
    /// `ws://host:port/` or `wss://host:port/`. Path is ignored — the
    /// server exposes the upgrade at `/` by convention.
    pub url: String,

    /// JWT issued by the target server's `jwt_secret`. Sent in the
    /// `Authorization: Bearer <token>` header during the WebSocket
    /// handshake.
    pub auth_token: String,

    /// How we identify ourselves in the `initialize` handshake. The
    /// server logs this and can surface it in the TUI ("connected:
    /// vscode-extension 1.2.3").
    pub client_info: ClientInfo,

    /// Round-trip timeout for request/response calls. Long enough for a
    /// heavily-loaded server on a slow link; not so long that a dead
    /// connection hangs indefinitely.
    #[serde(with = "duration_secs")]
    pub request_timeout: Duration,

    /// Capacity of the `subscribe_events` broadcast channel. Overflows
    /// are dropped on the slowest subscriber per tokio broadcast
    /// semantics; 256 gives comfortable headroom for TUI rendering
    /// while an agent streams tool-call output.
    pub event_buffer: usize,
}

impl ClientConfig {
    /// Build a minimal config from endpoint + token. Sets `client_info`
    /// to `(crab, CARGO_PKG_VERSION)` and `request_timeout` to 30s.
    pub fn new(url: impl Into<String>, auth_token: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            auth_token: auth_token.into(),
            client_info: ClientInfo {
                name: "crab".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            request_timeout: Duration::from_secs(30),
            event_buffer: 256,
        }
    }
}

/// Serde helper mirroring the one in `server::config` — `Duration` on
/// the wire is integer seconds.
mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_u64(d.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(de)?;
        Ok(Duration::from_secs(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_populates_client_info_and_defaults() {
        let c = ClientConfig::new("ws://127.0.0.1:4180/", "tok");
        assert_eq!(c.client_info.name, "crab");
        assert!(!c.client_info.version.is_empty());
        assert_eq!(c.request_timeout, Duration::from_secs(30));
        assert_eq!(c.event_buffer, 256);
    }

    #[test]
    fn serde_roundtrip_uses_integer_seconds() {
        let c = ClientConfig::new("ws://x/", "t");
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"requestTimeout\":30"));
        let back: ClientConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.request_timeout, c.request_timeout);
    }
}
