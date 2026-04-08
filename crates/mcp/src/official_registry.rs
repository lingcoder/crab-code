//! Official MCP server registry: well-known servers with default configs.
//!
//! Provides a catalog of officially supported MCP servers (e.g. Playwright,
//! filesystem, GitHub) with their default transport configurations. Users
//! can reference servers by name instead of specifying full config.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Types ─────────────────────────────────────────────────────────────

/// An entry in the official MCP server registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    /// Short canonical name (e.g. "playwright", "filesystem").
    pub name: String,
    /// Human-readable description of the server's capabilities.
    pub description: String,
    /// Default MCP transport configuration for this server.
    pub default_config: Value,
}

// ── Built-in registry ─────────────────────────────────────────────────

/// The built-in list of known MCP servers.
fn builtin_registry() -> Vec<RegistryEntry> {
    vec![
        RegistryEntry {
            name: "playwright".into(),
            description: "Browser automation via Playwright".into(),
            default_config: serde_json::json!({
                "command": "npx",
                "args": ["@anthropic-ai/mcp-playwright"]
            }),
        },
        RegistryEntry {
            name: "filesystem".into(),
            description: "Local filesystem access (read/write/search)".into(),
            default_config: serde_json::json!({
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-filesystem"]
            }),
        },
        RegistryEntry {
            name: "github".into(),
            description: "GitHub API integration (issues, PRs, repos)".into(),
            default_config: serde_json::json!({
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-github"]
            }),
        },
        RegistryEntry {
            name: "postgres".into(),
            description: "PostgreSQL database access".into(),
            default_config: serde_json::json!({
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-postgres"]
            }),
        },
        RegistryEntry {
            name: "brave-search".into(),
            description: "Web search via Brave Search API".into(),
            default_config: serde_json::json!({
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-brave-search"]
            }),
        },
    ]
}

// ── Lookup ────────────────────────────────────────────────────────────

/// Look up an official server by its canonical name.
#[must_use]
pub fn lookup_server(name: &str) -> Option<RegistryEntry> {
    let lower = name.to_lowercase();
    builtin_registry().into_iter().find(|e| e.name == lower)
}

/// List all officially registered MCP servers.
#[must_use]
pub fn list_official_servers() -> Vec<RegistryEntry> {
    let mut servers = builtin_registry();
    servers.sort_by(|a, b| a.name.cmp(&b.name));
    servers
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_entry_serde_roundtrip() {
        let entry = RegistryEntry {
            name: "playwright".into(),
            description: "Browser automation via Playwright".into(),
            default_config: serde_json::json!({"command": "npx", "args": ["@anthropic-ai/mcp-playwright"]}),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: RegistryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "playwright");
    }

    #[test]
    fn lookup_known_server() {
        assert!(lookup_server("playwright").is_some());
        assert!(lookup_server("filesystem").is_some());
        assert!(lookup_server("github").is_some());
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(lookup_server("nonexistent").is_none());
    }

    #[test]
    fn list_returns_sorted() {
        let servers = list_official_servers();
        assert!(servers.len() >= 5);
        // Verify sorted
        for w in servers.windows(2) {
            assert!(w[0].name <= w[1].name);
        }
    }
}
