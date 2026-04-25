//! Config schema validation.
//!
//! Validates config JSON values against expected types, ranges, and
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

/// Known top-level config fields and their expected types.
///
/// Must stay in sync with [`crate::config::Config`] (camelCase).
const KNOWN_FIELDS: &[(&str, &str)] = &[
    // ── Schema / metadata ──
    ("$schema", "string"),
    ("schemaVersion", "number"),
    // ── Provider / auth ──
    ("apiProvider", "string"),
    ("apiBaseUrl", "string"),
    ("apiKeyHelper", "string"),
    // ── Model ──
    ("model", "string"),
    ("smallModel", "string"),
    ("advisorModel", "string"),
    ("availableModels", "array"),
    ("modelOverrides", "object"),
    ("maxTokens", "number"),
    // ── Permissions ──
    ("permissions", "object"),
    ("permissionMode", "string"),
    // ── Prompts / instructions ──
    ("systemPrompt", "string"),
    ("includeGitInstructions", "boolean"),
    ("customInstructions", "string"),
    // ── MCP ──
    ("mcpServers", "object"),
    ("enableAllProjectMcpServers", "boolean"),
    // ── Hooks ──
    ("hooks", "object"),
    ("disableAllHooks", "boolean"),
    // ── Shell / environment ──
    ("defaultShell", "string"),
    ("env", "object"),
    // ── UI / display ──
    ("theme", "string"),
    ("language", "string"),
    ("outputStyle", "string"),
    // ── Git ──
    ("gitContext", "object"),
    ("respectGitignore", "boolean"),
    // ── Memory ──
    ("autoMemoryEnabled", "boolean"),
    ("autoMemoryDirectory", "string"),
    // ── Misc ──
    ("cleanupPeriodDays", "number"),
];

/// Valid permission mode values.
const VALID_PERMISSION_MODES: &[&str] = &[
    "default",
    "acceptEdits",
    "accept-edits",
    "trust-project",
    "dontAsk",
    "dont-ask",
    "bypassPermissions",
    "dangerously",
    "plan",
];

/// Valid API provider values.
const VALID_PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "ollama",
    "bedrock",
    "vertex",
    "deepseek",
    "custom",
];

/// Validate an entire config JSON value.
///
/// Returns an empty `Vec` if the config is valid.
pub fn validate_config(config: &serde_json::Value) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    let Some(obj) = config.as_object() else {
        errors.push(ValidationError {
            field: "<root>".into(),
            message: "config must be a JSON object".into(),
            suggestion: None,
        });
        return errors;
    };

    // Check for unknown fields
    for key in obj.keys() {
        if !KNOWN_FIELDS.iter().any(|(k, _)| *k == key.as_str()) {
            errors.push(ValidationError {
                field: key.clone(),
                message: format!("unknown settings field '{key}'"),
                suggestion: Some("check spelling or remove this field".into()),
            });
        }
    }

    // Type checks for known fields (null is always valid — means "not set")
    for &(field, expected_type) in KNOWN_FIELDS {
        if let Some(value) = obj.get(field) {
            if value.is_null() {
                continue;
            }
            let type_ok = match expected_type {
                "string" => value.is_string(),
                "number" => value.is_number(),
                "boolean" => value.is_boolean(),
                "object" => value.is_object(),
                "array" => value.is_array(),
                _ => true,
            };
            if !type_ok {
                errors.push(ValidationError {
                    field: field.into(),
                    message: format!("expected {expected_type}, got {}", value_type_name(value)),
                    suggestion: None,
                });
            }
        }
    }

    // Validate maxTokens range
    if let Some(max_tokens) = obj.get("maxTokens").and_then(serde_json::Value::as_u64)
        && (max_tokens == 0 || max_tokens > 1_000_000)
    {
        errors.push(ValidationError {
            field: "maxTokens".into(),
            message: "must be between 1 and 1,000,000".into(),
            suggestion: Some("typical values: 4096, 8192, 16384".into()),
        });
    }

    // Validate permissionMode
    if let Some(mode) = obj.get("permissionMode").and_then(|v| v.as_str())
        && !VALID_PERMISSION_MODES.contains(&mode)
    {
        errors.push(ValidationError {
            field: "permissionMode".into(),
            message: format!("unknown permission mode '{mode}'"),
            suggestion: Some(format!(
                "valid modes: {}",
                VALID_PERMISSION_MODES.join(", ")
            )),
        });
    }

    // Validate apiProvider
    if let Some(provider) = obj.get("apiProvider").and_then(|v| v.as_str())
        && !VALID_PROVIDERS.contains(&provider)
    {
        errors.push(ValidationError {
            field: "apiProvider".into(),
            message: format!("unknown API provider '{provider}'"),
            suggestion: Some(format!("valid providers: {}", VALID_PROVIDERS.join(", "))),
        });
    }

    // Validate mcpServers entries
    if let Some(servers) = obj.get("mcpServers").and_then(|v| v.as_object()) {
        for (name, config) in servers {
            errors.extend(validate_mcp_server_config(name, config));
        }
    }

    // Validate hooks
    if let Some(hooks) = obj.get("hooks").and_then(|v| v.as_object()) {
        for (trigger, entries) in hooks {
            if let Some(arr) = entries.as_array() {
                for entry in arr {
                    errors.extend(validate_hook_entry_inner(trigger, entry));
                }
            }
        }
    }

    errors
}

/// Validate a single permission rule string.
pub fn validate_permission_rule(rule: &str) -> Result<(), ValidationError> {
    let rule = rule.trim();
    if rule.is_empty() {
        return Err(ValidationError {
            field: "permission_rule".into(),
            message: "rule cannot be empty".into(),
            suggestion: None,
        });
    }

    // Check for common syntax errors
    if rule.contains('(') && !rule.contains(')') {
        return Err(ValidationError {
            field: "permission_rule".into(),
            message: format!("unmatched '(' in rule '{rule}'"),
            suggestion: Some("close with ')' e.g. Bash(command:git*)".into()),
        });
    }

    if !rule.contains('(') && rule.contains(')') {
        return Err(ValidationError {
            field: "permission_rule".into(),
            message: format!("unmatched ')' in rule '{rule}'"),
            suggestion: None,
        });
    }

    Ok(())
}

/// Validate an MCP server configuration object.
pub fn validate_mcp_server_config(
    server_name: &str,
    config: &serde_json::Value,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let prefix = format!("mcpServers.{server_name}");

    let Some(obj) = config.as_object() else {
        errors.push(ValidationError {
            field: prefix,
            message: "server config must be an object".into(),
            suggestion: None,
        });
        return errors;
    };

    // Must have either "command" (stdio) or "url" (SSE/WebSocket)
    let has_command = obj.get("command").and_then(|v| v.as_str()).is_some();
    let has_url = obj.get("url").and_then(|v| v.as_str()).is_some();

    if !has_command && !has_url {
        errors.push(ValidationError {
            field: prefix,
            message: "must have either 'command' (stdio) or 'url' (SSE/WebSocket)".into(),
            suggestion: Some("add command: \"npx server\" or url: \"http://...\"".into()),
        });
    }

    errors
}

/// Validate a hook configuration entry.
pub fn validate_hook_entry(entry: &serde_json::Value) -> Vec<ValidationError> {
    validate_hook_entry_inner("hooks", entry)
}

fn validate_hook_entry_inner(trigger: &str, entry: &serde_json::Value) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    let Some(obj) = entry.as_object() else {
        errors.push(ValidationError {
            field: format!("hooks.{trigger}"),
            message: "hook entry must be an object".into(),
            suggestion: None,
        });
        return errors;
    };

    // Must have "command" field
    if let Some(cmd) = obj.get("command") {
        if let Some(s) = cmd.as_str() {
            if s.trim().is_empty() {
                errors.push(ValidationError {
                    field: format!("hooks.{trigger}.command"),
                    message: "command cannot be empty".into(),
                    suggestion: None,
                });
            }
        } else {
            errors.push(ValidationError {
                field: format!("hooks.{trigger}.command"),
                message: "command must be a string".into(),
                suggestion: None,
            });
        }
    }

    errors
}

/// Validate a raw config file from disk.
///
/// Reads the file, parses TOML, and validates the resulting object.
/// Each error's `field` is prefixed with the source label (e.g. `[global]`).
/// Returns an empty `Vec` if the file does not exist or is empty.
pub fn validate_raw_file(path: &std::path::Path, source_label: &str) -> Vec<ValidationError> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            return vec![ValidationError {
                field: format!("[{source_label}]"),
                message: format!("cannot read file: {e}"),
                suggestion: None,
            }];
        }
    };

    let content = content.trim();
    if content.is_empty() {
        return Vec::new();
    }

    let toml_value: toml::Value = match toml::from_str(content) {
        Ok(v) => v,
        Err(e) => {
            return vec![ValidationError {
                field: format!("[{source_label}]"),
                message: format!("parse error: {e}"),
                suggestion: None,
            }];
        }
    };

    let json = crate::config::toml_value_to_json_for_validation(toml_value);

    validate_config(&json)
        .into_iter()
        .map(|mut e| {
            e.field = format!("[{source_label}] {}", e.field);
            e
        })
        .collect()
}

/// Validate all config files in the merge chain.
///
/// Validates each raw file independently so that `Option::None` fields
/// (absent from the file) never appear as false-positive `null` errors.
/// Returns warnings with source-prefixed field paths.
pub fn validate_all_config_files(project_dir: Option<&std::path::Path>) -> Vec<ValidationError> {
    let mut warnings = Vec::new();

    let global_path = crate::config::global_config_dir().join(crate::config::config_file_name());
    warnings.extend(validate_raw_file(&global_path, "global"));

    if let Some(dir) = project_dir {
        let project_path =
            crate::config::project_config_dir(dir).join(crate::config::config_file_name());
        warnings.extend(validate_raw_file(&project_path, "project"));

        let local_path =
            crate::config::project_config_dir(dir).join(crate::config::local_config_file_name());
        warnings.extend(validate_raw_file(&local_path, "local"));
    }

    warnings
}

/// Get a human-readable type name for a JSON value.
fn value_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
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

    #[test]
    fn validate_valid_settings() {
        let settings = serde_json::json!({
            "model": "claude-3-sonnet",
            "maxTokens": 4096,
            "permissionMode": "default"
        });
        let errors = validate_config(&settings);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn validate_unknown_field() {
        let settings = serde_json::json!({"unknownField": true});
        let errors = validate_config(&settings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("unknown"));
    }

    #[test]
    fn validate_wrong_type() {
        let settings = serde_json::json!({"maxTokens": "not a number"});
        let errors = validate_config(&settings);
        assert!(!errors.is_empty());
        assert!(errors[0].message.contains("expected number"));
    }

    #[test]
    fn validate_bad_permission_mode() {
        let settings = serde_json::json!({"permissionMode": "invalid"});
        let errors = validate_config(&settings);
        assert!(!errors.is_empty());
        assert!(errors[0].message.contains("unknown permission mode"));
    }

    #[test]
    fn validate_mcp_server_no_command_or_url() {
        let errors = validate_mcp_server_config("test", &serde_json::json!({"args": []}));
        assert!(!errors.is_empty());
        assert!(errors[0].message.contains("command"));
    }

    #[test]
    fn validate_mcp_server_with_command() {
        let errors =
            validate_mcp_server_config("test", &serde_json::json!({"command": "npx server"}));
        assert!(errors.is_empty());
    }

    #[test]
    fn validate_permission_rule_valid() {
        assert!(validate_permission_rule("Bash(command:git*)").is_ok());
        assert!(validate_permission_rule("*").is_ok());
        assert!(validate_permission_rule("Edit").is_ok());
    }

    #[test]
    fn validate_permission_rule_unmatched_paren() {
        assert!(validate_permission_rule("Bash(command:git*").is_err());
    }

    #[test]
    fn validate_permission_rule_empty() {
        assert!(validate_permission_rule("").is_err());
    }

    #[test]
    fn validate_hook_entry_valid() {
        let entry = serde_json::json!({"command": "echo check"});
        let errors = validate_hook_entry(&entry);
        assert!(errors.is_empty());
    }

    #[test]
    fn validate_hook_entry_empty_command() {
        let entry = serde_json::json!({"command": ""});
        let errors = validate_hook_entry(&entry);
        assert!(!errors.is_empty());
    }

    #[test]
    fn validate_null_values_are_valid() {
        let settings = serde_json::json!({
            "apiKeyHelper": null,
            "smallModel": null,
            "maxTokens": null
        });
        let errors = validate_config(&settings);
        assert!(errors.is_empty(), "null values should be valid: {errors:?}");
    }

    #[test]
    fn validate_api_key_is_unknown_field() {
        let settings = serde_json::json!({"apiKey": "sk-test"});
        let errors = validate_config(&settings);
        assert!(
            !errors.is_empty(),
            "apiKey is no longer a Config field; secrets must go through the auth chain"
        );
    }

    #[test]
    fn validate_non_object_root() {
        let errors = validate_config(&serde_json::json!("not an object"));
        assert!(!errors.is_empty());
    }
}
