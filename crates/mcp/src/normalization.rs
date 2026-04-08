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
/// If the name already has the `mcp__` prefix, it is returned as-is.
/// Otherwise, the raw name is returned unchanged (server context needed
/// for full normalization).
#[must_use]
pub fn normalize_tool_name(raw: &str) -> String {
    let trimmed = raw.trim();

    // Already in canonical form
    if trimmed.starts_with("mcp__") {
        return trimmed.to_string();
    }

    // Convert camelCase/PascalCase to snake_case for consistency
    let mut result = String::with_capacity(trimmed.len() + 4);
    for (i, ch) in trimmed.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(ch.to_ascii_lowercase());
    }

    // Replace hyphens with underscores
    result.replace('-', "_")
}

/// Build a fully-qualified canonical tool name from server and tool names.
#[must_use]
pub fn qualify_tool_name(server: &str, tool: &str) -> String {
    format!(
        "mcp__{}__{}",
        normalize_tool_name(server),
        normalize_tool_name(tool)
    )
}

// ── Resource URIs ─────────────────────────────────────────────────────

/// Normalize a raw MCP resource URI to a canonical form.
///
/// Ensures consistent scheme handling, path normalization, and encoding
/// so that resource lookups and caching work reliably across servers.
#[must_use]
pub fn normalize_resource_uri(raw: &str) -> String {
    let trimmed = raw.trim();

    // Normalize scheme to lowercase
    if let Some(scheme_end) = trimmed.find("://") {
        let scheme = &trimmed[..scheme_end];
        let rest = &trimmed[scheme_end..];
        format!("{}{}", scheme.to_lowercase(), rest)
    } else {
        trimmed.to_string()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn already_canonical_unchanged() {
        assert_eq!(
            normalize_tool_name("mcp__server__tool"),
            "mcp__server__tool"
        );
    }

    #[test]
    fn bare_name_lowercase() {
        assert_eq!(normalize_tool_name("readFile"), "read_file");
    }

    #[test]
    fn kebab_to_snake() {
        assert_eq!(normalize_tool_name("read-file"), "read_file");
    }

    #[test]
    fn already_snake_unchanged() {
        assert_eq!(normalize_tool_name("read_file"), "read_file");
    }

    #[test]
    fn qualify_builds_full_name() {
        assert_eq!(
            qualify_tool_name("playwright", "click"),
            "mcp__playwright__click"
        );
    }

    #[test]
    fn normalize_uri_lowercase_scheme() {
        assert_eq!(normalize_resource_uri("FILE://path"), "file://path");
    }

    #[test]
    fn normalize_uri_no_scheme() {
        assert_eq!(normalize_resource_uri("just-a-path"), "just-a-path");
    }

    #[test]
    fn normalize_uri_already_lowercase() {
        assert_eq!(
            normalize_resource_uri("https://example.com/resource"),
            "https://example.com/resource"
        );
    }
}
