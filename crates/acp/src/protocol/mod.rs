//! ACP wire types (scaffold — full surface in Phase δ).
//!
//! ACP is JSON-RPC over stdio: the editor spawns the agent as a child
//! process and uses the child's stdin/stdout as the transport. Frames are
//! line-delimited JSON (LSP-style `Content-Length` is **not** used in ACP).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// ACP protocol version advertised during the `initialize` handshake.
///
/// Kept as a string (not semver-split) because ACP's own spec uses string
/// form. Peers with mismatched versions fall back to shared-subset
/// behaviour per the spec.
pub const PROTOCOL_VERSION: &str = "0.1.0";

/// Agent identity sent back on `initialize`.
///
/// Mirrors the shape MCP uses for its own server-info so cross-protocol
/// logging can reuse a common rendering.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentInfo {
    pub name: String,
    pub version: String,
}

impl Default for AgentInfo {
    fn default() -> Self {
        Self {
            name: "crab".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_agent_info_is_crab() {
        let info = AgentInfo::default();
        assert_eq!(info.name, "crab");
        assert!(!info.version.is_empty());
    }

    #[test]
    fn agent_info_roundtrip() {
        let info = AgentInfo {
            name: "crab".into(),
            version: "0.1.0".into(),
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: AgentInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info.name, back.name);
        assert_eq!(info.version, back.version);
    }
}
