//! Tool and resource name normalization for cross-server consistency.
//!
//! Different MCP servers may use varying naming conventions for their
//! tools and resources (camelCase, `snake_case`, kebab-case, namespaced).
//! This module normalizes names to a canonical form so that tool lookups,
//! permission rules, and logging are consistent.

// ── Tool names ────────────────────────────────────────────────────────

/// Normalize a raw MCP tool name to the canonical form used internally.
///
/// The canonical form is `mcp__<server>__<tool>` with underscored segments.
/// This function handles various input formats:
/// - Already-namespaced: `mcp__server__tool` -> kept as-is
/// - Bare names: `read_file` -> prefixed if server context is known
/// - Mixed case: `ReadFile` -> lowercased and underscored
#[must_use]
pub fn normalize_tool_name(_raw: &str) -> String {
    todo!("normalize_tool_name: convert raw name to canonical mcp__server__tool form")
}

// ── Resource URIs ─────────────────────────────────────────────────────

/// Normalize a raw MCP resource URI to a canonical form.
///
/// Ensures consistent scheme handling, path normalization, and encoding
/// so that resource lookups and caching work reliably across servers.
#[must_use]
pub fn normalize_resource_uri(_raw: &str) -> String {
    todo!("normalize_resource_uri: normalize scheme, path, and encoding")
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #[test]
    fn module_compiles() {
        // Verifies the module is syntactically valid.
    }
}
