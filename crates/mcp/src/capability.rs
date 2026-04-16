//! MCP capability declaration and negotiation.
//!
//! Provides detailed capability structures for both server and client,
//! capability intersection logic, and a dynamic [`CapabilityRegistry`].

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Metadata associated with a capability.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilityMeta {
    /// Whether list-changed notifications are supported.
    #[serde(default)]
    pub list_changed: bool,
    /// Whether subscribe/unsubscribe is supported (resources).
    #[serde(default)]
    pub subscribe: bool,
    /// Additional key-value metadata.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, String>,
}

impl CapabilityMeta {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_list_changed(mut self) -> Self {
        self.list_changed = true;
        self
    }

    #[must_use]
    pub fn with_subscribe(mut self) -> Self {
        self.subscribe = true;
        self
    }
}

/// A single capability entry: enabled flag + metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityEntry {
    pub enabled: bool,
    #[serde(default)]
    pub meta: CapabilityMeta,
}

impl CapabilityEntry {
    #[must_use]
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            meta: CapabilityMeta::default(),
        }
    }

    #[must_use]
    pub fn enabled_with_meta(meta: CapabilityMeta) -> Self {
        Self {
            enabled: true,
            meta,
        }
    }

    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            meta: CapabilityMeta::default(),
        }
    }
}

/// Server-side capability declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerCapabilities {
    pub tools: CapabilityEntry,
    pub resources: CapabilityEntry,
    pub prompts: CapabilityEntry,
    pub sampling: CapabilityEntry,
    pub roots: CapabilityEntry,
    pub logging: CapabilityEntry,
}

impl Default for McpServerCapabilities {
    fn default() -> Self {
        Self {
            tools: CapabilityEntry::enabled(),
            resources: CapabilityEntry::disabled(),
            prompts: CapabilityEntry::disabled(),
            sampling: CapabilityEntry::disabled(),
            roots: CapabilityEntry::disabled(),
            logging: CapabilityEntry::disabled(),
        }
    }
}

impl McpServerCapabilities {
    /// Create with all capabilities enabled.
    #[must_use]
    pub fn all() -> Self {
        Self {
            tools: CapabilityEntry::enabled(),
            resources: CapabilityEntry::enabled(),
            prompts: CapabilityEntry::enabled(),
            sampling: CapabilityEntry::enabled(),
            roots: CapabilityEntry::enabled(),
            logging: CapabilityEntry::enabled(),
        }
    }

    /// List enabled capability names.
    #[must_use]
    pub fn enabled_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.tools.enabled {
            names.push("tools");
        }
        if self.resources.enabled {
            names.push("resources");
        }
        if self.prompts.enabled {
            names.push("prompts");
        }
        if self.sampling.enabled {
            names.push("sampling");
        }
        if self.roots.enabled {
            names.push("roots");
        }
        if self.logging.enabled {
            names.push("logging");
        }
        names
    }
}

/// Client-side capability declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpClientCapabilities {
    pub tools: CapabilityEntry,
    pub resources: CapabilityEntry,
    pub prompts: CapabilityEntry,
    pub sampling: CapabilityEntry,
    pub roots: CapabilityEntry,
}

impl Default for McpClientCapabilities {
    fn default() -> Self {
        Self {
            tools: CapabilityEntry::enabled(),
            resources: CapabilityEntry::enabled(),
            prompts: CapabilityEntry::enabled(),
            sampling: CapabilityEntry::disabled(),
            roots: CapabilityEntry::enabled(),
        }
    }
}

impl McpClientCapabilities {
    /// List enabled capability names.
    #[must_use]
    pub fn enabled_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.tools.enabled {
            names.push("tools");
        }
        if self.resources.enabled {
            names.push("resources");
        }
        if self.prompts.enabled {
            names.push("prompts");
        }
        if self.sampling.enabled {
            names.push("sampling");
        }
        if self.roots.enabled {
            names.push("roots");
        }
        names
    }
}

/// The intersection of server and client capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct NegotiatedCapabilities {
    pub tools: bool,
    pub resources: bool,
    pub prompts: bool,
    pub sampling: bool,
    pub roots: bool,
    pub logging: bool,
}

impl NegotiatedCapabilities {
    /// Compute the intersection of server and client capabilities.
    #[must_use]
    pub fn negotiate(server: &McpServerCapabilities, client: &McpClientCapabilities) -> Self {
        Self {
            tools: server.tools.enabled && client.tools.enabled,
            resources: server.resources.enabled && client.resources.enabled,
            prompts: server.prompts.enabled && client.prompts.enabled,
            sampling: server.sampling.enabled && client.sampling.enabled,
            roots: server.roots.enabled && client.roots.enabled,
            // Logging is server-only; if server offers it, it's available
            logging: server.logging.enabled,
        }
    }

    /// List the negotiated (active) capability names.
    #[must_use]
    pub fn active_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.tools {
            names.push("tools");
        }
        if self.resources {
            names.push("resources");
        }
        if self.prompts {
            names.push("prompts");
        }
        if self.sampling {
            names.push("sampling");
        }
        if self.roots {
            names.push("roots");
        }
        if self.logging {
            names.push("logging");
        }
        names
    }

    /// Whether a specific capability is active.
    #[must_use]
    pub fn is_active(&self, name: &str) -> bool {
        match name {
            "tools" => self.tools,
            "resources" => self.resources,
            "prompts" => self.prompts,
            "sampling" => self.sampling,
            "roots" => self.roots,
            "logging" => self.logging,
            _ => false,
        }
    }
}

/// Dynamic capability registry for runtime enable/disable.
#[derive(Debug, Default)]
pub struct CapabilityRegistry {
    capabilities: HashMap<String, CapabilityEntry>,
}

impl CapabilityRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a capability.
    pub fn register(&mut self, name: impl Into<String>, entry: CapabilityEntry) {
        self.capabilities.insert(name.into(), entry);
    }

    /// Enable a capability. Returns false if not registered.
    pub fn enable(&mut self, name: &str) -> bool {
        if let Some(entry) = self.capabilities.get_mut(name) {
            entry.enabled = true;
            true
        } else {
            false
        }
    }

    /// Disable a capability. Returns false if not registered.
    pub fn disable(&mut self, name: &str) -> bool {
        if let Some(entry) = self.capabilities.get_mut(name) {
            entry.enabled = false;
            true
        } else {
            false
        }
    }

    /// Check if a capability is enabled.
    #[must_use]
    pub fn is_enabled(&self, name: &str) -> bool {
        self.capabilities.get(name).is_some_and(|e| e.enabled)
    }

    /// Get a capability entry.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&CapabilityEntry> {
        self.capabilities.get(name)
    }

    /// List all enabled capability names.
    #[must_use]
    pub fn enabled_names(&self) -> Vec<&str> {
        self.capabilities
            .iter()
            .filter(|(_, e)| e.enabled)
            .map(|(n, _)| n.as_str())
            .collect()
    }

    /// Number of registered capabilities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.capabilities.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_meta_builders() {
        let m = CapabilityMeta::new().with_list_changed().with_subscribe();
        assert!(m.list_changed);
        assert!(m.subscribe);
    }

    #[test]
    fn capability_entry_enabled_disabled() {
        assert!(CapabilityEntry::enabled().enabled);
        assert!(!CapabilityEntry::disabled().enabled);
    }

    #[test]
    fn server_capabilities_default() {
        let sc = McpServerCapabilities::default();
        assert!(sc.tools.enabled);
        assert!(!sc.resources.enabled);
        assert!(!sc.prompts.enabled);
        assert_eq!(sc.enabled_names(), vec!["tools"]);
    }

    #[test]
    fn server_capabilities_all() {
        let sc = McpServerCapabilities::all();
        assert_eq!(sc.enabled_names().len(), 6);
    }

    #[test]
    fn client_capabilities_default() {
        let cc = McpClientCapabilities::default();
        assert!(cc.tools.enabled);
        assert!(cc.resources.enabled);
        assert!(!cc.sampling.enabled);
    }

    #[test]
    fn negotiate_capabilities_intersection() {
        let server = McpServerCapabilities {
            tools: CapabilityEntry::enabled(),
            resources: CapabilityEntry::enabled(),
            prompts: CapabilityEntry::disabled(),
            sampling: CapabilityEntry::enabled(),
            roots: CapabilityEntry::disabled(),
            logging: CapabilityEntry::enabled(),
        };
        let client = McpClientCapabilities {
            tools: CapabilityEntry::enabled(),
            resources: CapabilityEntry::disabled(),
            prompts: CapabilityEntry::enabled(),
            sampling: CapabilityEntry::enabled(),
            roots: CapabilityEntry::enabled(),
        };
        let neg = NegotiatedCapabilities::negotiate(&server, &client);
        assert!(neg.tools);
        assert!(!neg.resources); // server yes, client no
        assert!(!neg.prompts); // server no, client yes
        assert!(neg.sampling); // both yes
        assert!(!neg.roots); // server no
        assert!(neg.logging); // server-only
    }

    #[test]
    fn negotiated_active_names() {
        let neg = NegotiatedCapabilities {
            tools: true,
            resources: false,
            prompts: true,
            sampling: false,
            roots: false,
            logging: true,
        };
        let names = neg.active_names();
        assert_eq!(names, vec!["tools", "prompts", "logging"]);
    }

    #[test]
    fn negotiated_is_active() {
        let neg = NegotiatedCapabilities {
            tools: true,
            resources: false,
            prompts: false,
            sampling: false,
            roots: false,
            logging: false,
        };
        assert!(neg.is_active("tools"));
        assert!(!neg.is_active("resources"));
        assert!(!neg.is_active("unknown"));
    }

    #[test]
    fn negotiated_serde_roundtrip() {
        let neg = NegotiatedCapabilities {
            tools: true,
            resources: true,
            prompts: false,
            sampling: false,
            roots: true,
            logging: false,
        };
        let json = serde_json::to_string(&neg).unwrap();
        let back: NegotiatedCapabilities = serde_json::from_str(&json).unwrap();
        assert!(back.tools);
        assert!(!back.prompts);
    }

    #[test]
    fn capability_registry_basic() {
        let mut reg = CapabilityRegistry::new();
        reg.register("tools", CapabilityEntry::enabled());
        reg.register("sampling", CapabilityEntry::disabled());
        assert_eq!(reg.len(), 2);
        assert!(reg.is_enabled("tools"));
        assert!(!reg.is_enabled("sampling"));
    }

    #[test]
    fn capability_registry_enable_disable() {
        let mut reg = CapabilityRegistry::new();
        reg.register("tools", CapabilityEntry::enabled());
        assert!(reg.disable("tools"));
        assert!(!reg.is_enabled("tools"));
        assert!(reg.enable("tools"));
        assert!(reg.is_enabled("tools"));
        assert!(!reg.enable("nonexistent"));
    }

    #[test]
    fn capability_registry_enabled_names() {
        let mut reg = CapabilityRegistry::new();
        reg.register("tools", CapabilityEntry::enabled());
        reg.register("prompts", CapabilityEntry::enabled());
        reg.register("sampling", CapabilityEntry::disabled());
        let names = reg.enabled_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"tools"));
        assert!(names.contains(&"prompts"));
    }

    #[test]
    fn server_capabilities_serde_roundtrip() {
        let sc = McpServerCapabilities::all();
        let json = serde_json::to_string(&sc).unwrap();
        let back: McpServerCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(back.enabled_names().len(), 6);
    }

    #[test]
    fn client_capabilities_serde_roundtrip() {
        let cc = McpClientCapabilities::default();
        let json = serde_json::to_string(&cc).unwrap();
        let back: McpClientCapabilities = serde_json::from_str(&json).unwrap();
        assert!(back.tools.enabled);
    }
}
