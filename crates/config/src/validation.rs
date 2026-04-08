//! Settings schema validation.
//!
//! Validates settings JSON values against expected types, ranges, and
//! inter-field constraints. Returns all errors found (not just the first),
//! with optional suggestions for common mistakes.

use std::fmt;

/// A single validation error with optional remediation suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// Dot-separated field path, e.g. `"mcpServers.myServer.command"`.
    pub field: String,
    /// Human-readable error message.
    pub message: String,
    /// Optional suggestion for how to fix the error.
    pub suggestion: Option<String>,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)?;
        if let Some(ref suggestion) = self.suggestion {
            write!(f, " (suggestion: {suggestion})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationError {}

/// Validate an entire settings JSON value.
///
/// Checks:
/// - Known field names and types (string, number, bool, object, array)
/// - `maxTokens` is a positive integer within a sane range
/// - `apiProvider` is one of the recognized providers
/// - `permissionMode` is a valid mode string
/// - `mcpServers` entries have required `command` or `url` fields
/// - `hooks` entries have valid trigger types
///
/// Returns an empty `Vec` if the settings are valid.
pub fn validate_settings(_settings: &serde_json::Value) -> Vec<ValidationError> {
    todo!()
}

/// Validate a single permission rule string.
///
/// Permission rules use a glob-like syntax:
/// `tool_name:pattern` where pattern may contain `*` wildcards.
///
/// # Errors
///
/// Returns `ValidationError` if the rule is syntactically invalid.
pub fn validate_permission_rule(_rule: &str) -> Result<(), ValidationError> {
    todo!()
}

/// Validate an MCP server configuration object.
///
/// Checks that required fields (`command` for stdio, `url` for SSE/WS) are present
/// and have correct types.
pub fn validate_mcp_server_config(
    _server_name: &str,
    _config: &serde_json::Value,
) -> Vec<ValidationError> {
    todo!()
}

/// Validate a hook configuration entry.
///
/// Checks that the trigger type is known and the command is non-empty.
pub fn validate_hook_entry(_entry: &serde_json::Value) -> Vec<ValidationError> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_error_display() {
        let err = ValidationError {
            field: "maxTokens".into(),
            message: "must be a positive integer".into(),
            suggestion: Some("use a value like 4096".into()),
        };
        let s = err.to_string();
        assert!(s.contains("maxTokens"));
        assert!(s.contains("positive integer"));
        assert!(s.contains("4096"));
    }

    #[test]
    fn validation_error_display_no_suggestion() {
        let err = ValidationError {
            field: "model".into(),
            message: "must be a string".into(),
            suggestion: None,
        };
        let s = err.to_string();
        assert!(s.contains("model"));
        assert!(!s.contains("suggestion"));
    }
}
