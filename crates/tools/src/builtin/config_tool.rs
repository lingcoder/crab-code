//! `ConfigTool` — programmatic settings.json read/write.
//!
//! Provides get, set, and list operations on the merged configuration,
//! allowing the LLM to inspect and modify settings at runtime.

use crab_common::Result;
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
}

/// Read a setting value by dot-separated key path.
async fn get_setting(key: &str) -> Result<ToolOutput> {
    let _ = key;
    todo!("ConfigTool::get_setting — load merged config and resolve key path")
}

/// Write a setting value by dot-separated key path.
async fn set_setting(key: &str, value: &Value) -> Result<ToolOutput> {
    let _ = (key, value);
    todo!("ConfigTool::set_setting — update project-level settings.json")
}

/// List all current settings.
async fn list_settings() -> Result<ToolOutput> {
    todo!("ConfigTool::list_settings — dump merged config as formatted JSON")
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
