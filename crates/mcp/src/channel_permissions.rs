//! Channel-level permissions for MCP servers.
//!
//! Controls which tools and resources each MCP server is allowed to expose.
//! Permissions are configured per-server and evaluated before any tool call
//! or resource access is forwarded to the MCP server process.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Per-server permission rules.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerPermissions {
    /// Tool name patterns that are allowed (glob syntax, e.g., `"read_*"`).
    /// Empty means all tools are allowed.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Tool name patterns that are explicitly denied.
    #[serde(default)]
    pub denied_tools: Vec<String>,
    /// Resource URI patterns that are allowed.
    #[serde(default)]
    pub allowed_resources: Vec<String>,
    /// Resource URI patterns that are denied.
    #[serde(default)]
    pub denied_resources: Vec<String>,
}

/// Manages channel-level permissions across all connected MCP servers.
///
/// Permissions are evaluated in order: deny rules take precedence over allow
/// rules. If no rules match, the default is to allow.
pub struct ChannelPermissions {
    /// Per-server permission configurations.
    servers: HashMap<String, ServerPermissions>,
}

impl ChannelPermissions {
    /// Create with no permission restrictions.
    pub fn new() -> Self {
        Self {
            servers: HashMap::new(),
        }
    }

    /// Create from a map of server name to permissions.
    pub fn from_config(servers: HashMap<String, ServerPermissions>) -> Self {
        Self { servers }
    }

    /// Check whether a tool call is allowed for the given server.
    ///
    /// Returns `true` if no rules match (default-allow) or if the tool
    /// matches an allow pattern without matching any deny pattern.
    pub fn is_tool_allowed(&self, _server: &str, _tool: &str) -> bool {
        todo!()
    }

    /// Check whether a resource access is allowed for the given server.
    pub fn is_resource_allowed(&self, _server: &str, _resource: &str) -> bool {
        todo!()
    }

    /// Set permissions for a server, replacing any existing rules.
    pub fn set_server_permissions(&mut self, server: String, perms: ServerPermissions) {
        self.servers.insert(server, perms);
    }

    /// Remove all permission rules for a server.
    pub fn remove_server(&mut self, server: &str) {
        self.servers.remove(server);
    }

    /// Get the current permissions for a server, if configured.
    pub fn get_server_permissions(&self, server: &str) -> Option<&ServerPermissions> {
        self.servers.get(server)
    }
}

impl Default for ChannelPermissions {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ChannelPermissions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelPermissions")
            .field("server_count", &self.servers.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_permissions_allows_everything() {
        let perms = ChannelPermissions::new();
        // No rules configured → default-allow.
        // (Once `is_tool_allowed` is implemented, this should return true.)
        assert!(perms.servers.is_empty());
    }

    #[test]
    fn server_permissions_serde() {
        let sp = ServerPermissions {
            allowed_tools: vec!["read_*".into()],
            denied_tools: vec!["write_secret".into()],
            allowed_resources: vec![],
            denied_resources: vec![],
        };
        let json = serde_json::to_string(&sp).unwrap();
        let parsed: ServerPermissions = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.allowed_tools, vec!["read_*"]);
        assert_eq!(parsed.denied_tools, vec!["write_secret"]);
    }

    #[test]
    fn set_and_get_server_permissions() {
        let mut cp = ChannelPermissions::new();
        let sp = ServerPermissions::default();
        cp.set_server_permissions("my-server".into(), sp);
        assert!(cp.get_server_permissions("my-server").is_some());
        assert!(cp.get_server_permissions("other").is_none());
    }

    #[test]
    fn remove_server_clears_permissions() {
        let mut cp = ChannelPermissions::new();
        cp.set_server_permissions("srv".into(), ServerPermissions::default());
        cp.remove_server("srv");
        assert!(cp.get_server_permissions("srv").is_none());
    }
}
