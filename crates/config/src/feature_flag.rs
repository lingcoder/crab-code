//! Runtime feature flag management (local evaluation only).
//!
//! Feature flags allow enabling/disabling experimental or in-development
//! functionality at runtime without recompilation. All evaluation is local —
//! no remote flag service is contacted.
//!
//! Flags can be set from:
//! 1. Hard-coded defaults (see [`flags`] module for well-known flags)
//! 2. Settings files (`settings.json` `"featureFlags"` object)
//! 3. Programmatic override via [`FeatureFlags::set`]

use std::collections::HashMap;

/// Runtime feature flag store.
///
/// Wraps a `HashMap<String, bool>` with convenience methods for
/// flag lookup, bulk loading, and default initialization.
#[derive(Debug, Clone)]
pub struct FeatureFlags {
    flags: HashMap<String, bool>,
}

impl FeatureFlags {
    /// Create a `FeatureFlags` instance with all well-known flags set to their
    /// default values (typically `false` for experimental features).
    pub fn default_flags() -> Self {
        Self {
            flags: HashMap::new(),
        }
    }

    /// Load feature flags from a settings JSON value.
    ///
    /// Expects the `"featureFlags"` key to be an object of `{ "flag_name": bool }`.
    /// Unknown flags are preserved. Missing flags retain their defaults.
    pub fn load_from_settings(_settings: &serde_json::Value) -> Self {
        todo!()
    }

    /// Check whether a flag is enabled.
    ///
    /// Returns `false` for unknown flags.
    pub fn is_enabled(&self, flag: &str) -> bool {
        self.flags.get(flag).copied().unwrap_or(false)
    }

    /// Set a flag value, inserting it if it doesn't already exist.
    pub fn set(&mut self, flag: &str, enabled: bool) {
        self.flags.insert(flag.to_string(), enabled);
    }

    /// Return all currently registered flags and their values.
    pub fn all(&self) -> &HashMap<String, bool> {
        &self.flags
    }

    /// Merge another set of flags on top, overriding existing values.
    pub fn merge(&mut self, _other: &Self) {
        todo!()
    }
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self::default_flags()
    }
}

/// Well-known feature flag constants.
///
/// Use these constants when checking flags to avoid typo-related bugs:
/// ```rust,ignore
/// if flags.is_enabled(flags::WASM_PLUGINS) { /* ... */ }
/// ```
pub mod flags {
    /// Enable WASM plugin support.
    pub const WASM_PLUGINS: &str = "wasm_plugins";
    /// Enable MCP server authentication (`OAuth2` / API keys).
    pub const MCP_AUTH: &str = "mcp_auth";
    /// Enable shared team memory.
    pub const TEAM_MEMORY: &str = "team_memory";
    /// Enable automatic conversation compaction.
    pub const AUTO_COMPACT: &str = "auto_compact";
    /// Enable prompt suggestions in the chat input.
    pub const PROMPT_SUGGESTIONS: &str = "prompt_suggestions";
    /// Enable extended thinking display.
    pub const EXTENDED_THINKING: &str = "extended_thinking";
    /// Enable multi-turn tool use.
    pub const MULTI_TURN_TOOL_USE: &str = "multi_turn_tool_use";
    /// Enable streaming markdown rendering.
    pub const STREAMING_MARKDOWN: &str = "streaming_markdown";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_flag_returns_false() {
        let ff = FeatureFlags {
            flags: HashMap::new(),
        };
        // Unknown flags should not panic, just return false.
        assert!(!ff.is_enabled("nonexistent_flag"));
    }

    #[test]
    fn all_returns_reference() {
        let ff = FeatureFlags {
            flags: HashMap::from([("test".into(), true)]),
        };
        assert_eq!(ff.all().get("test"), Some(&true));
    }

    #[test]
    fn flag_constants_are_non_empty() {
        assert!(!flags::WASM_PLUGINS.is_empty());
        assert!(!flags::MCP_AUTH.is_empty());
        assert!(!flags::TEAM_MEMORY.is_empty());
        assert!(!flags::AUTO_COMPACT.is_empty());
        assert!(!flags::PROMPT_SUGGESTIONS.is_empty());
    }
}
