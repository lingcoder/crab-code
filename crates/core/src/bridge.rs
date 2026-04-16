//! Bridge connection status types shared with TUI and `core::Event`.
//!
//! The concrete WebSocket protocol types live in `crab-bridge`; this module
//! only carries the user-visible status shape so consumers can render it
//! without depending on the bridge crate.

use serde::{Deserialize, Serialize};

/// How the bridge is configured to operate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BridgeMode {
    /// No bridge running.
    Local,
    /// Outbound-only: connect to a remote relay.
    Remote,
    /// Both local WebSocket server and remote relay.
    Hybrid,
}

/// Origin of a bridge client connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientSource {
    VsCode,
    JetBrains,
    Web,
    Cli,
    Unknown,
}

/// Current bridge state surfaced to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BridgeStatus {
    /// Feature disabled in config.
    Disabled,
    /// WebSocket server bound and listening.
    Listening { port: u16 },
    /// N clients currently attached.
    Connected(u32),
    /// Fatal error; bridge is stopped.
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_serde_roundtrip() {
        let s = BridgeStatus::Listening { port: 4180 };
        let json = serde_json::to_string(&s).unwrap();
        let back: BridgeStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn client_source_equality() {
        assert_eq!(ClientSource::VsCode, ClientSource::VsCode);
        assert_ne!(ClientSource::VsCode, ClientSource::Web);
    }
}
