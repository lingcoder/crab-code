//! Agent identification sent back on `initialize`.
//!
//! Mirrors the shape MCP uses for its own server-info so cross-protocol
//! logging can reuse a common rendering.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
