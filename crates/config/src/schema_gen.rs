//! JSON Schema generation for `settings.json`.
//!
//! Produces a JSON Schema (draft-07) that describes all known configuration
//! fields so editors can provide validation and autocomplete.

use serde_json::{json, Value};

/// Generate a complete JSON Schema for `settings.json`.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn generate_settings_schema() -> Value {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": "Crab Code Settings",
        "description": "Configuration file for Crab Code (~/.crab/settings.json)",
        "type": "object",
        "properties": properties_schema(),
        "additionalProperties": true,
        "$defs": defs_schema()
    })
}

/// Top-level properties for the settings schema.
fn properties_schema() -> Value {
    json!({
        "configVersion": {
            "type": "integer",
            "description": "Schema version for config migration",
            "minimum": 0
        },
        "apiProvider": {
            "type": "string",
            "description": "LLM provider to use",
            "enum": ["anthropic", "openai", "deepseek", "ollama", "vllm", "bedrock", "vertex"],
            "default": "anthropic"
        },
        "apiBaseUrl": {
            "type": "string",
            "description": "Base URL for the LLM API endpoint",
            "format": "uri",
            "examples": ["https://api.anthropic.com", "http://localhost:11434/v1"]
        },
        "apiKey": {
            "type": "string",
            "description": "API key for the LLM provider (prefer env vars or keychain)"
        },
        "model": {
            "type": "string",
            "description": "Primary model to use for generation",
            "examples": ["claude-sonnet-4-20250514", "gpt-4o", "deepseek-chat"]
        },
        "smallModel": {
            "type": "string",
            "description": "Smaller/faster model for auxiliary tasks",
            "examples": ["claude-haiku-4-5-20251001", "gpt-4o-mini"]
        },
        "maxTokens": {
            "type": "integer",
            "description": "Maximum tokens per response",
            "minimum": 1,
            "maximum": 1_000_000,
            "default": 4096
        },
        "permissionMode": {
            "type": "string",
            "description": "Permission enforcement level",
            "enum": ["default", "trustProject", "dangerously"],
            "default": "default"
        },
        "systemPrompt": {
            "type": "string",
            "description": "Custom system prompt to prepend"
        },
        "mcpServers": {
            "type": "object",
            "description": "MCP server configurations",
            "additionalProperties": {
                "$ref": "#/$defs/McpServerConfig"
            }
        },
        "hooks": {
            "type": "object",
            "description": "Hook definitions triggered by lifecycle events",
            "additionalProperties": {
                "$ref": "#/$defs/HookDefinition"
            }
        },
        "theme": {
            "type": "string",
            "description": "UI color theme",
            "enum": ["auto", "dark", "light"],
            "default": "auto"
        }
    })
}

/// Shared sub-schema definitions (`$defs`).
fn defs_schema() -> Value {
    json!({
        "McpServerConfig": {
            "type": "object",
            "description": "Configuration for an MCP server",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Command to launch the MCP server"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Arguments for the command"
                },
                "env": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "description": "Environment variables"
                },
                "url": {
                    "type": "string",
                    "format": "uri",
                    "description": "URL for SSE/WebSocket transport (instead of command)"
                }
            },
            "additionalProperties": true
        },
        "HookDefinition": {
            "type": "object",
            "description": "A hook triggered by a lifecycle event",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds",
                    "minimum": 0,
                    "default": 10000
                }
            },
            "required": ["command"]
        }
    })
}

/// Return a list of all top-level property names defined in the schema.
#[must_use]
pub fn schema_property_names() -> Vec<&'static str> {
    vec![
        "configVersion",
        "apiProvider",
        "apiBaseUrl",
        "apiKey",
        "model",
        "smallModel",
        "maxTokens",
        "permissionMode",
        "systemPrompt",
        "mcpServers",
        "hooks",
        "theme",
    ]
}

/// Return the JSON Schema for a specific field, or `None` if unknown.
#[must_use]
pub fn field_schema(field: &str) -> Option<Value> {
    let schema = generate_settings_schema();
    schema
        .get("properties")
        .and_then(|props| props.get(field))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_valid_json_object() {
        let schema = generate_settings_schema();
        assert!(schema.is_object());
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn schema_has_title() {
        let schema = generate_settings_schema();
        assert_eq!(schema["title"], "Crab Code Settings");
    }

    #[test]
    fn schema_has_all_known_properties() {
        let schema = generate_settings_schema();
        let props = schema["properties"].as_object().unwrap();
        for name in schema_property_names() {
            assert!(props.contains_key(name), "missing property: {name}");
        }
    }

    #[test]
    fn schema_api_provider_enum() {
        let schema = generate_settings_schema();
        let providers = schema["properties"]["apiProvider"]["enum"]
            .as_array()
            .unwrap();
        assert!(providers.contains(&json!("anthropic")));
        assert!(providers.contains(&json!("openai")));
        assert!(providers.contains(&json!("deepseek")));
    }

    #[test]
    fn schema_permission_mode_enum() {
        let schema = generate_settings_schema();
        let modes = schema["properties"]["permissionMode"]["enum"]
            .as_array()
            .unwrap();
        assert!(modes.contains(&json!("default")));
        assert!(modes.contains(&json!("trustProject")));
        assert!(modes.contains(&json!("dangerously")));
    }

    #[test]
    fn schema_max_tokens_constraints() {
        let schema = generate_settings_schema();
        let mt = &schema["properties"]["maxTokens"];
        assert_eq!(mt["type"], "integer");
        assert_eq!(mt["minimum"], 1);
        assert_eq!(mt["maximum"], 1_000_000);
    }

    #[test]
    fn schema_has_defs() {
        let schema = generate_settings_schema();
        let defs = schema["$defs"].as_object().unwrap();
        assert!(defs.contains_key("McpServerConfig"));
        assert!(defs.contains_key("HookDefinition"));
    }

    #[test]
    fn schema_mcp_servers_ref() {
        let schema = generate_settings_schema();
        let additional = &schema["properties"]["mcpServers"]["additionalProperties"];
        assert_eq!(additional["$ref"], "#/$defs/McpServerConfig");
    }

    #[test]
    fn schema_property_names_count() {
        let names = schema_property_names();
        assert_eq!(names.len(), 12);
    }

    #[test]
    fn field_schema_known_field() {
        let fs = field_schema("apiProvider").unwrap();
        assert_eq!(fs["type"], "string");
        assert!(fs["enum"].is_array());
    }

    #[test]
    fn field_schema_unknown_field() {
        assert!(field_schema("nonexistent").is_none());
    }

    #[test]
    fn schema_theme_enum() {
        let schema = generate_settings_schema();
        let themes = schema["properties"]["theme"]["enum"]
            .as_array()
            .unwrap();
        assert!(themes.contains(&json!("auto")));
        assert!(themes.contains(&json!("dark")));
        assert!(themes.contains(&json!("light")));
    }

    #[test]
    fn schema_hook_definition_required_command() {
        let schema = generate_settings_schema();
        let hook_def = &schema["$defs"]["HookDefinition"];
        let required = hook_def["required"].as_array().unwrap();
        assert!(required.contains(&json!("command")));
    }
}
