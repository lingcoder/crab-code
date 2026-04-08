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
/// Deny rules take precedence over allow rules. If no rules match, default is allow.
pub struct ChannelPermissions {
    servers: HashMap<String, ServerPermissions>,
}

impl ChannelPermissions {
    pub fn new() -> Self {
        Self {
            servers: HashMap::new(),
        }
    }

    pub fn from_config(servers: HashMap<String, ServerPermissions>) -> Self {
        Self { servers }
    }

    /// Check whether a tool call is allowed for the given server.
    pub fn is_tool_allowed(&self, server: &str, tool: &str) -> bool {
        let Some(perms) = self.servers.get(server) else {
            return true; // No rules → default allow
        };
        check_allowed(tool, &perms.allowed_tools, &perms.denied_tools)
    }

    /// Check whether a resource access is allowed for the given server.
    pub fn is_resource_allowed(&self, server: &str, resource: &str) -> bool {
        let Some(perms) = self.servers.get(server) else {
            return true;
        };
        check_allowed(resource, &perms.allowed_resources, &perms.denied_resources)
    }

    pub fn set_server_permissions(&mut self, server: String, perms: ServerPermissions) {
        self.servers.insert(server, perms);
    }

    pub fn remove_server(&mut self, server: &str) {
        self.servers.remove(server);
    }

    pub fn get_server_permissions(&self, server: &str) -> Option<&ServerPermissions> {
        self.servers.get(server)
    }
}

/// Check if a name is allowed by the allow/deny pattern lists.
/// Deny takes precedence. Empty allow list = allow all.
fn check_allowed(name: &str, allowed: &[String], denied: &[String]) -> bool {
    // Deny takes precedence
    if denied.iter().any(|p| glob_match(p, name)) {
        return false;
    }
    // Empty allow list = allow all
    if allowed.is_empty() {
        return true;
    }
    // Must match at least one allow pattern
    allowed.iter().any(|p| glob_match(p, name))
}

/// Simple glob matching supporting `*` wildcard.
fn glob_match(pattern: &str, input: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return input.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return input.ends_with(suffix);
    }
    pattern == input
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
        assert!(perms.is_tool_allowed("any-server", "any-tool"));
        assert!(perms.is_resource_allowed("any-server", "any-resource"));
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

    #[test]
    fn deny_takes_precedence() {
        let mut cp = ChannelPermissions::new();
        cp.set_server_permissions(
            "srv".into(),
            ServerPermissions {
                allowed_tools: vec!["*".into()],
                denied_tools: vec!["dangerous_tool".into()],
                ..Default::default()
            },
        );
        assert!(cp.is_tool_allowed("srv", "safe_tool"));
        assert!(!cp.is_tool_allowed("srv", "dangerous_tool"));
    }

    #[test]
    fn allow_list_restricts() {
        let mut cp = ChannelPermissions::new();
        cp.set_server_permissions(
            "srv".into(),
            ServerPermissions {
                allowed_tools: vec!["read_*".into()],
                ..Default::default()
            },
        );
        assert!(cp.is_tool_allowed("srv", "read_file"));
        assert!(!cp.is_tool_allowed("srv", "write_file"));
    }

    #[test]
    fn resource_permissions() {
        let mut cp = ChannelPermissions::new();
        cp.set_server_permissions(
            "srv".into(),
            ServerPermissions {
                denied_resources: vec!["secret://*".into()],
                ..Default::default()
            },
        );
        assert!(cp.is_resource_allowed("srv", "file://readme"));
        // Exact match only for our simple glob
        assert!(!cp.is_resource_allowed("srv", "secret://key"));
    }
}
