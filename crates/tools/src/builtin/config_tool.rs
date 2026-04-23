//! `ConfigTool` — programmatic settings.json read/write.
//!
//! Provides get, set, and list operations on the merged configuration,
//! allowing the LLM to inspect and modify settings at runtime.

use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Tool name constant for `ConfigTool`.
pub const CONFIG_TOOL_NAME: &str = "Config";

/// Programmatic settings read/write tool.
///
/// Input:
/// - `operation`: `"get"` | `"set"` | `"list"`
/// - `key`: Setting key path (dot-separated), required for get/set
/// - `value`: New value, required for set
pub struct ConfigTool;

impl Tool for ConfigTool {
    fn name(&self) -> &'static str {
        CONFIG_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Read, write, or list settings in the Crab Code configuration. \
         Use 'get' to read a setting by key, 'set' to update a setting, \
         or 'list' to show all current settings."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["get", "set", "list"],
                    "description": "The operation to perform"
                },
                "key": {
                    "type": "string",
                    "description": "Dot-separated settings key path (e.g. 'model.provider')"
                },
                "value": {
                    "description": "New value for the setting (required for 'set')"
                }
            },
            "required": ["operation"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        let operation = input["operation"].as_str().unwrap_or("").to_owned();
        let key = input.get("key").and_then(|v| v.as_str()).map(String::from);
        let value = input.get("value").cloned();

        Box::pin(async move {
            match operation.as_str() {
                "get" => {
                    let Some(key) = key else {
                        return Ok(ToolOutput::error("'key' is required for 'get' operation"));
                    };
                    get_setting(&key).await
                }
                "set" => {
                    let Some(key) = key else {
                        return Ok(ToolOutput::error("'key' is required for 'set' operation"));
                    };
                    let Some(value) = value else {
                        return Ok(ToolOutput::error("'value' is required for 'set' operation"));
                    };
                    set_setting(&key, &value).await
                }
                "list" => list_settings().await,
                other => Ok(ToolOutput::error(format!(
                    "unknown operation: '{other}'. Expected 'get', 'set', or 'list'"
                ))),
            }
        })
    }

    fn requires_confirmation(&self) -> bool {
        // set operations modify config, but we handle at operation level
        true
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let op = input["operation"].as_str().unwrap_or("?");
        let key = input["key"].as_str().unwrap_or("");
        if key.is_empty() {
            Some(format!("Config ({op})"))
        } else {
            Some(format!("Config ({op}: {key})"))
        }
    }
}

/// Resolve a dot-separated key path in a JSON value.
///
/// For example, `"model"` returns the `model` field,
/// and `"gitContext.enabled"` traverses into the nested object.
fn resolve_dot_path<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in key.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

/// Set a value at a dot-separated key path in a JSON object, creating
/// intermediate objects as needed.
fn set_dot_path(root: &mut Value, key: &str, new_value: Value) {
    let segments: Vec<&str> = key.split('.').collect();
    let mut current = root;
    for &segment in &segments[..segments.len().saturating_sub(1)] {
        if !current.get(segment).is_some_and(Value::is_object) {
            current[segment] = Value::Object(serde_json::Map::new());
        }
        current = current.get_mut(segment).expect("just created");
    }
    if let Some(&last) = segments.last() {
        current[last] = new_value;
    }
}

/// Read a setting value by dot-separated key path.
async fn get_setting(key: &str) -> Result<ToolOutput> {
    let settings = crab_config::settings::load_merged_settings(None)
        .map_err(|e| crab_core::Error::Config(format!("failed to load merged settings: {e}")))?;
    let json = serde_json::to_value(&settings)
        .map_err(|e| crab_core::Error::Config(format!("failed to serialize settings: {e}")))?;

    match resolve_dot_path(&json, key) {
        Some(val) => {
            let formatted = serde_json::to_string_pretty(val)
                .map_err(|e| crab_core::Error::Config(format!("failed to format value: {e}")))?;
            Ok(ToolOutput::success(formatted))
        }
        None => Ok(ToolOutput::error(format!("setting '{key}' not found"))),
    }
}

/// Write a setting value by dot-separated key path.
async fn set_setting(key: &str, value: &Value) -> Result<ToolOutput> {
    let project_dir = std::path::Path::new(".crab");
    if !project_dir.exists() {
        tokio::fs::create_dir_all(project_dir).await.map_err(|e| {
            crab_core::Error::Config(format!("failed to create .crab directory: {e}"))
        })?;
    }
    let settings_path = project_dir.join("settings.json");

    // Load existing project settings as raw JSON (or start with empty object).
    let mut root: Value = if settings_path.exists() {
        let content = tokio::fs::read_to_string(&settings_path)
            .await
            .map_err(|e| {
                crab_core::Error::Config(format!("failed to read {}: {e}", settings_path.display()))
            })?;
        serde_json::from_str(&content).unwrap_or(Value::Object(serde_json::Map::new()))
    } else {
        Value::Object(serde_json::Map::new())
    };

    set_dot_path(&mut root, key, value.clone());

    let output = serde_json::to_string_pretty(&root)
        .map_err(|e| crab_core::Error::Config(format!("failed to serialize settings: {e}")))?;
    tokio::fs::write(&settings_path, &output)
        .await
        .map_err(|e| {
            crab_core::Error::Config(format!("failed to write {}: {e}", settings_path.display()))
        })?;

    Ok(ToolOutput::success(format!(
        "Setting '{key}' updated in {}",
        settings_path.display()
    )))
}

/// List all current settings.
async fn list_settings() -> Result<ToolOutput> {
    let settings = crab_config::settings::load_merged_settings(None)
        .map_err(|e| crab_core::Error::Config(format!("failed to load merged settings: {e}")))?;
    let json = serde_json::to_string_pretty(&settings)
        .map_err(|e| crab_core::Error::Config(format!("failed to serialize settings: {e}")))?;
    Ok(ToolOutput::success(json))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let tool = ConfigTool;
        assert_eq!(tool.name(), "Config");
        assert!(!tool.description().is_empty());
        assert!(tool.requires_confirmation());
    }

    #[test]
    fn schema_has_required_fields() {
        let schema = ConfigTool.input_schema();
        assert_eq!(schema["required"], serde_json::json!(["operation"]));
        assert!(schema["properties"]["operation"].is_object());
        assert!(schema["properties"]["key"].is_object());
        assert!(schema["properties"]["value"].is_object());
    }
}
