//! `MonitorTool` — file and process change monitoring.
//!
//! Watches a file path or process for changes and reports when a
//! modification is detected or a process exits. Useful for build-watch
//! loops and waiting for external processes to complete.

use crab_core::Result;
use crab_core::tool::{Tool, ToolContext, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Tool name constant for `MonitorTool`.
pub const MONITOR_TOOL_NAME: &str = "Monitor";

/// Default timeout in milliseconds (30 seconds).
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

// ── Input types ───────────────────────────────────────────────────────

/// Parsed input for the Monitor tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorInput {
    /// File or directory path to watch for changes.
    #[serde(default)]
    pub path: Option<String>,
    /// Process name to monitor.
    #[serde(default)]
    pub process_name: Option<String>,
    /// Timeout in milliseconds (default: 30000).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

// ── Tool implementation ───────────────────────────────────────────────

/// File and process change monitor.
///
/// Input schema:
/// ```json
/// {
///   "path": "<optional file/dir path>",
///   "process_name": "<optional process name>",
///   "timeout_ms": 30000
/// }
/// ```
///
/// At least one of `path` or `process_name` must be provided.
pub struct MonitorTool;

impl Tool for MonitorTool {
    fn name(&self) -> &str {
        MONITOR_TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Watch a file/directory for changes or monitor a process. Returns when \
         a change is detected, the process exits, or the timeout expires. \
         At least one of path or process_name must be provided."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File or directory path to watch for changes"
                },
                "process_name": {
                    "type": "string",
                    "description": "Process name to monitor"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 30000)",
                    "default": 30000
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + '_>> {
        Box::pin(async move {
            let parsed: MonitorInput = serde_json::from_value(input)
                .map_err(|e| crab_core::Error::Tool(format!("Invalid input: {e}")))?;

            if parsed.path.is_none() && parsed.process_name.is_none() {
                return Ok(ToolOutput::error(
                    "At least one of 'path' or 'process_name' must be provided",
                ));
            }

            let timeout = parsed.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);

            if let Some(ref path) = parsed.path {
                Ok(ToolOutput::error(format!(
                    "File monitoring for '{path}' (timeout: {timeout}ms) \
                     is not yet implemented. File system watch support is \
                     planned for a future release."
                )))
            } else if let Some(ref name) = parsed.process_name {
                Ok(ToolOutput::error(format!(
                    "Process monitoring for '{name}' (timeout: {timeout}ms) \
                     is not yet implemented. Process watch support is \
                     planned for a future release."
                )))
            } else {
                // unreachable due to earlier validation, but be safe
                Ok(ToolOutput::error(
                    "At least one of 'path' or 'process_name' must be provided",
                ))
            }
        })
    }

    fn format_use_summary(&self, input: &Value) -> Option<String> {
        let target = input["path"]
            .as_str()
            .or_else(|| input["process_name"].as_str())
            .unwrap_or("?");
        Some(format!("Monitor ({target})"))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_metadata() {
        let tool = MonitorTool;
        assert_eq!(tool.name(), "Monitor");
        assert!(tool.is_read_only());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn schema_has_optional_properties() {
        let schema = MonitorTool.input_schema();
        // No required fields — validation is in execute()
        assert!(schema.get("required").is_none());
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["process_name"].is_object());
        assert!(schema["properties"]["timeout_ms"].is_object());
    }

    #[test]
    fn input_parse_path_only() {
        let input: MonitorInput = serde_json::from_value(json!({
            "path": "/tmp/build.log"
        }))
        .unwrap();
        assert_eq!(input.path.as_deref(), Some("/tmp/build.log"));
        assert!(input.process_name.is_none());
        assert!(input.timeout_ms.is_none());
    }

    #[test]
    fn input_parse_process_only() {
        let input: MonitorInput = serde_json::from_value(json!({
            "process_name": "cargo",
            "timeout_ms": 60000
        }))
        .unwrap();
        assert!(input.path.is_none());
        assert_eq!(input.process_name.as_deref(), Some("cargo"));
        assert_eq!(input.timeout_ms, Some(60000));
    }
}
